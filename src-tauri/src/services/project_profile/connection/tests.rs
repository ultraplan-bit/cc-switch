use super::validate_provider;
use crate::app_config::AppType;
use crate::database::{Database, Profile};
use crate::provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta};
use crate::services::profile::{ClaudeProjectConnection, ProfilePayload, ProfileScope};
use crate::services::project_profile::{
    manifest_path, test_projects, ConflictResolution, ProjectDeployResult, CLAUDE_PROJECT_SETTINGS,
};
use crate::store::AppState;
use crate::test_support::TempUserHome;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

fn test_state() -> (TempUserHome, AppState) {
    let home = TempUserHome::new();
    let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
    (home, state)
}

fn provider(state: &AppState, id: &str, token: &str) -> Provider {
    let provider = Provider::with_id(
        id.into(),
        id.into(),
        json!({"env": {
            "ANTHROPIC_BASE_URL": format!("https://{id}.example"),
            "ANTHROPIC_AUTH_TOKEN": token
        }}),
        None,
    );
    state.db.save_provider(&AppType::Claude, &provider).unwrap();
    provider
}

fn profile(
    state: &AppState,
    root: &Path,
    id: &str,
    provider_id: Option<&str>,
    connection: ClaudeProjectConnection,
    model: &str,
) -> Profile {
    let mut payload = ProfilePayload {
        claude_project_connection: connection,
        ..Default::default()
    };
    payload.native_config.claude = Some("{}".into());
    payload.providers.claude = provider_id.map(str::to_string);
    payload.models.claude = Some(model.into());
    payload.model_settings.claude = Some(BTreeMap::new());
    let profile = Profile {
        id: id.into(),
        name: id.into(),
        payload: serde_json::to_string(&payload).unwrap(),
        sort_order: None,
        created_at: None,
        updated_at: None,
    };
    state.db.save_profile(&profile).unwrap();
    test_projects::bind(id, root);
    profile
}

fn deploy(
    state: &AppState,
    id: &str,
    resolution: Option<ConflictResolution>,
) -> ProjectDeployResult {
    test_projects::deploy(state, id, ProfileScope::Claude, resolution).unwrap()
}

fn applied(state: &AppState, id: &str) {
    assert!(matches!(
        deploy(state, id, None),
        ProjectDeployResult::Applied { .. }
    ));
}

fn read_local(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join(CLAUDE_PROJECT_SETTINGS)).unwrap()).unwrap()
}

fn write_local(root: &Path, content: &Value) {
    fs::create_dir_all(root.join(".claude")).unwrap();
    fs::write(
        root.join(CLAUDE_PROJECT_SETTINGS),
        serde_json::to_vec_pretty(content).unwrap(),
    )
    .unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial_test::serial]
fn project_connections_are_isolated_and_manifests_contain_only_hashes() {
    let (_home, state) = test_state();
    let global = provider(&state, "global", "global-secret");
    crate::settings::set_current_provider(&AppType::Claude, Some(&global.id)).unwrap();
    let global_path = crate::config::get_claude_settings_path();
    crate::config::write_json_file(&global_path, &global.settings_config).unwrap();
    let global_before = fs::read(&global_path).unwrap();

    let projects = [
        ("alpha", "alpha-secret", tempfile::tempdir().unwrap()),
        ("beta", "beta-secret", tempfile::tempdir().unwrap()),
    ];
    for (id, token, project) in &projects {
        let selected = provider(&state, id, token);
        profile(
            &state,
            project.path(),
            id,
            Some(&selected.id),
            ClaudeProjectConnection::Direct,
            &format!("{id}-model"),
        );
        applied(&state, id);
        let local = read_local(project.path());
        assert_eq!(
            local["env"]["ANTHROPIC_BASE_URL"],
            format!("https://{id}.example")
        );
        assert_eq!(local["env"]["ANTHROPIC_AUTH_TOKEN"], *token);
        assert_eq!(local["model"], format!("{id}-model"));
        assert_eq!(local["env"]["ANTHROPIC_MODEL"], format!("{id}-model"));
        let manifest = fs::read_to_string(
            manifest_path(&project.path().canonicalize().unwrap(), "claude").unwrap(),
        )
        .unwrap();
        assert!(!manifest.contains(*token));
        assert!(!manifest.contains(&format!("https://{id}.example")));
        let manifest: Value = serde_json::from_str(&manifest).unwrap();
        let hashes = manifest["claudeConnectionHashes"].as_object().unwrap();
        assert!(!hashes.is_empty());
        assert!(hashes.values().all(|hash| hash.as_str().is_some_and(
            |hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        )));
        assert_eq!(fs::read(&global_path).unwrap(), global_before);
    }
    for (id, token, project) in &projects {
        let local = read_local(project.path());
        assert_eq!(local["env"]["ANTHROPIC_AUTH_TOKEN"], *token);
        assert_eq!(local["model"], format!("{id}-model"));
    }
    assert_eq!(
        crate::mode::current::provider_for(
            &state.db,
            &AppType::Claude,
            crate::mode::current::Purpose::InUse,
        )
        .unwrap()
        .as_deref(),
        Some("global")
    );
}

#[test]
#[serial_test::serial]
fn switching_and_rotating_project_connection_then_inheriting_preserves_unmanaged_settings() {
    let (_home, state) = test_state();
    let project = tempfile::tempdir().unwrap();
    provider(&state, "first", "first-secret");
    provider(&state, "second", "second-secret");
    write_local(
        project.path(),
        &json!({
            "env": {"USER_FEATURE": "keep"},
            "permissions": {"allow": ["Read"]}
        }),
    );
    for (id, token) in [("first", "first-secret"), ("second", "second-secret")] {
        profile(
            &state,
            project.path(),
            "changing",
            Some(id),
            ClaudeProjectConnection::Direct,
            &format!("{id}-model"),
        );
        applied(&state, "changing");
        let local = read_local(project.path());
        assert_eq!(
            local["env"]["ANTHROPIC_BASE_URL"],
            format!("https://{id}.example")
        );
        assert_eq!(local["env"]["ANTHROPIC_AUTH_TOKEN"], token);
        assert_eq!(local["model"], format!("{id}-model"));
    }
    provider(&state, "second", "rotated-secret");
    applied(&state, "changing");
    assert_eq!(
        read_local(project.path())["env"]["ANTHROPIC_AUTH_TOKEN"],
        "rotated-secret"
    );
    let mut key_provider = state
        .db
        .get_provider_by_id("second", &AppType::Claude)
        .unwrap()
        .unwrap();
    key_provider.settings_config["env"]
        .as_object_mut()
        .unwrap()
        .remove("ANTHROPIC_AUTH_TOKEN");
    key_provider.settings_config["env"]["ANTHROPIC_API_KEY"] = json!("rotated-api-key");
    state
        .db
        .save_provider(&AppType::Claude, &key_provider)
        .unwrap();
    applied(&state, "changing");
    let switched_auth = read_local(project.path());
    assert_eq!(switched_auth["env"]["ANTHROPIC_AUTH_TOKEN"], "");
    assert_eq!(switched_auth["env"]["ANTHROPIC_API_KEY"], "rotated-api-key");
    profile(
        &state,
        project.path(),
        "changing",
        None,
        ClaudeProjectConnection::Inherit,
        "",
    );
    applied(&state, "changing");
    let local = read_local(project.path());
    assert!(local["env"].get("ANTHROPIC_BASE_URL").is_none());
    assert!(local["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
    assert!(local["env"].get("ANTHROPIC_API_KEY").is_none());
    assert_eq!(local["env"]["USER_FEATURE"], "keep");
    assert_eq!(local["permissions"]["allow"], json!(["Read"]));
}

#[test]
#[serial_test::serial]
fn connection_conflict_keeps_endpoint_credentials_and_model_together() {
    let (_home, state) = test_state();
    let project = tempfile::tempdir().unwrap();
    provider(&state, "first", "first-secret");
    provider(&state, "second", "second-secret");
    profile(
        &state,
        project.path(),
        "changing",
        Some("first"),
        ClaudeProjectConnection::Direct,
        "first-model",
    );
    applied(&state, "changing");
    let mut edited = read_local(project.path());
    edited["env"]["ANTHROPIC_BASE_URL"] = json!("https://handwritten.example");
    write_local(project.path(), &edited);
    let before = fs::read(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap();
    profile(
        &state,
        project.path(),
        "changing",
        Some("second"),
        ClaudeProjectConnection::Direct,
        "second-model",
    );
    assert!(matches!(
        deploy(&state, "changing", None),
        ProjectDeployResult::NeedsResolution { .. }
    ));
    assert_eq!(
        fs::read(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap(),
        before
    );
    assert!(matches!(
        deploy(&state, "changing", Some(ConflictResolution::KeepCurrent)),
        ProjectDeployResult::Applied { .. }
    ));
    let kept = read_local(project.path());
    assert_eq!(
        kept["env"]["ANTHROPIC_BASE_URL"],
        "https://handwritten.example"
    );
    assert_eq!(kept["env"]["ANTHROPIC_AUTH_TOKEN"], "first-secret");
    assert_eq!(kept["model"], "first-model");
    assert_eq!(kept["env"]["ANTHROPIC_MODEL"], "first-model");
    assert!(matches!(
        deploy(&state, "changing", Some(ConflictResolution::Overwrite)),
        ProjectDeployResult::Applied { .. }
    ));
    let replaced = read_local(project.path());
    assert_eq!(
        replaced["env"]["ANTHROPIC_BASE_URL"],
        "https://second.example"
    );
    assert_eq!(replaced["env"]["ANTHROPIC_AUTH_TOKEN"], "second-secret");
    assert_eq!(replaced["model"], "second-model");
    assert_eq!(replaced["env"]["ANTHROPIC_MODEL"], "second-model");
}

#[test]
#[serial_test::serial]
fn matching_handwritten_connection_requires_group_takeover_and_is_removed_when_inheriting() {
    let (_home, state) = test_state();
    provider(&state, "api", "project-secret");
    for (key, value) in [
        ("ANTHROPIC_BASE_URL", "https://api.example"),
        ("ANTHROPIC_AUTH_TOKEN", "project-secret"),
    ] {
        let project = tempfile::tempdir().unwrap();
        let mut initial = json!({"env":{"USER_FEATURE":"keep"}});
        initial["env"][key] = json!(value);
        write_local(project.path(), &initial);
        profile(
            &state,
            project.path(),
            "takeover",
            Some("api"),
            ClaudeProjectConnection::Direct,
            "project-model",
        );
        assert!(matches!(
            deploy(&state, "takeover", None),
            ProjectDeployResult::NeedsResolution { .. }
        ));
        assert_eq!(read_local(project.path()), initial);
        assert!(matches!(
            deploy(&state, "takeover", Some(ConflictResolution::KeepCurrent)),
            ProjectDeployResult::Applied { .. }
        ));
        assert_eq!(read_local(project.path()), initial);
        assert!(matches!(
            deploy(&state, "takeover", Some(ConflictResolution::Overwrite)),
            ProjectDeployResult::Applied { .. }
        ));
        let deployed = read_local(project.path());
        assert_eq!(deployed["env"]["ANTHROPIC_BASE_URL"], "https://api.example");
        assert_eq!(deployed["env"]["ANTHROPIC_AUTH_TOKEN"], "project-secret");
        let manifest: Value = serde_json::from_slice(
            &fs::read(manifest_path(&project.path().canonicalize().unwrap(), "claude").unwrap())
                .unwrap(),
        )
        .unwrap();
        for field in ["env.ANTHROPIC_BASE_URL", "env.ANTHROPIC_AUTH_TOKEN"] {
            assert!(manifest["claudeConnectionHashes"].get(field).is_some());
        }
        profile(
            &state,
            project.path(),
            "takeover",
            None,
            ClaudeProjectConnection::Inherit,
            "",
        );
        applied(&state, "takeover");
        let inherited = read_local(project.path());
        assert!(inherited["env"].get("ANTHROPIC_BASE_URL").is_none());
        assert!(inherited["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert_eq!(inherited["env"]["USER_FEATURE"], "keep");
    }
}

#[test]
#[serial_test::serial]
fn unavailable_or_unsupported_project_connections_fail_before_writing() {
    let (_home, state) = test_state();
    provider(&state, "deleted", "deleted-secret");
    state
        .db
        .delete_provider(&AppType::Claude, "deleted")
        .unwrap();
    let mut unsupported = provider(&state, "unsupported", "unsupported-secret");
    unsupported.meta = Some(ProviderMeta {
        api_format: Some("openai_chat".into()),
        ..Default::default()
    });
    state
        .db
        .save_provider(&AppType::Claude, &unsupported)
        .unwrap();
    for id in ["missing", "deleted", "unsupported"] {
        let project = tempfile::tempdir().unwrap();
        write_local(
            project.path(),
            &json!({"model":"handwritten-model","userField":true}),
        );
        let before = fs::read(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap();
        profile(
            &state,
            project.path(),
            id,
            Some(id),
            ClaudeProjectConnection::Direct,
            "must-not-write",
        );
        assert!(test_projects::deploy(&state, id, ProfileScope::Claude, None).is_err());
        assert_eq!(
            fs::read(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap(),
            before
        );
    }
}

#[test]
#[serial_test::serial]
fn git_tracked_settings_are_rejected_and_untracked_settings_are_ignored() {
    let (_home, state) = test_state();
    provider(&state, "api", "project-secret");
    let tracked = tempfile::tempdir().unwrap();
    git(tracked.path(), &["init", "--quiet"]);
    write_local(tracked.path(), &json!({"userField":true}));
    git(
        tracked.path(),
        &["add", "-f", "--", CLAUDE_PROJECT_SETTINGS],
    );
    let before = fs::read(tracked.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap();
    profile(
        &state,
        tracked.path(),
        "tracked",
        Some("api"),
        ClaudeProjectConnection::Direct,
        "model",
    );
    assert!(test_projects::deploy(&state, "tracked", ProfileScope::Claude, None).is_err());
    assert_eq!(
        fs::read(tracked.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap(),
        before
    );

    let untracked = tempfile::tempdir().unwrap();
    git(untracked.path(), &["init", "--quiet"]);
    profile(
        &state,
        untracked.path(),
        "untracked",
        Some("api"),
        ClaudeProjectConnection::Direct,
        "model",
    );
    applied(&state, "untracked");
    git(
        untracked.path(),
        &["check-ignore", "--quiet", "--", CLAUDE_PROJECT_SETTINGS],
    );
    assert_eq!(
        read_local(untracked.path())["env"]["ANTHROPIC_AUTH_TOKEN"],
        "project-secret"
    );
}

#[test]
#[serial_test::serial]
fn inherit_mode_never_copies_provider_connection_into_project() {
    let (_home, state) = test_state();
    let project = tempfile::tempdir().unwrap();
    let selected = provider(&state, "global", "global-secret");
    crate::settings::set_current_provider(&AppType::Claude, Some(&selected.id)).unwrap();
    profile(
        &state,
        project.path(),
        "inherits",
        Some(&selected.id),
        ClaudeProjectConnection::Inherit,
        "project-model",
    );
    applied(&state, "inherits");
    let local = read_local(project.path());
    assert_eq!(local["model"], "project-model");
    for key in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
    ] {
        assert!(local["env"].get(key).is_none());
    }
    assert!(!local.to_string().contains("global-secret"));
}

#[test]
#[serial_test::serial]
fn direct_provider_validation_accepts_explicit_keys_and_rejects_proxy_only_auth() {
    let (_home, state) = test_state();
    let valid = provider(&state, "api", "real-test-token");
    validate_provider(&valid).unwrap();
    let mut official = valid.clone();
    official.category = Some("official".into());
    official.settings_config = json!({"env": {
        "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
        "ANTHROPIC_API_KEY": "explicit-official-key"
    }});
    validate_provider(&official).unwrap();
    let mut managed = valid.clone();
    managed.meta = Some(ProviderMeta {
        auth_binding: Some(AuthBinding {
            source: AuthBindingSource::ManagedAccount,
            auth_provider: Some("claude".into()),
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut non_anthropic = valid.clone();
    non_anthropic.meta = Some(ProviderMeta {
        api_format: Some("openai_chat".into()),
        ..Default::default()
    });
    let mut both_keys = valid.clone();
    both_keys.settings_config["env"]["ANTHROPIC_API_KEY"] = json!("other-key");
    let mut placeholder = valid.clone();
    placeholder.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"] =
        json!(crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER);
    let mut keyless = official;
    keyless.settings_config = json!({});
    for invalid in [managed, non_anthropic, both_keys, placeholder, keyless] {
        assert!(validate_provider(&invalid).is_err(), "{}", invalid.id);
    }
}

#[test]
fn failed_settings_rollback_keeps_credential_git_protection() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    git(&root, &["init", "--quiet"]);
    let mut undo = Vec::new();
    super::ensure_ignored(&root, &mut undo, &mut Vec::new()).unwrap();
    write_local(
        &root,
        &json!({"env":{"ANTHROPIC_AUTH_TOKEN":"new-test-secret"}}),
    );
    let path = root.join(CLAUDE_PROJECT_SETTINGS);
    undo.push(crate::services::project_profile::ProjectFileUndo {
        applied_hash: Some(crate::live::engine::sha256_hex(&fs::read(&path).unwrap())),
        path: path.clone(),
        original: None,
    });
    // A concurrent user edit prevents restoring/removing the settings file.
    let mut edited = read_local(&root);
    edited["userField"] = json!(true);
    write_local(&root, &edited);
    assert!(!crate::services::project_profile::rollback_project_files(&undo).is_empty());
    assert_eq!(read_local(&root), edited);
    git(
        &root,
        &["check-ignore", "--quiet", "--", CLAUDE_PROJECT_SETTINGS],
    );
}

#[cfg(windows)]
#[test]
#[serial_test::serial]
fn later_deployment_failure_rolls_back_connection_ignore_file_and_manifest() {
    use std::os::windows::fs::OpenOptionsExt;

    let (_home, state) = test_state();
    provider(&state, "api", "project-secret");
    for original_ignore in [None, Some("# User rules\ncache/\n")] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let mut instructions = crate::prompt::Prompt {
            id: "project-instructions".into(),
            name: "Instructions".into(),
            content: "original instructions".into(),
            description: None,
            enabled: false,
            created_at: None,
            updated_at: None,
        };
        state
            .db
            .save_prompt(&AppType::Claude, &instructions)
            .unwrap();
        let mut initial = profile(
            &state,
            &root,
            "rollback",
            None,
            ClaudeProjectConnection::Inherit,
            "original-model",
        );
        let mut payload: ProfilePayload = serde_json::from_str(&initial.payload).unwrap();
        payload.prompts.claude = Some(instructions.id.clone());
        initial.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&initial).unwrap();
        applied(&state, &initial.id);

        let ignore_path = root.join(".claude/.gitignore");
        if let Some(text) = original_ignore {
            fs::write(&ignore_path, text).unwrap();
        }
        let settings_path = root.join(CLAUDE_PROJECT_SETTINGS);
        let manifest_path = manifest_path(&root, "claude").unwrap();
        let original_settings = fs::read(&settings_path).unwrap();
        let original_manifest = fs::read(&manifest_path).unwrap();
        payload.claude_project_connection = ClaudeProjectConnection::Direct;
        payload.providers.claude = Some("api".into());
        payload.models.claude = Some("new-model".into());
        initial.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&initial).unwrap();
        instructions.content = "updated instructions".into();
        state
            .db
            .save_prompt(&AppType::Claude, &instructions)
            .unwrap();

        // Preflight may read this file, but replacement after the settings write must fail.
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(root.join("CLAUDE.md"))
            .unwrap();
        assert!(test_projects::deploy(&state, &initial.id, ProfileScope::Claude, None).is_err());
        assert_eq!(fs::read(&settings_path).unwrap(), original_settings);
        assert_eq!(fs::read(&manifest_path).unwrap(), original_manifest);
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "original instructions"
        );
        match original_ignore {
            Some(text) => assert_eq!(fs::read_to_string(&ignore_path).unwrap(), text),
            None => assert!(!ignore_path.exists()),
        }
        drop(locked);

        applied(&state, &initial.id);
        assert_eq!(
            read_local(&root)["env"]["ANTHROPIC_AUTH_TOKEN"],
            "project-secret"
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "updated instructions"
        );
    }
}

#[test]
#[serial_test::serial]
fn direct_connections_reject_aggregated_main_and_role_models_before_writing() {
    let (_home, state) = test_state();
    provider(&state, "api", "project-secret");
    for role in [
        None,
        Some("ANTHROPIC_DEFAULT_SONNET_MODEL"),
        Some("CLAUDE_CODE_SUBAGENT_MODEL"),
    ] {
        let project = tempfile::tempdir().unwrap();
        write_local(project.path(), &json!({"userField":true}));
        let before = fs::read(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap();
        let mut candidate = profile(
            &state,
            project.path(),
            "aggregate-model",
            Some("api"),
            ClaudeProjectConnection::Direct,
            if role.is_none() {
                "ccs-claude-member--remote-model"
            } else {
                "plain-model"
            },
        );
        if let Some(role) = role {
            let mut payload: ProfilePayload = serde_json::from_str(&candidate.payload).unwrap();
            payload
                .model_settings
                .claude
                .as_mut()
                .unwrap()
                .insert(role.into(), "ccs-claude-member--remote-model".into());
            candidate.payload = serde_json::to_string(&payload).unwrap();
            state.db.save_profile(&candidate).unwrap();
        }
        assert!(test_projects::deploy(&state, &candidate.id, ProfileScope::Claude, None).is_err());
        assert_eq!(
            fs::read(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap(),
            before
        );
        assert!(!project.path().join(".claude/.gitignore").exists());
        assert!(
            !manifest_path(&project.path().canonicalize().unwrap(), "claude")
                .unwrap()
                .exists()
        );
    }
}

#[test]
#[serial_test::serial]
fn returning_to_global_connection_clears_models_owned_by_the_previous_project_provider() {
    let (_home, state) = test_state();
    let project = tempfile::tempdir().unwrap();
    provider(&state, "project-api", "project-secret");
    provider(&state, "global", "global-secret");
    crate::settings::set_current_provider(&AppType::Claude, Some("global")).unwrap();
    profile(
        &state,
        project.path(),
        "returning",
        Some("project-api"),
        ClaudeProjectConnection::Direct,
        "project-model",
    );
    applied(&state, "returning");
    profile(
        &state,
        project.path(),
        "returning",
        Some("project-api"),
        ClaudeProjectConnection::Inherit,
        "project-model",
    );
    applied(&state, "returning");
    let local = read_local(project.path());
    assert!(local.get("model").is_none());
    for key in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ] {
        assert!(
            local["env"].get(key).is_none(),
            "{key} must not follow another provider"
        );
    }
}

#[test]
#[serial_test::serial]
fn compatibility_defaults_follow_native_ownership_and_profile_overrides_preserve_user_edits() {
    let (_home, state) = test_state();
    let project = tempfile::tempdir().unwrap();
    let flag = "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS";
    let mut selected = provider(&state, "compatible", "project-secret");
    selected.settings_config["env"][flag] = json!("1");
    state.db.save_provider(&AppType::Claude, &selected).unwrap();
    write_local(project.path(), &json!({"env":{"ENABLE_TOOL_SEARCH":"1"}}));
    let mut candidate = profile(
        &state,
        project.path(),
        "compatibility",
        Some(&selected.id),
        ClaudeProjectConnection::Direct,
        "project-model",
    );
    applied(&state, &candidate.id);
    assert_eq!(read_local(project.path())["env"][flag], "1");
    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest_path(&project.path().canonicalize().unwrap(), "claude").unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(manifest["nativeConfigHashes"]
        .get(serde_json::to_string(&["env", flag]).unwrap())
        .is_some());
    assert!(manifest["claudeConnectionHashes"]
        .get(format!("env.{flag}"))
        .is_none());

    let mut payload: ProfilePayload = serde_json::from_str(&candidate.payload).unwrap();
    payload.native_config.claude = Some(json!({"env":{flag:"0"}}).to_string());
    candidate.payload = serde_json::to_string(&payload).unwrap();
    state.db.save_profile(&candidate).unwrap();
    applied(&state, &candidate.id);
    assert_eq!(read_local(project.path())["env"][flag], "0");

    // Dropping a managed provider default removes it, retaining other native user settings.
    selected.settings_config["env"]
        .as_object_mut()
        .unwrap()
        .remove(flag);
    state.db.save_provider(&AppType::Claude, &selected).unwrap();
    payload.native_config.claude = Some("{}".into());
    candidate.payload = serde_json::to_string(&payload).unwrap();
    state.db.save_profile(&candidate).unwrap();
    applied(&state, &candidate.id);
    let local = read_local(project.path());
    assert!(local["env"].get(flag).is_none());
    assert_eq!(local["env"]["ENABLE_TOOL_SEARCH"], "1");

    selected.settings_config["env"][flag] = json!("1");
    state.db.save_provider(&AppType::Claude, &selected).unwrap();
    applied(&state, &candidate.id);
    let mut edited = read_local(project.path());
    edited["env"][flag] = json!("0");
    write_local(project.path(), &edited);
    profile(
        &state,
        project.path(),
        &candidate.id,
        None,
        ClaudeProjectConnection::Inherit,
        "",
    );
    assert!(matches!(
        deploy(&state, &candidate.id, None),
        ProjectDeployResult::NeedsResolution { .. }
    ));
    assert!(matches!(
        deploy(&state, &candidate.id, Some(ConflictResolution::KeepCurrent)),
        ProjectDeployResult::Applied { .. }
    ));
    let inherited = read_local(project.path());
    assert_eq!(inherited["env"][flag], "0");
    assert_eq!(inherited["env"]["ENABLE_TOOL_SEARCH"], "1");
    assert!(inherited["env"].get("ANTHROPIC_BASE_URL").is_none());
    assert!(inherited["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
}

#[test]
#[serial_test::serial]
fn case_variant_connection_keys_are_rejected_in_all_configuration_layers_before_writing() {
    for key in ["anthropic_base_url", "claude_code_use_bedrock"] {
        for source in ["provider", "local", "native", "global", "shared"] {
            let (_home, state) = test_state();
            let project = tempfile::tempdir().unwrap();
            let mut selected = provider(&state, "api", "project-secret");
            let mut local = json!({"env":{"USER_FEATURE":"keep"}});
            let mut inherited = json!({});
            let mut shared = json!({});
            let mut native = json!({});
            let variant = json!({"env":{key:"invalid-connection-variant"}});
            match source {
                "provider" => {
                    selected.settings_config["env"][key] = variant["env"][key].clone();
                    state.db.save_provider(&AppType::Claude, &selected).unwrap();
                    assert!(validate_provider(&selected).is_err());
                }
                "local" => local["env"][key] = variant["env"][key].clone(),
                "native" => native = variant,
                "global" => inherited = variant,
                "shared" => shared = variant,
                _ => unreachable!(),
            }
            write_local(project.path(), &local);
            let global_path = crate::config::get_claude_settings_path();
            crate::config::write_json_file(&global_path, &inherited).unwrap();
            let shared_path = project.path().join(".claude/settings.json");
            fs::write(&shared_path, serde_json::to_vec_pretty(&shared).unwrap()).unwrap();
            let originals = [
                (
                    project.path().join(CLAUDE_PROJECT_SETTINGS),
                    fs::read(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap(),
                ),
                (global_path.clone(), fs::read(&global_path).unwrap()),
                (shared_path.clone(), fs::read(&shared_path).unwrap()),
            ];
            let mut candidate = profile(
                &state,
                project.path(),
                "invalid-case",
                Some(&selected.id),
                ClaudeProjectConnection::Direct,
                "model",
            );
            let mut payload: ProfilePayload = serde_json::from_str(&candidate.payload).unwrap();
            payload.native_config.claude = Some(native.to_string());
            candidate.payload = serde_json::to_string(&payload).unwrap();
            state.db.save_profile(&candidate).unwrap();
            assert!(
                test_projects::deploy(&state, &candidate.id, ProfileScope::Claude, None).is_err(),
                "{source} {key}"
            );
            for (path, bytes) in originals {
                assert_eq!(fs::read(path).unwrap(), bytes, "{source} {key}");
            }
            assert!(!project.path().join(".claude/.gitignore").exists());
            assert!(
                !manifest_path(&project.path().canonicalize().unwrap(), "claude")
                    .unwrap()
                    .exists()
            );
        }
    }
}
