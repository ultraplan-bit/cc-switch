//! Save reusable provider connections without applying an Agent configuration.
use serde_json::{Map, Value};
use toml_edit::DocumentMut;

use super::ProviderService;
use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::profile::native_config::{
    claude_connection_env, claude_connection_top, codex_connection_top,
};
use crate::store::AppState;

fn merge_settings(app: &AppType, stored: &Value, edited: &Value) -> Result<Value, AppError> {
    let mut result = stored
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("供应商配置必须是对象".into()))?;
    let edited = edited
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("供应商连接必须是对象".into()))?;
    if app == &AppType::Claude {
        result.retain(|key, _| !claude_connection_top(key));
        result.extend(
            edited
                .iter()
                .filter(|(key, _)| claude_connection_top(key))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        let mut env = result
            .get("env")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        env.retain(|key, _| !claude_connection_env(key));
        if let Some(next) = edited.get("env") {
            let next = next
                .as_object()
                .ok_or_else(|| AppError::InvalidInput("供应商 env 必须是对象".into()))?;
            env.extend(
                next.iter()
                    .filter(|(key, _)| claude_connection_env(key))
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        if !env.is_empty() || result.contains_key("env") {
            result.insert("env".into(), Value::Object(env));
        }
    } else {
        let parse = |value: Option<&Value>| -> Result<DocumentMut, AppError> {
            let text = value
                .map(|value| {
                    value.as_str().ok_or_else(|| {
                        AppError::InvalidInput("Codex config 必须是 TOML 字符串".into())
                    })
                })
                .transpose()?
                .unwrap_or("");
            text.parse()
                .map_err(|error| AppError::Config(format!("Codex TOML 无效: {error}")))
        };
        let mut config = parse(result.get("config"))?;
        let edited_config = parse(edited.get("config"))?;
        let keys: Vec<_> = config
            .iter()
            .filter(|(key, _)| codex_connection_top(key))
            .map(|(key, _)| key.to_string())
            .collect();
        for key in keys {
            config.remove(&key);
        }
        for (key, value) in edited_config
            .iter()
            .filter(|(key, _)| codex_connection_top(key))
        {
            config.insert(key, value.clone());
        }
        let auth = edited
            .get("auth")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !auth.is_object() {
            return Err(AppError::InvalidInput("Codex auth 必须是对象".into()));
        }
        result.insert("auth".into(), auth);
        result.insert("config".into(), Value::String(config.to_string()));
    }
    Ok(Value::Object(result))
}

pub(super) fn save(
    state: &AppState,
    app: AppType,
    mut provider: Provider,
    adding: bool,
) -> Result<bool, AppError> {
    if !matches!(app, AppType::Claude | AppType::Codex) {
        return Err(AppError::InvalidInput(
            "此 Agent 尚未支持独立供应商连接".into(),
        ));
    }
    let _profile_guard = futures::executor::block_on(crate::commands::PROFILE_LOCK.lock());
    let _guard = crate::mode::controller::lock_settled_blocking(state, &app)?;
    let stored = state.db.get_provider_by_id(&provider.id, &app)?;
    if adding {
        if stored.is_some() {
            return Err(AppError::InvalidInput("供应商已存在".into()));
        }
        // Keep preset defaults available when the user configures a Profile later.
        ProviderService::prepare_new_provider(&app, &mut provider)?;
    } else {
        let stored = stored.ok_or_else(|| AppError::InvalidInput("供应商不存在".into()))?;
        provider.settings_config =
            merge_settings(&app, &stored.settings_config, &provider.settings_config)?;
        if let Some(meta) = provider.meta.as_mut() {
            meta.stack_models = stored
                .meta
                .as_ref()
                .and_then(|meta| meta.stack_models.clone());
        }
        ProviderService::validate_provider_settings(&app, &provider)?;
        ProviderService::normalize_usage_script_credential_overrides(&app, &mut provider);
    }
    if !adding {
        if let Some(current) = state.db.get_current_profile_id(app.as_str())? {
            if let Some(profile) = state.db.get_profile(&current)? {
                let payload: crate::services::profile::ProfilePayload =
                    serde_json::from_str(&profile.payload)
                        .map_err(|error| AppError::Config(error.to_string()))?;
                let used = match payload.providers.get(&app).and_then(Option::as_ref) {
                    Some(id) => id == &provider.id,
                    None => {
                        crate::mode::current::provider_for(
                            &state.db,
                            &app,
                            crate::mode::current::Purpose::InUse,
                        )?
                        .as_deref()
                            == Some(&provider.id)
                    }
                };
                if used {
                    // The connection it was applied with changed; it stays current
                    // but has to be applied again to take effect.
                    state
                        .db
                        .mark_current_profile_stale(app.as_str(), &current)?;
                }
            }
        }
    }
    state.db.save_provider(&app, &provider)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;

    #[test]
    #[serial]
    fn connection_library_save_never_writes_live_or_switches_provider() {
        let _home = crate::test_support::TestHome::new();
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().unwrap(),
        ));
        let path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let live = "{\"env\":{\"ANTHROPIC_MODEL\":\"live-profile-model\"},\"hooks\":{\"Stop\":[]}}";
        std::fs::write(&path, live).unwrap();
        let provider = Provider::with_id(
            "connection".into(),
            "Connection".into(),
            json!({"env":{"ANTHROPIC_BASE_URL":"https://old.example", "ANTHROPIC_AUTH_TOKEN":"old-key", "ANTHROPIC_MODEL":"preset-model"}}),
            None,
        );
        let editor = || serde_json::from_value(json!({"base":{}, "connectionOnly":true})).unwrap();
        ProviderService::add_from_editor(
            &state,
            AppType::Claude,
            provider.clone(),
            true,
            Some(editor()),
        )
        .unwrap();
        assert!(ProviderService::current(&state, AppType::Claude)
            .unwrap()
            .is_empty());
        let mut changed = provider;
        changed.settings_config["env"]["ANTHROPIC_BASE_URL"] = json!("https://new.example");
        changed.settings_config["env"]["ANTHROPIC_MODEL"] = json!("stale-model");
        ProviderService::update_from_editor(&state, AppType::Claude, None, changed, Some(editor()))
            .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), live);
        let saved = state
            .db
            .get_provider_by_id("connection", &AppType::Claude)
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.settings_config["env"]["ANTHROPIC_BASE_URL"],
            "https://new.example"
        );
        assert_eq!(
            saved.settings_config["env"]["ANTHROPIC_MODEL"],
            "preset-model"
        );
        state
            .db
            .set_current_profile_id("claude", Some("previous-profile"))
            .unwrap();
        assert!(ProviderService::switch(&state, AppType::Claude, "missing").is_err());
        assert_eq!(
            state
                .db
                .get_current_profile_id("claude")
                .unwrap()
                .as_deref(),
            Some("previous-profile")
        );
        // Saving or switching a new connection never creates a Profile.
        assert!(state.db.get_all_profiles().unwrap().is_empty());
        ProviderService::switch(&state, AppType::Claude, "connection").unwrap();
        assert!(state.db.get_all_profiles().unwrap().is_empty());
        assert_eq!(state.db.get_current_profile_id("claude").unwrap(), None);
    }

    #[test]
    #[serial]
    fn adding_providers_leaves_profiles_empty_until_explicit_creation() {
        use crate::services::profile::{ProfileScope, ProfileService, ProfileSource};

        for app in [AppType::Claude, AppType::Codex] {
            let _home = crate::test_support::TestHome::new();
            let state = AppState::new(std::sync::Arc::new(
                crate::database::Database::memory().unwrap(),
            ));
            let scope = ProfileScope::for_app(&app).unwrap();
            let settings = match app {
                AppType::Claude => json!({"env": {
                    "ANTHROPIC_BASE_URL": "https://provider.example",
                    "ANTHROPIC_AUTH_TOKEN": "test-key",
                    "ANTHROPIC_MODEL": "saved-model"
                }}),
                AppType::Codex => json!({
                    "auth": {"OPENAI_API_KEY": "test-key"},
                    "config": "model = \"saved-model\"\nmodel_provider = \"custom\"\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"https://provider.example/v1\"\nwire_api = \"responses\"\n"
                }),
                _ => unreachable!(),
            };
            // Connection-only, full (stack) editor, and add without editor metadata.
            for (index, connection_only) in [Some(true), Some(false), None].into_iter().enumerate()
            {
                let provider = Provider::with_id(
                    format!("provider-{index}"),
                    format!("Provider {index}"),
                    settings.clone(),
                    None,
                );
                let editor = connection_only.map(|connection_only| super::super::EditorSave {
                    connection_only,
                    base: settings.clone(),
                    draft: Some(settings.clone()),
                    on_conflict: Default::default(),
                });
                ProviderService::add_from_editor(
                    &state,
                    app.clone(),
                    provider.clone(),
                    false,
                    editor,
                )
                .unwrap();
                assert!(state
                    .db
                    .get_provider_by_id(&provider.id, &app)
                    .unwrap()
                    .is_some());
                assert!(ProfileService::list(&state).unwrap().is_empty());
                assert!(state.db.get_all_profiles().unwrap().is_empty());
            }
            let profile = ProfileService::create_from(
                &state,
                "My profile",
                scope,
                ProfileSource::Blank,
                crate::database::ProfileTarget::Global,
            )
            .unwrap();
            let profiles = ProfileService::list(&state).unwrap();
            assert_eq!(profiles.len(), 1);
            assert_eq!(profiles[0].id, profile.id);
        }
    }
    #[test]
    fn connection_edits_preserve_claude_models_behavior_and_unknown_fields() {
        let original = json!({"env": {"ANTHROPIC_BASE_URL":"old", "ANTHROPIC_AUTH_TOKEN":"old-key", "ANTHROPIC_MODEL":"profile-model", "DEBUG":"1"}, "hooks":{"Stop":[]}, "future":{"value":42}, "apiKeyHelper":"old-helper"});
        let edited = json!({"env":{"ANTHROPIC_BASE_URL":"new", "ANTHROPIC_API_KEY":"new-key", "ANTHROPIC_MODEL":"stale-model", "DEBUG":"0"}, "hooks":{}, "apiKeyHelper":"new-helper"});
        let merged = merge_settings(&AppType::Claude, &original, &edited).unwrap();
        assert_eq!(merged["env"]["ANTHROPIC_BASE_URL"], "new");
        assert!(merged["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert_eq!(merged["env"]["ANTHROPIC_API_KEY"], "new-key");
        assert_eq!(merged["env"]["ANTHROPIC_MODEL"], "profile-model");
        assert_eq!(merged["env"]["DEBUG"], "1");
        assert_eq!(merged["hooks"], original["hooks"]);
        assert_eq!(merged["future"], original["future"]);
        assert_eq!(merged["apiKeyHelper"], "new-helper");
    }

    #[test]
    fn connection_edits_preserve_codex_models_catalog_behavior_and_comments() {
        let original = json!({"auth":{"OPENAI_API_KEY":"old"}, "config":"# keep behavior\nmodel = \"profile-model\"\nmodel_reasoning_effort = \"high\"\napproval_policy = \"on-request\"\nmodel_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://old.example\"\n", "modelCatalog":{"models":[{"model":"profile-model"}]}});
        let edited = json!({"auth":{"OPENAI_API_KEY":"new"}, "config":"model = \"stale-model\"\nmodel_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://new.example\"\nhttp_headers = {X-Custom = \"value\"}\n"});
        let merged = merge_settings(&AppType::Codex, &original, &edited).unwrap();
        let text = merged["config"].as_str().unwrap();
        let config: toml::Value = text.parse().unwrap();
        assert!(text.contains("# keep behavior"));
        assert_eq!(config["model"].as_str(), Some("profile-model"));
        assert_eq!(config["approval_policy"].as_str(), Some("on-request"));
        assert_eq!(
            config["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://new.example")
        );
        assert_eq!(merged["modelCatalog"], original["modelCatalog"]);
        assert_eq!(merged["auth"]["OPENAI_API_KEY"], "new");
    }
}
