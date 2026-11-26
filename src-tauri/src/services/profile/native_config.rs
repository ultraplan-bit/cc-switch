//! Native Agent behavior owned by Profiles. Connection/authentication stays in providers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use super::{profile_model_keys, AppType, ProfilePayload, ProfileScope};
use crate::error::AppError;
use crate::live::floor;
use crate::live::patch::{json::JsonPatch, toml::TomlPatch, KeyPath, LivePatch, LiveWriteError};

pub(crate) fn claude_connection_env(key: &str) -> bool {
    floor::claude_floor_env(key)
        && !super::CLAUDE_PROFILE_MODEL_KEYS.contains(&key)
        && !matches!(key, "ANTHROPIC_MODEL" | "ANTHROPIC_SMALL_FAST_MODEL")
}

pub(crate) fn claude_connection_top(key: &str) -> bool {
    matches!(
        key,
        "apiKeyHelper"
            | "apiBaseUrl"
            | "apiKey"
            | "awsAuthRefresh"
            | "awsCredentialExport"
            | "gcpAuthRefresh"
    )
}

pub(crate) fn codex_connection_top(key: &str) -> bool {
    matches!(
        key,
        "model_provider"
            | "openai_base_url"
            | "model_providers"
            | "experimental_bearer_token"
            | "base_url"
            | "wire_api"
            | "auth"
    )
}

fn reserved(app: &AppType, path: &[String]) -> bool {
    let Some(first) = path.first().map(String::as_str) else {
        return false;
    };
    match app {
        AppType::Claude => match path {
            [first, key] if first == "env" => {
                claude_connection_env(key)
                    || key == "ANTHROPIC_MODEL"
                    || profile_model_keys(app).contains(&key.as_str())
            }
            [_] => {
                claude_connection_top(first)
                    || matches!(
                        first,
                        "model"
                            | "mcpServers"
                            | "skillOverrides"
                            | "deniedMcpServers"
                            | "enabledMcpjsonServers"
                            | "disabledMcpjsonServers"
                            | "enableAllProjectMcpServers"
                            | "api_format"
                            | "apiFormat"
                    )
            }
            _ => false,
        },
        AppType::Codex => {
            let joined = path.join(".");
            path.len() == 1
                && (codex_connection_top(first)
                    || matches!(
                        first,
                        "model" | "model_catalog_json" | "mcp_servers" | "mcp" | "skills"
                    )
                    || profile_model_keys(app).contains(&first))
                || path.len() == 2 && profile_model_keys(app).contains(&joined.as_str())
        }
        AppType::ClaudeDesktop => {
            path.len() == 1 && (floor::desktop_profile_floor(first) || claude_connection_top(first))
                || matches!(path, [first, key] if first == "env" && floor::claude_floor_env(key))
        }
        _ => true,
    }
}

#[derive(Clone)]
pub(crate) enum NativeDocument {
    Json(Value),
    Toml(DocumentMut),
}

#[derive(Clone)]
pub(crate) struct NativeEntry {
    pub path: Vec<String>,
    pub value: Value,
    pub toml: Option<Item>,
}

pub(crate) type NativeEntries = BTreeMap<String, NativeEntry>;

impl NativeDocument {
    pub(crate) fn entry(&self, path: &[String]) -> Result<Option<NativeEntry>, AppError> {
        match self {
            Self::Json(value) => {
                let mut value = value;
                for key in path {
                    let Some(next) = value.as_object().and_then(|object| object.get(key)) else {
                        return Ok(None);
                    };
                    value = next;
                }
                Ok(Some(NativeEntry {
                    path: path.to_vec(),
                    value: value.clone(),
                    toml: None,
                }))
            }
            Self::Toml(document) => {
                let Some((last, parents)) = path.split_last() else {
                    return Ok(None);
                };
                let mut table: &dyn toml_edit::TableLike = document.as_table();
                for key in parents {
                    let Some(next) = table.get(key).and_then(Item::as_table_like) else {
                        return Ok(None);
                    };
                    table = next;
                }
                let Some(item) = table.get(last) else {
                    return Ok(None);
                };
                let mut wrapper = DocumentMut::new();
                wrapper["value"] = item.clone();
                let value: toml::Value = toml::from_str(&wrapper.to_string())
                    .map_err(|error| AppError::Config(error.to_string()))?;
                Ok(Some(NativeEntry {
                    path: path.to_vec(),
                    value: serde_json::to_value(&value["value"])
                        .map_err(|error| AppError::Config(error.to_string()))?,
                    toml: Some(item.clone()),
                }))
            }
        }
    }
    pub(crate) fn parse(app: &AppType, text: &str) -> Result<Self, AppError> {
        if text.len() > 16 * 1024 * 1024 {
            return Err(AppError::InvalidInput("原生配置超过 16 MiB".into()));
        }
        if app == &AppType::Codex {
            Ok(Self::Toml(text.parse().map_err(|error| {
                AppError::Config(format!("原生 TOML 无效: {error}"))
            })?))
        } else {
            let value: Value =
                serde_json::from_str(if text.trim().is_empty() { "{}" } else { text })
                    .map_err(|error| AppError::Config(format!("原生 JSON 无效: {error}")))?;
            if !value.is_object() {
                return Err(AppError::InvalidInput("原生配置必须是对象".into()));
            }
            Ok(Self::Json(value))
        }
    }

    pub(crate) fn text(&self) -> Result<String, AppError> {
        match self {
            Self::Json(value) => serde_json::to_string_pretty(value)
                .map_err(|error| AppError::Config(error.to_string())),
            Self::Toml(document) => Ok(document.to_string()),
        }
    }

    pub(crate) fn entries(&self) -> Result<NativeEntries, AppError> {
        let mut entries = NativeEntries::new();
        fn json(value: &Value, path: Vec<String>, entries: &mut NativeEntries) {
            if let Some(object) = value.as_object().filter(|object| !object.is_empty()) {
                for (key, value) in object {
                    let mut next = path.clone();
                    next.push(key.clone());
                    json(value, next, entries);
                }
            } else if !path.is_empty() {
                entries.insert(
                    serde_json::to_string(&path).expect("path serializes"),
                    NativeEntry {
                        path,
                        value: value.clone(),
                        toml: None,
                    },
                );
            }
        }
        fn toml(
            table: &dyn toml_edit::TableLike,
            path: Vec<String>,
            entries: &mut NativeEntries,
        ) -> Result<(), AppError> {
            for (key, item) in table.iter() {
                let mut next = path.clone();
                next.push(key.to_string());
                if let Some(children) = item.as_table_like().filter(|table| !table.is_empty()) {
                    toml(children, next, entries)?;
                } else {
                    let mut wrapper = DocumentMut::new();
                    wrapper["value"] = item.clone();
                    let value: toml::Value = toml::from_str(&wrapper.to_string())
                        .map_err(|error| AppError::Config(error.to_string()))?;
                    entries.insert(
                        serde_json::to_string(&next).expect("path serializes"),
                        NativeEntry {
                            path: next,
                            value: serde_json::to_value(&value["value"])
                                .map_err(|error| AppError::Config(error.to_string()))?,
                            toml: Some(item.clone()),
                        },
                    );
                }
            }
            Ok(())
        }
        match self {
            Self::Json(value) => json(value, vec![], &mut entries),
            Self::Toml(document) => toml(document.as_table(), vec![], &mut entries)?,
        }
        Ok(entries)
    }

    pub(crate) fn set(
        &mut self,
        path: &Path,
        key: Vec<String>,
        entry: Option<&NativeEntry>,
    ) -> Result<(), AppError> {
        match self {
            Self::Json(value) => {
                let mut patch = JsonPatch::default();
                if let Some(entry) = entry {
                    patch.set.push((KeyPath(key), entry.value.clone()));
                } else {
                    patch.remove.push(KeyPath(key));
                }
                patch.apply_to(path, value)?;
            }
            Self::Toml(document) => {
                let mut patch = TomlPatch::default();
                if let Some(entry) = entry {
                    patch.set.push((
                        KeyPath(key),
                        entry
                            .toml
                            .clone()
                            .ok_or_else(|| AppError::Config("缺少 TOML 配置值".into()))?,
                    ));
                } else {
                    patch.remove.push(KeyPath(key));
                }
                patch.apply_to(path, document)?;
            }
        }
        Ok(())
    }
}

/// Extraction removes protected fields. Explicit editing rejects them so users
/// cannot believe a connection/typed field was saved while it was discarded.
pub(crate) fn normalize(app: &AppType, text: &str, strict: bool) -> Result<String, AppError> {
    let mut document = NativeDocument::parse(app, text)?;
    let shared_container = if app == &AppType::Codex {
        "agents"
    } else {
        "env"
    };
    let valid_container = match &document {
        NativeDocument::Json(value) => value.get(shared_container).is_none_or(Value::is_object),
        NativeDocument::Toml(value) => value
            .get(shared_container)
            .is_none_or(|item| item.as_table_like().is_some()),
    };
    if !valid_container {
        return Err(AppError::InvalidInput(format!(
            "{shared_container} 必须是对象或表，不能覆盖连接与模型容器"
        )));
    }
    let entries = document.entries()?;
    let mut removed = BTreeMap::new();
    for entry in entries.values() {
        for length in 1..=entry.path.len() {
            let prefix = &entry.path[..length];
            if reserved(app, prefix) {
                removed.insert(
                    serde_json::to_string(prefix).expect("path serializes"),
                    prefix.to_vec(),
                );
                break;
            }
        }
    }
    if strict && !removed.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "以下字段由供应商、模型或资源配置管理，不能写入原生行为配置：{}",
            removed
                .values()
                .map(|path| path.join("."))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    for path in removed.into_values() {
        document.set(Path::new("profile-native-config"), path, None)?;
    }
    // Empty shared containers cannot replace their connection/model children.
    // Removing their last filtered key must not turn extraction into a whole-env
    // assignment that would clear credentials when applied later.
    let shared_container = if app == &AppType::Codex {
        "agents"
    } else {
        "env"
    };
    let empty = match &document {
        NativeDocument::Json(value) => value
            .get(shared_container)
            .and_then(Value::as_object)
            .is_some_and(|value| value.is_empty()),
        NativeDocument::Toml(value) => value
            .get(shared_container)
            .and_then(Item::as_table_like)
            .is_some_and(|value| value.is_empty()),
    };
    if empty {
        document.set(
            Path::new("profile-native-config"),
            vec![shared_container.into()],
            None,
        )?;
    }
    document.text()
}

pub(crate) fn from_provider(app: &AppType, settings: &Value) -> Result<String, AppError> {
    if app == &AppType::ClaudeDesktop {
        // Desktop provider rows are CC Switch's env/model-mapping schema, not a
        // native configLibrary document. Only native policy fields are transferable.
        let mut policy = serde_json::Map::new();
        for key in floor::DESKTOP_PROFILE_SEED {
            if let Some(value) = settings.get(*key) {
                policy.insert((*key).into(), value.clone());
            }
        }
        return serde_json::to_string_pretty(&policy)
            .map_err(|error| AppError::Config(error.to_string()));
    }
    let text = if app == &AppType::Codex {
        settings
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        serde_json::to_string(settings).map_err(|error| AppError::Config(error.to_string()))?
    };
    normalize(app, &text, false)
}

/// Only behavior that the original provider projector actually writes. Shared
/// inactive fields in a legacy row are not its property and must not be removed.
pub(crate) fn projected_provider_behavior(
    app: &AppType,
    settings: &Value,
) -> Result<NativeEntries, AppError> {
    let native = from_provider(app, settings)?;
    let mut entries = NativeDocument::parse(app, &native)?.entries()?;
    entries.retain(|_, entry| match app {
        AppType::Claude => match entry.path.as_slice() {
            [parent, key, ..] if parent == "env" => {
                floor::claude_floor_env(key) || floor::claude_exclusive_env(key)
            }
            [key, ..] => floor::claude_floor_top(key),
            _ => false,
        },
        AppType::Codex => {
            entry.path.first().is_some_and(|key| {
                floor::CODEX_FLOOR_TOP.contains(&key.as_str())
                    || floor::CODEX_EXCLUSIVE_TOP.contains(&key.as_str())
            }) || floor::CODEX_FLOOR_NESTED.iter().any(|path| {
                entry
                    .path
                    .iter()
                    .map(String::as_str)
                    .eq(path.iter().copied())
            })
        }
        _ => false,
    });
    Ok(entries)
}

pub(crate) fn overlay_provider_behavior(
    app: &AppType,
    settings: &Value,
    native: &str,
    document: &mut NativeDocument,
) -> Result<(), AppError> {
    let safe = normalize(app, native, true)?;
    let desired = NativeDocument::parse(app, &safe)?.entries()?;
    for (key, entry) in projected_provider_behavior(app, settings)? {
        if !desired.contains_key(&key) {
            document.set(Path::new("profile-effective"), entry.path, None)?;
        }
    }
    for entry in desired.values() {
        document.set(
            Path::new("profile-effective"),
            entry.path.clone(),
            Some(entry),
        )?;
    }
    Ok(())
}

pub(crate) fn global_path(app: &AppType) -> Result<PathBuf, AppError> {
    match app {
        AppType::Claude => Ok(crate::config::get_claude_settings_path()),
        AppType::Codex => Ok(crate::codex_config::get_codex_config_path()),
        AppType::ClaudeDesktop => Ok(crate::claude_desktop_config::get_config_library_path()?
            .join(format!("{}.json", crate::claude_desktop_config::PROFILE_ID))),
        _ => Err(AppError::InvalidInput("此 Agent 不支持 Profile".into())),
    }
}

pub(crate) fn capture(app: &AppType) -> Result<String, AppError> {
    let path = global_path(app)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(AppError::io(&path, error)),
    };
    normalize(app, &text, false)
}

struct NativeGlobalPatch {
    app: AppType,
    expected: NativeEntries,
    desired: NativeEntries,
    previous: NativeEntries,
}

impl LivePatch for NativeGlobalPatch {
    fn apply(&self, path: &Path, bytes: Option<&[u8]>) -> Result<Vec<u8>, LiveWriteError> {
        let run = || -> Result<Vec<u8>, AppError> {
            let text = std::str::from_utf8(bytes.unwrap_or_default())
                .map_err(|error| AppError::Config(error.to_string()))?;
            let mut document = NativeDocument::parse(&self.app, text)?;
            let observed = document.clone();
            let mut operations = BTreeMap::<String, Option<&NativeEntry>>::new();
            for (key, entry) in &self.desired {
                operations.insert(key.clone(), Some(entry));
            }
            for key in self.previous.keys() {
                operations.entry(key.clone()).or_insert(None);
            }
            // Remove old leaf paths before replacing an ancestor with a scalar.
            let mut operations = operations.into_iter().collect::<Vec<_>>();
            operations.sort_by_key(|(_, desired)| desired.is_some());
            for (key, desired) in operations {
                let before = self.expected.get(&key).map(|entry| &entry.value);
                let reference = desired
                    .or_else(|| self.previous.get(&key))
                    .expect("planned entry");
                let actual = observed.entry(&reference.path)?;
                let now = actual.as_ref().map(|entry| &entry.value);
                if now != before && now != desired.map(|entry| &entry.value) {
                    return Err(AppError::Config(format!(
                        "原生配置在应用期间被修改：{}",
                        key
                    )));
                }
                if desired.is_none() && now != self.previous.get(&key).map(|entry| &entry.value) {
                    continue;
                }
                let entry = desired
                    .or_else(|| self.previous.get(&key))
                    .expect("planned entry");
                document.set(path, entry.path.clone(), desired)?;
            }
            Ok(document.text()?.into_bytes())
        };
        run().map_err(|error| LiveWriteError::Parse {
            path: path.to_path_buf(),
            line: 1,
            column: 1,
            message: error.to_string(),
        })
    }
}

pub(crate) fn apply_global(
    state: &crate::store::AppState,
    payload: &ProfilePayload,
    app: &AppType,
) -> Result<(), AppError> {
    let Some(text) = payload.native_config.get(app).and_then(Option::as_deref) else {
        return Ok(());
    };
    let text = normalize(app, text, true)?;
    let current_provider =
        crate::mode::current::provider_for(&state.db, app, crate::mode::current::Purpose::InUse)?
            .map(|id| state.db.read_provider(app, &id))
            .transpose()?
            .flatten()
            .and_then(Result::ok);
    let operation = crate::mode::operation::AppWrite::begin(&state.db, app.as_str())?;
    let path = global_path(app)?;
    let current = crate::live::engine::read_current(&path)?;
    let current_text = std::str::from_utf8(current.as_deref().unwrap_or_default())
        .map_err(|error| AppError::Config(error.to_string()))?;
    let current_document = NativeDocument::parse(app, current_text)?;
    // Store applied ownership in the same recoverable file operation as the
    // native file; a separate database write after publishing could lose it.
    let ledger_path = operation
        .store
        .file(&format!("profile-native-{}.config", app.as_str()));
    let prior_bytes = crate::live::engine::read_current(&ledger_path)?;
    let prior = std::str::from_utf8(prior_bytes.as_deref().unwrap_or_default())
        .map_err(|error| AppError::Config(error.to_string()))?;
    let prior = normalize(app, prior, true)?;
    let desired = NativeDocument::parse(app, &text)?.entries()?;
    let mut previous = NativeDocument::parse(app, &prior)?.entries()?;
    // A first application has no ledger, but switching the referenced provider
    // just projected its legacy defaults. Honor explicit Profile removals of
    // those defaults without treating inactive/shared row data as owned.
    if let Some(provider) = current_provider {
        for (key, entry) in projected_provider_behavior(app, &provider.settings_config)? {
            if current_document
                .entry(&entry.path)?
                .is_some_and(|current| current.value == entry.value)
            {
                previous.insert(key, entry);
            }
        }
    }
    let mut expected = current_document.entries()?;
    for (key, entry) in desired.iter().chain(previous.iter()) {
        if let Some(value) = current_document.entry(&entry.path)? {
            expected.insert(key.clone(), value);
        }
    }
    let patch = NativeGlobalPatch {
        app: app.clone(),
        expected,
        desired,
        previous,
    };
    let ledger = crate::live::patch::Guarded {
        expected_pre: crate::live::engine::digest(prior_bytes.as_deref()),
        then: crate::live::patch::WholeFile::Write(text.into_bytes()),
    };
    operation.run(
        "profile-native-config",
        &[
            crate::mode::operation::FileChange {
                file: crate::live::engine::LiveFile::private(path),
                patch: &patch,
            },
            crate::mode::operation::FileChange {
                file: crate::live::engine::LiveFile::private(ledger_path),
                patch: &ledger,
            },
        ],
        crate::mode::state::PendingTarget::default(),
    )?;
    Ok(())
}

pub(crate) fn provider_payload(
    state: &crate::store::AppState,
    scope: ProfileScope,
    provider: &crate::provider::Provider,
) -> Result<ProfilePayload, AppError> {
    let app = &scope.apps()[0];
    let mut payload = ProfilePayload::default();
    *payload.providers.get_mut(app).unwrap() = Some(provider.id.clone());
    *payload.native_config.get_mut(app).unwrap() =
        Some(from_provider(app, &provider.settings_config)?);
    if app == &AppType::Claude {
        let env = provider.settings_config.get("env");
        payload.models.claude = Some(
            env.and_then(|env| env.get("ANTHROPIC_MODEL"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        );
        payload.model_settings.claude = Some(
            profile_model_keys(app)
                .iter()
                .filter_map(|key| {
                    env.and_then(|env| env.get(*key))
                        .and_then(Value::as_str)
                        .map(|value| ((*key).to_string(), value.to_string()))
                })
                .collect(),
        );
    } else if app == &AppType::Codex {
        let text = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let document = text
            .parse::<DocumentMut>()
            .map_err(|error| AppError::Config(error.to_string()))?;
        payload.models.codex = Some(
            document
                .get("model")
                .and_then(Item::as_str)
                .unwrap_or_default()
                .into(),
        );
        payload.model_settings.codex = Some(
            profile_model_keys(app)
                .iter()
                .filter_map(|key| {
                    let value = if let Some((section, field)) = key.split_once('.') {
                        document
                            .get(section)
                            .and_then(Item::as_table_like)
                            .and_then(|table| table.get(field))
                    } else {
                        document.get(key)
                    };
                    value
                        .and_then(|value| {
                            value
                                .as_str()
                                .map(str::to_string)
                                .or_else(|| value.as_integer().map(|value| value.to_string()))
                        })
                        .map(|value| ((*key).to_string(), value))
                })
                .collect(),
        );
        payload.model_catalog.codex = super::provider_model_catalog(state, app, &provider.id)?;
    }
    if app != &AppType::ClaudeDesktop {
        let servers = state
            .db
            .get_mcp_server_rows()?
            .into_readable("迁移 Profile 资源引用");
        let skills = state.db.get_all_installed_skills()?;
        let prompts = state.db.get_prompts(app)?;
        *payload.mcp.get_mut(app).unwrap() = Some(
            servers
                .values()
                .filter(|server| server.apps.is_enabled_for(app))
                .map(|server| server.id.clone())
                .collect(),
        );
        *payload.skills.get_mut(app).unwrap() = Some(
            skills
                .values()
                .filter(|skill| skill.apps.is_enabled_for(app))
                .map(|skill| skill.id.clone())
                .collect(),
        );
        *payload.prompts.get_mut(app).unwrap() = Some(
            prompts
                .values()
                .find(|prompt| prompt.enabled)
                .map(|prompt| prompt.id.clone())
                .unwrap_or_default(),
        );
        let names = payload.resource_names.get_mut(app).unwrap();
        super::remember_resource_names(
            &mut names.mcp,
            payload
                .mcp
                .get(app)
                .and_then(Option::as_deref)
                .unwrap_or_default(),
            servers.values().map(|server| (&server.id, &server.name)),
        );
        super::remember_resource_names(
            &mut names.skills,
            payload
                .skills
                .get(app)
                .and_then(Option::as_deref)
                .unwrap_or_default(),
            skills.values().map(|skill| (&skill.id, &skill.name)),
        );
        super::remember_resource_names(
            &mut names.prompts,
            payload.prompts.get(app).unwrap().as_slice(),
            prompts.values().map(|prompt| (&prompt.id, &prompt.name)),
        );
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::store::AppState;
    use crate::test_support::TempUserHome;
    use serial_test::serial;

    #[test]
    fn native_config_extraction_keeps_unknown_behavior_and_additional_models_but_not_connections() {
        let claude = normalize(&AppType::Claude, r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"private","ANTHROPIC_BASE_URL":"https://private.test","ANTHROPIC_MODEL":"typed","CUSTOM_FEATURE":"on"},"model":"typed","fallbackModel":"fallback","modelOverrides":{"a":"b"},"advisorModel":"advisor","permissions":{"allow":["Read"]},"future":{"nested":[1,2]}}"#, false).unwrap();
        let value: Value = serde_json::from_str(&claude).unwrap();
        assert_eq!(value["env"]["CUSTOM_FEATURE"], "on");
        assert_eq!(value["fallbackModel"], "fallback");
        assert_eq!(value["modelOverrides"]["a"], "b");
        assert_eq!(value["future"]["nested"], serde_json::json!([1, 2]));
        assert!(!claude.contains("private"));
        assert!(normalize(
            &AppType::Claude,
            r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"private"}}"#,
            true
        )
        .is_err());
        let codex = normalize(&AppType::Codex, "model='typed'\nreview_model='review'\nmodel_provider='private'\nmodel_context_window=123\n[model_providers.private]\nbase_url='https://private.test'\n[agents]\nmax_threads=3\nfuture_feature=true\n[future]\nnested=['a','b']\n", false).unwrap();
        let document: toml::Value = toml::from_str(&codex).unwrap();
        assert_eq!(document["review_model"].as_str(), Some("review"));
        assert_eq!(document["agents"]["future_feature"].as_bool(), Some(true));
        assert!(document.get("model_provider").is_none());
        assert!(!codex.contains("private"));
        assert!(normalize(&AppType::Codex, "[agents]\nmax_threads=2", true).is_err());
    }

    #[test]
    fn desktop_provider_native_migration_never_copies_environment_credentials_or_mapping_metadata()
    {
        let source = serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"private","ANTHROPIC_BASE_URL":"https://private.test","ANTHROPIC_MODEL":"model"},"modelMapping":{"x":"y"},"disableDeploymentModeChooser":true,"coworkEgressAllowedHosts":["example.test"]});
        let text = from_provider(&AppType::ClaudeDesktop, &source).unwrap();
        let native: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            native,
            serde_json::json!({"disableDeploymentModeChooser":true,"coworkEgressAllowedHosts":["example.test"]})
        );
        assert!(!text.contains("private"));
    }

    #[test]
    #[serial]
    fn native_global_application_preserves_connections_and_unowned_fields_and_removes_previous_owned_fields(
    ) {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let path = global_path(&AppType::Claude).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"keep"},"unrelated":7}"#,
        )
        .unwrap();
        let mut payload = ProfilePayload::default();
        payload.native_config.claude =
            Some(r#"{"permissions":{"allow":["Read"]},"future":{"nested":"x"}}"#.into());
        apply_global(&state, &payload, &AppType::Claude).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["env"]["ANTHROPIC_AUTH_TOKEN"], "keep");
        assert_eq!(value["unrelated"], 7);
        assert_eq!(value["permissions"]["allow"], serde_json::json!(["Read"]));
        payload.native_config.claude = Some(r#"{"future":"scalar"}"#.into());
        apply_global(&state, &payload, &AppType::Claude).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["future"], "scalar");
        assert!(value["permissions"].get("allow").is_none());
        assert_eq!(value["env"]["ANTHROPIC_AUTH_TOKEN"], "keep");
        let ledger =
            crate::live::engine::DeviceStore::for_device().file("profile-native-claude.config");
        assert!(ledger.is_file());
    }

    #[test]
    fn native_patch_rejects_concurrent_edit_without_overwriting_other_keys() {
        let app = AppType::Codex;
        let expected = NativeDocument::parse(&app, "sandbox_mode='read-only'\nkeep=1")
            .unwrap()
            .entries()
            .unwrap();
        let patch = NativeGlobalPatch {
            app: app.clone(),
            expected,
            desired: NativeDocument::parse(&app, "sandbox_mode='workspace-write'")
                .unwrap()
                .entries()
                .unwrap(),
            previous: NativeEntries::new(),
        };
        assert!(patch
            .apply(
                Path::new("config.toml"),
                Some(b"sandbox_mode='danger-full-access'\nkeep=1")
            )
            .is_err());
        let result = patch
            .apply(
                Path::new("config.toml"),
                Some(b"sandbox_mode='read-only'\nkeep=2"),
            )
            .unwrap();
        let document: toml::Value = toml::from_str(std::str::from_utf8(&result).unwrap()).unwrap();
        assert_eq!(document["keep"].as_integer(), Some(2));
        assert_eq!(document["sandbox_mode"].as_str(), Some("workspace-write"));
    }

    #[test]
    fn native_shared_containers_cannot_replace_connection_or_typed_children() {
        assert!(normalize(&AppType::Claude, r#"{"env":"bad"}"#, true).is_err());
        assert!(normalize(&AppType::Codex, "agents=1", true).is_err());
        let json: Value =
            serde_json::from_str(&normalize(&AppType::Claude, r#"{"env":{}}"#, true).unwrap())
                .unwrap();
        assert!(json.get("env").is_none());
        let toml: toml::Value =
            toml::from_str(&normalize(&AppType::Codex, "[agents]", true).unwrap()).unwrap();
        assert!(toml.get("agents").is_none());
    }

    #[test]
    #[serial]
    fn effective_profile_projection_is_read_only_and_preserves_the_provider_connection() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let provider = crate::provider::Provider::with_id(
            "route".into(),
            "Route".into(),
            serde_json::json!({"auth":{"OPENAI_API_KEY":"keep"},"config":"model='provider-default'\nmodel_provider='custom'\n[model_providers.custom]\nbase_url='https://keep.test'"}),
            None,
        );
        state.db.save_provider(&AppType::Codex, &provider).unwrap();
        let mut payload = ProfilePayload::default();
        payload.models.codex = Some("profile-model".into());
        payload.model_settings.codex = Some(BTreeMap::from([(
            "model_context_window".into(),
            "123456".into(),
        )]));
        payload.model_catalog.codex =
            Some(serde_json::json!({"models":[{"model":"profile-model"}]}));
        payload.native_config.codex = Some("[features]\nfuture=true".into());
        let profile = crate::database::Profile {
            id: "active".into(),
            name: "Active".into(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            project_path: None,
            created_at: None,
            updated_at: None,
        };
        state.db.save_profile(&profile).unwrap();
        state
            .db
            .set_current_profile_id("codex", Some(&profile.id))
            .unwrap();
        let effective = super::super::ProfileService::effective_provider_for_applied_profile(
            &state,
            &AppType::Codex,
            &provider,
        )
        .unwrap();
        let document: toml::Value =
            toml::from_str(effective.settings_config["config"].as_str().unwrap()).unwrap();
        assert_eq!(document["model"].as_str(), Some("profile-model"));
        assert_eq!(document["model_context_window"].as_integer(), Some(123456));
        assert_eq!(document["features"]["future"].as_bool(), Some(true));
        assert_eq!(effective.settings_config["auth"]["OPENAI_API_KEY"], "keep");
        assert_eq!(
            state
                .db
                .get_provider_by_id("route", &AppType::Codex)
                .unwrap()
                .unwrap()
                .settings_config,
            provider.settings_config
        );
        assert_eq!(
            state.db.get_profile(&profile.id).unwrap().unwrap().payload,
            profile.payload
        );
    }

    #[test]
    #[serial]
    fn first_profile_apply_and_reattach_do_not_resurrect_removed_provider_behavior() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let provider = crate::provider::Provider::with_id(
            "connection".into(),
            "Connection".into(),
            serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"keep","ANTHROPIC_BASE_URL":"https://example.test","CLAUDE_CODE_MAX_OUTPUT_TOKENS":"1000","SHARED_FEATURE":"row-value"},"permissions":{"allow":["Read"]}}),
            None,
        );
        state.db.save_provider(&AppType::Claude, &provider).unwrap();
        let path = global_path(&AppType::Claude).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"env":{"SHARED_FEATURE":"global-value"},"permissions":{"allow":["Write"]}}"#,
        )
        .unwrap();
        let mut payload = ProfilePayload::default();
        payload.providers.claude = Some(provider.id.clone());
        payload.native_config.claude = Some("{}".into());
        let profile = crate::database::Profile {
            id: "removed-default".into(),
            name: "Removed default".into(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            project_path: None,
            created_at: None,
            updated_at: None,
        };
        state.db.save_profile(&profile).unwrap();
        let warnings =
            super::super::ProfileService::apply(&state, &profile.id, ProfileScope::Claude).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let current: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(current["env"]
            .get("CLAUDE_CODE_MAX_OUTPUT_TOKENS")
            .is_none());
        assert_eq!(current["env"]["ANTHROPIC_AUTH_TOKEN"], "keep");
        assert_eq!(current["env"]["SHARED_FEATURE"], "global-value");
        assert_eq!(
            current["permissions"]["allow"],
            serde_json::json!(["Write"])
        );
        let effective = super::super::ProfileService::effective_provider_for_applied_profile(
            &state,
            &AppType::Claude,
            &provider,
        )
        .unwrap();
        assert!(effective.settings_config["env"]
            .get("CLAUDE_CODE_MAX_OUTPUT_TOKENS")
            .is_none());
        assert_eq!(
            effective.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"],
            "keep"
        );
        assert_eq!(
            effective.settings_config["permissions"], provider.settings_config["permissions"],
            "inactive shared row fields are not deleted"
        );
        assert_eq!(
            state
                .db
                .get_provider_by_id(&provider.id, &AppType::Claude)
                .unwrap()
                .unwrap()
                .settings_config,
            provider.settings_config
        );
    }

    #[test]
    fn native_overlay_removes_only_provider_projected_behavior_defaults() {
        let source = serde_json::json!({"auth":{"OPENAI_API_KEY":"keep"},"config":"model='typed'\nreview_model='remove'\nmodel_verbosity='low'\nsandbox_mode='workspace-write'\n[agents]\nmax_threads=5\n[model_providers.custom]\nbase_url='https://keep.test'"});
        let mut document =
            NativeDocument::parse(&AppType::Codex, source["config"].as_str().unwrap()).unwrap();
        overlay_provider_behavior(&AppType::Codex, &source, "", &mut document).unwrap();
        let value: toml::Value = toml::from_str(&document.text().unwrap()).unwrap();
        assert!(value.get("review_model").is_none());
        assert!(value.get("model_verbosity").is_none());
        assert_eq!(value["sandbox_mode"].as_str(), Some("workspace-write"));
        assert_eq!(value["agents"]["max_threads"].as_integer(), Some(5));
        assert_eq!(value["model"].as_str(), Some("typed"));
        assert_eq!(
            value["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://keep.test")
        );
    }

    #[test]
    #[serial]
    fn explicit_empty_model_overrides_remain_cleared_in_restart_projection() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        for app in [AppType::Claude, AppType::Codex] {
            let settings = if app == AppType::Claude {
                serde_json::json!({"model":"old","env":{"ANTHROPIC_AUTH_TOKEN":"keep","ANTHROPIC_MODEL":"old","ANTHROPIC_DEFAULT_SONNET_MODEL":"old-role","CLAUDE_CODE_SUBAGENT_MODEL":"old-agent"}})
            } else {
                serde_json::json!({"auth":{"OPENAI_API_KEY":"keep"},"config":"model='old'\nmodel_context_window=1000\n[agents]\ndefault_subagent_model='old-agent'\nmax_threads=5"})
            };
            let provider = crate::provider::Provider::with_id(
                "connection".into(),
                "Connection".into(),
                settings,
                None,
            );
            let mut payload = ProfilePayload::default();
            *payload.models.get_mut(&app).unwrap() = Some("new-main".into());
            *payload.model_settings.get_mut(&app).unwrap() = Some(if app == AppType::Claude {
                BTreeMap::from([
                    ("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), String::new()),
                    ("CLAUDE_CODE_SUBAGENT_MODEL".into(), String::new()),
                ])
            } else {
                BTreeMap::from([
                    ("model_context_window".into(), String::new()),
                    ("agents.default_subagent_model".into(), String::new()),
                    ("agents.max_threads".into(), String::new()),
                ])
            });
            let profile = crate::database::Profile {
                id: format!("clear-{}", app.as_str()),
                name: "Clear overrides".into(),
                payload: serde_json::to_string(&payload).unwrap(),
                sort_order: None,
                project_path: None,
                created_at: None,
                updated_at: None,
            };
            state.db.save_profile(&profile).unwrap();
            state
                .db
                .set_current_profile_id(app.as_str(), Some(&profile.id))
                .unwrap();
            let effective = super::super::ProfileService::effective_provider_for_applied_profile(
                &state, &app, &provider,
            )
            .unwrap();
            if app == AppType::Claude {
                assert!(effective.settings_config["env"]
                    .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                    .is_none());
                assert!(effective.settings_config["env"]
                    .get("CLAUDE_CODE_SUBAGENT_MODEL")
                    .is_none());
                assert_eq!(
                    effective.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"],
                    "keep"
                );
                assert_eq!(
                    effective.settings_config["env"]["ANTHROPIC_MODEL"],
                    "new-main"
                );
                assert_eq!(effective.settings_config["model"], "new-main");
            } else {
                let config: toml::Value =
                    toml::from_str(effective.settings_config["config"].as_str().unwrap()).unwrap();
                assert!(config.get("model_context_window").is_none());
                assert!(config["agents"].get("default_subagent_model").is_none());
                assert!(config["agents"].get("max_threads").is_none());
                assert_eq!(config["model"].as_str(), Some("new-main"));
                assert_eq!(effective.settings_config["auth"]["OPENAI_API_KEY"], "keep");
            }
        }
    }
}
