use super::*;

impl ProfileService {
    /// Idempotent migration. Original providers remain intact; no live files or
    /// enabled flags are written. Each provider and the current global snapshot
    /// has its own marker so providers added later are migrated on the next visit.
    pub fn migrate_legacy_agent_profiles(
        state: &AppState,
        scope: ProfileScope,
    ) -> Result<(Vec<Profile>, Vec<String>), AppError> {
        let app = &scope.apps()[0];
        let mut warnings = Vec::new();
        let providers = state.db.get_provider_rows(app)?;
        for invalid in &providers.unreadable {
            warnings.push(format!(
                "供应商 '{}' 配置不可读，未迁移为 Profile",
                invalid.id
            ));
        }
        for provider in providers.readable.values() {
            let marker = format!(
                "profile_migration_v1_provider_{}_{}",
                scope.as_str(),
                provider.id
            );
            if state.db.get_setting(&marker)?.is_some() {
                continue;
            }
            let result = (|| -> Result<(), AppError> {
                let payload = native_config::provider_payload(state, scope, provider)?;
                let now = chrono::Utc::now().timestamp();
                let profile = Profile {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: provider.name.clone(),
                    payload: serde_json::to_string(&payload)
                        .map_err(|error| AppError::Config(error.to_string()))?,
                    sort_order: provider
                        .sort_index
                        .and_then(|value| i64::try_from(value).ok()),
                    project_path: None,
                    created_at: Some(now),
                    updated_at: Some(now),
                };
                state.db.save_migrated_profile(&profile, &marker)?;
                Ok(())
            })();
            if let Err(error) = result {
                warnings.push(format!("迁移供应商 '{}' 失败：{error}", provider.name));
            }
        }
        let marker = format!("profile_migration_v1_current_{}", scope.as_str());
        if state.db.get_setting(&marker)?.is_none() {
            let result = (|| -> Result<(), AppError> {
                let payload = Self::snapshot_current(state, scope)?;
                let now = chrono::Utc::now().timestamp();
                let profile = Profile {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: format!("{} · 当前全局配置", scope.as_str()),
                    payload: serde_json::to_string(&payload)
                        .map_err(|error| AppError::Config(error.to_string()))?,
                    sort_order: None,
                    project_path: None,
                    created_at: Some(now),
                    updated_at: Some(now),
                };
                state.db.save_migrated_profile(&profile, &marker)?;
                Ok(())
            })();
            if let Err(error) = result {
                warnings.push(format!("迁移当前全局配置失败：{error}"));
            }
        }
        // Existing edited Profiles keep every field. Only previously absent
        // native behavior is filled from their own project or referenced provider.
        for mut profile in state.db.get_all_profiles()? {
            let result = (|| -> Result<(), AppError> {
                let previous = profile.payload.clone();
                let mut payload: ProfilePayload = serde_json::from_str(&previous)
                    .map_err(|error| AppError::Config(error.to_string()))?;
                if !payload.scope_captured(scope)
                    || payload.native_config.get(app).is_some_and(Option::is_some)
                {
                    return Ok(());
                }
                let text = if let Some(path) = profile.project_path.as_deref() {
                    crate::services::project_profile::capture_native_config(Path::new(path), app)?
                } else if let Some(provider) = payload
                    .providers
                    .get(app)
                    .and_then(Option::as_ref)
                    .and_then(|id| providers.readable.get(id))
                {
                    native_config::from_provider(app, &provider.settings_config)?
                } else {
                    native_config::normalize(app, "", false)?
                };
                *payload.native_config.get_mut(app).unwrap() = Some(text);
                profile.payload = serde_json::to_string(&payload)
                    .map_err(|error| AppError::Config(error.to_string()))?;
                state
                    .db
                    .backfill_profile_native_config(&profile, &previous)?;
                Ok(())
            })();
            if let Err(error) = result {
                warnings.push(format!("补齐 Profile '{}' 配置失败：{error}", profile.name));
            }
        }
        Ok((state.db.get_all_profiles()?, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::test_support::TempUserHome;
    use serial_test::serial;

    #[test]
    #[serial]
    fn profile_migration_is_idempotent_keeps_sources_and_does_not_mix_global_behavior_into_provider_defaults(
    ) {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let global = r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"private","ANTHROPIC_MODEL":"current"},"globalOnly":{"value":42}}"#;
        std::fs::write(&path, global).unwrap();
        let provider = crate::provider::Provider::with_id(
            "provider".into(),
            "Provider defaults".into(),
            serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"private","ANTHROPIC_MODEL":"saved"},"providerBehavior":{"feature":true}}),
            None,
        );
        state.db.save_provider(&AppType::Claude, &provider).unwrap();
        let (first, warnings) =
            ProfileService::migrate_legacy_agent_profiles(&state, ProfileScope::Claude).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(first.len(), 2);
        let imported = first
            .iter()
            .find(|profile| profile.name == "Provider defaults")
            .unwrap();
        let payload: ProfilePayload = serde_json::from_str(&imported.payload).unwrap();
        assert_eq!(payload.models.claude.as_deref(), Some("saved"));
        let native = payload.native_config.claude.unwrap();
        assert!(native.contains("providerBehavior"));
        assert!(!native.contains("globalOnly"));
        assert!(!native.contains("private"));
        assert_eq!(payload.mcp.claude, Some(vec![]));
        let mut edited = imported.clone();
        let mut payload: ProfilePayload = serde_json::from_str(&edited.payload).unwrap();
        payload.native_config.claude = Some(r#"{"userEdited":true}"#.into());
        edited.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&edited).unwrap();
        let (second, warnings) =
            ProfileService::migrate_legacy_agent_profiles(&state, ProfileScope::Claude).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(second.len(), 2);
        assert_eq!(
            state.db.get_profile(&edited.id).unwrap().unwrap().payload,
            edited.payload
        );
        assert_eq!(
            state
                .db
                .get_provider_by_id("provider", &AppType::Claude)
                .unwrap()
                .unwrap()
                .settings_config,
            provider.settings_config
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), global);
        state.db.delete_profile(&edited.id).unwrap();
        let (third, _) =
            ProfileService::migrate_legacy_agent_profiles(&state, ProfileScope::Claude).unwrap();
        assert_eq!(
            third.len(),
            1,
            "deleted migrated Profiles must not reappear"
        );
        let new_provider = crate::provider::Provider::with_id(
            "later".into(),
            "Later".into(),
            serde_json::json!({"env":{"ANTHROPIC_MODEL":"later"}}),
            None,
        );
        state
            .db
            .save_provider(&AppType::Claude, &new_provider)
            .unwrap();
        let (fourth, _) =
            ProfileService::migrate_legacy_agent_profiles(&state, ProfileScope::Claude).unwrap();
        assert_eq!(fourth.len(), 2);
    }

    #[test]
    #[serial]
    fn partial_migration_keeps_failed_sources_retryable() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let mut provider = crate::provider::Provider::with_id(
            "broken".into(),
            "Broken".into(),
            serde_json::json!({"config":"not valid = ["}),
            None,
        );
        state.db.save_provider(&AppType::Codex, &provider).unwrap();
        let (_, warnings) =
            ProfileService::migrate_legacy_agent_profiles(&state, ProfileScope::Codex).unwrap();
        assert!(!warnings.is_empty());
        assert!(state
            .db
            .get_setting("profile_migration_v1_provider_codex_broken")
            .unwrap()
            .is_none());
        provider.settings_config =
            serde_json::json!({"config":"model='fixed'\n[features]\nfuture=true"});
        state.db.save_provider(&AppType::Codex, &provider).unwrap();
        let (profiles, warnings) =
            ProfileService::migrate_legacy_agent_profiles(&state, ProfileScope::Codex).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(profiles.iter().any(|profile| profile.name == "Broken"));
    }

    #[test]
    #[serial]
    fn global_apply_failure_clears_active_marker_and_never_resnapshots_another_saved_profile() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let previous = Profile {
            id: "previous".into(),
            name: "Previous".into(),
            payload: serde_json::to_string(&ProfilePayload::default()).unwrap(),
            sort_order: None,
            project_path: None,
            created_at: None,
            updated_at: None,
        };
        state.db.save_profile(&previous).unwrap();
        state
            .db
            .set_current_profile_id("claude", Some(&previous.id))
            .unwrap();
        let mut payload = ProfilePayload::default();
        payload.native_config.claude =
            Some(r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"forbidden"}}"#.into());
        let target = Profile {
            id: "target".into(),
            name: "Target".into(),
            payload: serde_json::to_string(&payload).unwrap(),
            ..previous.clone()
        };
        state.db.save_profile(&target).unwrap();
        let warnings = ProfileService::apply(&state, &target.id, ProfileScope::Claude).unwrap();
        assert!(!warnings.is_empty());
        assert_eq!(state.db.get_current_profile_id("claude").unwrap(), None);
        assert_eq!(
            state.db.get_profile(&previous.id).unwrap().unwrap().payload,
            previous.payload
        );
        assert!(!crate::config::get_claude_settings_path().exists());
    }

    #[test]
    #[serial]
    fn codex_global_catalog_can_use_current_connection_without_a_profile_provider_reference() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let path = crate::codex_config::get_codex_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "model_provider='custom'\n[model_providers.custom]\nbase_url='https://keep.test'\n",
        )
        .unwrap();
        let mut payload = ProfilePayload::default();
        payload.models.codex = Some("independent".into());
        payload.model_catalog.codex =
            Some(serde_json::json!({"models":[{"model":"independent","contextWindow":123456}]}));
        let mut warnings = Vec::new();
        ProfileService::apply_saved_model_catalog(&state, &payload, &mut warnings).unwrap();
        assert!(!warnings.iter().any(|warning| warning.contains("没有选择")));
        let config: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            config["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://keep.test")
        );
        assert!(config.get("model_catalog_json").is_some());
        assert!(crate::codex_config::get_codex_model_catalog_path().exists());
        let profile = Profile {
            id: "catalog-applied".into(),
            name: "Catalog applied".into(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            project_path: None,
            created_at: None,
            updated_at: None,
        };
        state.db.save_profile(&profile).unwrap();
        let warnings = ProfileService::apply(&state, &profile.id, ProfileScope::Codex).unwrap();
        assert!(
            warnings.is_empty(),
            "successful local catalog application has no warning: {warnings:?}"
        );
        assert_eq!(
            state.db.get_current_profile_id("codex").unwrap().as_deref(),
            Some(profile.id.as_str())
        );
    }

    #[test]
    #[serial]
    fn independent_profile_catalog_is_rejected_before_touching_an_active_stack() {
        let _home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        let store = crate::live::engine::DeviceStore::for_device();
        crate::mode::state::update(&store, |live| {
            let app = live.apps.entry("codex".into()).or_default();
            app.mode = Some(crate::mode::state::Mode::Proxy);
            app.stack.enabled = true;
            app.stack.members = vec!["one".into(), "two".into()];
        })
        .unwrap();
        let path = crate::codex_config::get_codex_model_catalog_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"{"models":[{"slug":"stack-one"},{"slug":"stack-two"}]}"#;
        std::fs::write(&path, original).unwrap();
        let mut payload = ProfilePayload::default();
        payload.model_catalog.codex = Some(serde_json::json!({"models":[{"model":"private"}]}));
        payload.native_config.codex = Some("[features]\nshould_not_apply=true".into());
        let profile = Profile {
            id: "private".into(),
            name: "Private".into(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            project_path: None,
            created_at: None,
            updated_at: None,
        };
        state.db.save_profile(&profile).unwrap();
        let error = ProfileService::apply(&state, &profile.id, ProfileScope::Codex).unwrap_err();
        assert!(error.to_string().contains("聚合"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!crate::codex_config::get_codex_config_path().exists());
        assert_eq!(
            crate::mode::state::stack(&store, "codex").unwrap().members,
            vec!["one", "two"]
        );
    }
}
