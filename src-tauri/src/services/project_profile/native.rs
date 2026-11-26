use super::*;
use crate::services::profile::native_config::{self, NativeDocument, NativeEntries, NativeEntry};

pub(super) struct NativeOperation {
    key: String,
    path: Vec<String>,
    desired: Option<NativeEntry>,
    pub operation: ModelOperation,
}

pub(super) fn plan(
    app: &AppType,
    text: Option<&str>,
    settings: Option<&ClaudeSettingsDocument>,
    mcp: Option<&ProjectMcpDocument>,
    manifest: &ProjectManifest,
) -> Result<Vec<NativeOperation>, AppError> {
    let Some(text) = text else {
        return Ok(Vec::new());
    };
    let text = native_config::normalize(app, text, true)?;
    let desired = NativeDocument::parse(app, &text)?.entries()?;
    let (file, current) = match app {
        AppType::Claude => (
            CLAUDE_PROJECT_SETTINGS,
            NativeDocument::Json(
                settings
                    .ok_or_else(|| AppError::Config("缺少项目原生 JSON".into()))?
                    .root
                    .clone(),
            ),
        ),
        AppType::Codex => match mcp {
            Some(ProjectMcpDocument::Codex { document, .. }) => {
                (".codex/config.toml", NativeDocument::Toml(document.clone()))
            }
            _ => return Err(AppError::Config("缺少项目原生 TOML".into())),
        },
        _ => return Err(AppError::InvalidInput("此 Agent 不支持项目原生配置".into())),
    };
    let current_document = current;
    let mut current = current_document.entries()?;
    let mut keys = desired
        .keys()
        .chain(manifest.native_config_hashes.keys())
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    keys.sort();
    let mut operations = Vec::new();
    for key in keys {
        let path: Vec<String> = serde_json::from_str(&key)
            .map_err(|error| AppError::Config(format!("原生配置清单键无效: {error}")))?;
        if path.is_empty() || path.iter().any(|key| key.is_empty()) {
            return Err(AppError::Config("原生配置清单路径为空".into()));
        }
        if let Some(entry) = current_document.entry(&path)? {
            current.insert(key.clone(), entry);
        }
        // A previous manifest must not be able to remove a protected connection key.
        let mut probe = NativeDocument::parse(app, "")?;
        if let Some(entry) = current.get(&key).or_else(|| desired.get(&key)) {
            probe.set(Path::new(file), path.clone(), Some(entry))?;
            native_config::normalize(app, &probe.text()?, true)?;
        }
        let serialize = |entries: &NativeEntries| {
            entries
                .get(&key)
                .map(|entry| {
                    serde_json::to_string(&entry.value)
                        .map_err(|error| AppError::Config(error.to_string()))
                })
                .transpose()
        };
        let desired_text = serialize(&desired)?;
        let current_text = serialize(&current)?;
        let Some(operation) = plan_model_field_op(
            &format!("{file}#{}", path.join(".")),
            Some(desired_text),
            current_text,
            manifest.native_config_hashes.get(&key),
        ) else {
            continue;
        };
        operations.push(NativeOperation {
            key: key.clone(),
            path,
            desired: desired.get(&key).cloned(),
            operation,
        });
    }
    Ok(operations)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    root: &Path,
    app: &AppType,
    operations: &[NativeOperation],
    resolution: Option<ConflictResolution>,
    settings: Option<&mut ClaudeSettingsDocument>,
    mcp: Option<&mut ProjectMcpDocument>,
    manifest: &mut ProjectManifest,
    changed: &mut Vec<String>,
    undo: &mut Vec<ProjectFileUndo>,
) -> Result<(), AppError> {
    if operations.is_empty() {
        return Ok(());
    }
    let (path, mut document) = match app {
        AppType::Claude => {
            let settings = settings
                .as_ref()
                .ok_or_else(|| AppError::Config("缺少项目原生 JSON".into()))?;
            (
                settings.path.clone(),
                NativeDocument::Json(settings.root.clone()),
            )
        }
        AppType::Codex => match mcp.as_ref() {
            Some(ProjectMcpDocument::Codex { path, document, .. }) => {
                (path.clone(), NativeDocument::Toml(document.clone()))
            }
            _ => return Err(AppError::Config("缺少项目原生 TOML".into())),
        },
        _ => return Err(AppError::InvalidInput("此 Agent 不支持项目原生配置".into())),
    };
    let mut modified = false;
    let preserved = if matches!(resolution, Some(ConflictResolution::KeepCurrent)) {
        operations
            .iter()
            .filter(|operation| operation.operation.conflict.is_some())
            .map(|operation| operation.path.clone())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut operations = operations.iter().collect::<Vec<_>>();
    operations.sort_by_key(|operation| operation.desired.is_some());
    for native in operations {
        let operation = &native.operation;
        if preserved
            .iter()
            .any(|path| path.starts_with(&native.path) || native.path.starts_with(path))
        {
            manifest.native_config_hashes.remove(&native.key);
            continue;
        }
        if operation.conflict.is_some() {
            match resolution {
                Some(ConflictResolution::KeepCurrent) => {
                    manifest.native_config_hashes.remove(&native.key);
                    continue;
                }
                Some(ConflictResolution::Overwrite) => {}
                _ => continue,
            }
        }
        if operation.no_op && !manifest.native_config_hashes.contains_key(&native.key) {
            continue;
        }
        if !operation.no_op {
            document.set(&path, native.path.clone(), native.desired.as_ref())?;
            modified = true;
            changed.push(format!(
                "{}#{}",
                path.strip_prefix(root).unwrap_or(&path).display(),
                native.path.join(".")
            ));
        }
        match operation.desired_hash.as_ref() {
            Some(hash) => {
                manifest
                    .native_config_hashes
                    .insert(native.key.clone(), hash.clone());
            }
            None => {
                manifest.native_config_hashes.remove(&native.key);
            }
        }
    }
    match (document, settings, mcp) {
        (NativeDocument::Json(value), Some(settings), _) => {
            settings.root = value;
            settings.changed |= modified;
            let original = read_optional(&settings.path)?;
            save_manifest(root, "claude", manifest)?;
            if let Some(hash) = settings.save(root)? {
                undo.push(ProjectFileUndo {
                    path: settings.path.clone(),
                    original,
                    applied_hash: Some(hash),
                });
            }
        }
        (
            NativeDocument::Toml(value),
            _,
            Some(ProjectMcpDocument::Codex {
                document, changed, ..
            }),
        ) => {
            *document = value;
            *changed |= modified;
        }
        _ => return Err(AppError::Config("项目原生配置类型不匹配".into())),
    }
    Ok(())
}

pub(crate) fn capture_native_config(root: &Path, app: &AppType) -> Result<String, AppError> {
    let root = root
        .canonicalize()
        .map_err(|error| AppError::io(root, error))?;
    let files: &[&str] = match app {
        AppType::Claude => &[".claude/settings.json", CLAUDE_PROJECT_SETTINGS],
        AppType::Codex => &[".codex/config.toml"],
        _ => return native_config::normalize(app, "", false),
    };
    let mut merged = NativeDocument::parse(app, "")?;
    for file in files {
        let relative = Path::new(file);
        ensure_project_parent(&root, relative.parent().unwrap_or(Path::new(".")), false)?;
        let path = root.join(relative);
        ensure_regular_target(&path)?;
        let Some(bytes) = read_optional(&path)? else {
            continue;
        };
        let text =
            std::str::from_utf8(&bytes).map_err(|error| AppError::Config(error.to_string()))?;
        let safe = native_config::normalize(app, text, false)?;
        for entry in NativeDocument::parse(app, &safe)?.entries()?.values() {
            merged.set(&path, entry.path.clone(), Some(entry))?;
        }
    }
    merged.text()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Profile;
    use crate::test_support::TempUserHome;
    use serial_test::serial;

    fn seed(state: &AppState, root: &Path, scope: ProfileScope, text: &str) -> Profile {
        let mut payload = ProfilePayload::default();
        *payload.native_config.get_mut(&scope.apps()[0]).unwrap() = Some(text.into());
        let profile = Profile {
            id: "native".into(),
            name: "Native".into(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            project_path: Some(root.to_string_lossy().into()),
            created_at: None,
            updated_at: None,
        };
        state.db.save_profile(&profile).unwrap();
        profile
    }

    #[test]
    #[serial]
    fn project_native_config_uses_existing_conflict_resolution_and_preserves_other_fields() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join(CLAUDE_PROJECT_SETTINGS);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, r#"{"permissions":{"allow":["Read"],"deny":["Bash"]},"env":{"ANTHROPIC_AUTH_TOKEN":"keep"},"untouched":1}"#).unwrap();
        let profile = seed(
            &state,
            project.path(),
            ProfileScope::Claude,
            r#"{"permissions":{"allow":["Write"]},"future":{"enabled":true}}"#,
        );
        assert!(matches!(
            ProjectProfileService::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap(),
            ProjectDeployResult::NeedsResolution { .. }
        ));
        let before = fs::read(&file).unwrap();
        assert!(matches!(
            ProjectProfileService::deploy(
                &state,
                &profile.id,
                ProfileScope::Claude,
                Some(ConflictResolution::Skip)
            )
            .unwrap(),
            ProjectDeployResult::Skipped
        ));
        assert_eq!(fs::read(&file).unwrap(), before);
        ProjectProfileService::deploy(
            &state,
            &profile.id,
            ProfileScope::Claude,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        let current: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            current["permissions"]["allow"],
            serde_json::json!(["Write"])
        );
        assert_eq!(current["permissions"]["deny"], serde_json::json!(["Bash"]));
        assert_eq!(current["env"]["ANTHROPIC_AUTH_TOKEN"], "keep");
        seed(&state, project.path(), ProfileScope::Claude, "{}");
        ProjectProfileService::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap();
        let current: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert!(current["permissions"].get("allow").is_none());
        assert_eq!(current["permissions"]["deny"], serde_json::json!(["Bash"]));
        assert_eq!(current["untouched"], 1);
    }

    #[test]
    #[serial]
    fn project_native_toml_is_typed_comment_preserving_and_protected_keys_fail_before_writes() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join(".codex/config.toml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "# Keep comment\nmodel_provider='custom'\n[model_providers.custom]\nbase_url='https://keep.test'\n").unwrap();
        let profile = seed(&state, project.path(), ProfileScope::Codex, "sandbox_mode='read-only'\n[features]\nfuture=true\n[agents.worker]\nconfig_file='worker.toml'\n");
        ProjectProfileService::deploy(&state, &profile.id, ProfileScope::Codex, None).unwrap();
        let current = fs::read_to_string(&file).unwrap();
        assert!(current.contains("# Keep comment"));
        let parsed: toml::Value = toml::from_str(&current).unwrap();
        assert_eq!(
            parsed["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://keep.test")
        );
        assert_eq!(parsed["features"]["future"].as_bool(), Some(true));
        assert_eq!(
            parsed["agents"]["worker"]["config_file"].as_str(),
            Some("worker.toml")
        );
        seed(
            &state,
            project.path(),
            ProfileScope::Codex,
            "experimental_bearer_token='forbidden'",
        );
        assert!(ProjectProfileService::deploy(
            &state,
            &profile.id,
            ProfileScope::Codex,
            Some(ConflictResolution::Overwrite)
        )
        .is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), current);
    }

    #[test]
    #[serial]
    fn parent_replacement_requires_conflict_choice_and_keep_preserves_the_entire_subtree() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join(CLAUDE_PROJECT_SETTINGS);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, r#"{"future":{"handwritten":true}}"#).unwrap();
        let profile = seed(
            &state,
            project.path(),
            ProfileScope::Claude,
            r#"{"future":{"managed":1}}"#,
        );
        ProjectProfileService::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap();
        seed(
            &state,
            project.path(),
            ProfileScope::Claude,
            r#"{"future":"replacement"}"#,
        );
        assert!(matches!(
            ProjectProfileService::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap(),
            ProjectDeployResult::NeedsResolution { .. }
        ));
        ProjectProfileService::deploy(
            &state,
            &profile.id,
            ProfileScope::Claude,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            value["future"],
            serde_json::json!({"handwritten":true,"managed":1})
        );
    }
}
