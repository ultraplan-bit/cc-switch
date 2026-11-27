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
        // Preset model/behavior defaults stay available for the idempotent Profile migration.
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
    if adding {
        // A connection is usable right away through its own default Profile.
        if let Some(scope) = crate::services::profile::ProfileScope::for_app(&app) {
            if let Err(error) = crate::services::profile::ProfileService::ensure_default_profile(
                state, scope, &provider,
            ) {
                state.db.delete_provider(&app, &provider.id)?;
                return Err(error);
            }
        }
    }
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
        // Switching follows the default Profile created together with the connection.
        ProviderService::switch(&state, AppType::Claude, "connection").unwrap();
        let default_profile = state
            .db
            .get_setting(&crate::services::profile::default_profile_key(
                crate::services::profile::ProfileScope::Claude,
                "connection",
            ))
            .unwrap()
            .expect("a new connection records its default Profile");
        assert_eq!(
            state.db.get_current_profile_id("claude").unwrap(),
            Some(default_profile)
        );
    }

    #[test]
    #[serial]
    fn a_new_connection_gets_a_default_profile_that_switching_follows() {
        use crate::services::profile::{
            default_profile_key, ProfileConfig, ProfilePayload, ProfileScope, ProfileService,
        };
        let _home = crate::test_support::TestHome::new();
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().unwrap(),
        ));
        let path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"env":{"KEEP":"1"}}"#).unwrap();
        let mut server = crate::app_config::McpServer {
            id: "enabled".into(),
            name: "Enabled".into(),
            server: json!({"command":"echo"}),
            apps: crate::app_config::McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        };
        server.apps.claude = true;
        state.db.save_mcp_server(&server).unwrap();
        let editor = || serde_json::from_value(json!({"base":{}, "connectionOnly":true})).unwrap();
        let config = |provider: &str, model: &str| ProfileConfig {
            native_config: None,
            provider_id: Some(provider.into()),
            model: model.into(),
            model_settings: Some(Default::default()),
            model_catalog: None,
            mcp_ids: vec![],
            skill_ids: vec![],
            prompt_id: None,
            follow_global_resources: true,
        };
        let default_of = |id: &str| {
            state
                .db
                .get_setting(&default_profile_key(ProfileScope::Claude, id))
                .unwrap()
                .expect("default Profile is recorded for the new connection")
        };
        for (id, model) in [("first", "first-model"), ("second", "second-model")] {
            let provider = Provider::with_id(
                id.into(),
                id.into(),
                json!({"env":{"ANTHROPIC_BASE_URL":format!("https://{id}.example"),"ANTHROPIC_AUTH_TOKEN":"key"}}),
                None,
            );
            ProviderService::add_from_editor(
                &state,
                AppType::Claude,
                provider,
                true,
                Some(editor()),
            )
            .unwrap();
            let profile = state.db.get_profile(&default_of(id)).unwrap().unwrap();
            assert_eq!(profile.name, id);
            let payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
            assert_eq!(payload.providers.claude.as_deref(), Some(id));
            // Default Profiles follow global resources instead of freezing them.
            assert_eq!(payload.mcp.claude, None);
            assert_eq!(payload.skills.claude, None);
            assert_eq!(payload.prompts.claude, None);
            ProfileService::update_config(
                &state,
                &profile.id,
                ProfileScope::Claude,
                config(id, model),
                None,
            )
            .unwrap();
        }

        let result = ProviderService::switch(&state, AppType::Claude, "first").unwrap();
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(
            state.db.get_applied_profile_id("claude").unwrap(),
            Some(default_of("first"))
        );
        let live: Value = crate::config::read_json_file(&path).unwrap();
        assert_eq!(live["env"]["ANTHROPIC_BASE_URL"], "https://first.example");
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "first-model");
        assert_eq!(live["env"]["KEEP"], "1");

        ProviderService::switch(&state, AppType::Claude, "second").unwrap();
        assert_eq!(
            state.db.get_applied_profile_id("claude").unwrap(),
            Some(default_of("second"))
        );
        let live: Value = crate::config::read_json_file(&path).unwrap();
        assert_eq!(live["env"]["ANTHROPIC_BASE_URL"], "https://second.example");
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "second-model");
        assert!(
            state.db.get_all_mcp_servers().unwrap()["enabled"]
                .apps
                .claude,
            "following a connection never touches global resource enablement"
        );

        // Editing the connection of the current Profile keeps it current but pending.
        let mut edited = state
            .db
            .get_provider_by_id("second", &AppType::Claude)
            .unwrap()
            .unwrap();
        edited.settings_config["env"]["ANTHROPIC_BASE_URL"] = json!("https://moved.example");
        ProviderService::update_from_editor(&state, AppType::Claude, None, edited, Some(editor()))
            .unwrap();
        assert_eq!(
            state.db.get_current_profile_id("claude").unwrap(),
            Some(default_of("second"))
        );
        assert_eq!(state.db.get_applied_profile_id("claude").unwrap(), None);

        // A default Profile repointed at another connection is no longer followed.
        ProfileService::update_config(
            &state,
            &default_of("first"),
            ProfileScope::Claude,
            config("second", "elsewhere"),
            None,
        )
        .unwrap();
        ProviderService::switch(&state, AppType::Claude, "first").unwrap();
        assert_eq!(state.db.get_current_profile_id("claude").unwrap(), None);

        // Explicitly applying a Profile switches its connection without the follow-up.
        let custom = ProfileService::duplicate(
            &state,
            &default_of("second"),
            "Custom",
            ProfileScope::Claude,
        )
        .unwrap();
        ProfileService::update_config(
            &state,
            &custom.id,
            ProfileScope::Claude,
            config("second", "custom-model"),
            None,
        )
        .unwrap();
        let warnings = ProfileService::apply(&state, &custom.id, ProfileScope::Claude).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            state.db.get_applied_profile_id("claude").unwrap(),
            Some(custom.id.clone())
        );
        let live: Value = crate::config::read_json_file(&path).unwrap();
        assert_eq!(live["env"]["ANTHROPIC_BASE_URL"], "https://moved.example");
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "custom-model");
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
