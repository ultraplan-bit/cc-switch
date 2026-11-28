use super::*;

fn state() -> AppState {
    AppState::new(std::sync::Arc::new(
        crate::database::Database::memory().unwrap(),
    ))
}

fn blank(state: &AppState, target: ProfileTarget) -> Profile {
    ProfileService::create_from(
        state,
        "Example",
        ProfileScope::Claude,
        ProfileSource::Blank,
        target,
    )
    .unwrap()
}

fn config() -> ProfileConfig {
    ProfileConfig {
        claude_project_connection: ClaudeProjectConnection::Inherit,
        native_config: None,
        provider_id: None,
        model: "example-model".into(),
        model_settings: Some(BTreeMap::new()),
        model_catalog: None,
        mcp_ids: vec![],
        skill_ids: vec![],
        prompt_id: None,
        follow_global_resources: true,
    }
}

#[test]
#[serial_test::serial]
fn profile_targets_reject_wrong_destinations_without_writing_files() {
    let _home = crate::test_support::TempUserHome::new();
    let state = state();
    let global = blank(&state, ProfileTarget::Global);
    let project = blank(&state, ProfileTarget::Project);
    state
        .db
        .set_current_profile_id("claude", Some(&global.id))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_string_lossy();
    ProfileService::add_project(&state, &path).unwrap();

    let error = ProfileService::apply(&state, &project.id, ProfileScope::Claude).unwrap_err();
    assert!(error.to_string().contains("复制为全局方案"));
    let error = crate::services::project_profile::ProjectProfileService::deploy(
        &state,
        &global.id,
        ProfileScope::Claude,
        &path,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("复制为项目方案"));
    assert_eq!(
        state.db.get_current_profile_id("claude").unwrap(),
        Some(global.id)
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    assert!(!crate::config::get_claude_settings_path().exists());
}

#[test]
#[serial_test::serial]
fn profile_targets_reject_project_desktop_global_capture_and_global_project_connection() {
    let _home = crate::test_support::TempUserHome::new();
    let state = state();
    let project = blank(&state, ProfileTarget::Project);
    let global = blank(&state, ProfileTarget::Global);
    assert!(ProfileService::create_from(
        &state,
        "Desktop",
        ProfileScope::ClaudeDesktop,
        ProfileSource::Blank,
        ProfileTarget::Project,
    )
    .is_err());
    assert!(ProfileService::duplicate(
        &state,
        &global.id,
        "Desktop",
        ProfileScope::ClaudeDesktop,
        ProfileTarget::Project,
    )
    .is_err());
    assert!(ProfileService::update_config(
        &state,
        &project.id,
        ProfileScope::ClaudeDesktop,
        config(),
        Some("Changed".into()),
    )
    .is_err());
    assert!(ProfileService::update(
        &state,
        &project.id,
        Some("Changed".into()),
        true,
        Some(ProfileScope::Claude),
    )
    .is_err());
    assert!(ProfileService::update_config(
        &state,
        &global.id,
        ProfileScope::Claude,
        ProfileConfig {
            claude_project_connection: ClaudeProjectConnection::Direct,
            ..config()
        },
        None,
    )
    .is_err());
    for original in [&project, &global] {
        let saved = state.db.get_profile(&original.id).unwrap().unwrap();
        assert_eq!(saved.name, original.name);
        assert_eq!(saved.payload, original.payload);
        assert_eq!(saved.target, original.target);
    }
    assert_eq!(state.db.get_all_profiles().unwrap().len(), 2);
}

#[test]
#[serial_test::serial]
fn profile_target_copies_are_independent_and_remove_fields_of_the_other_target() {
    let _home = crate::test_support::TempUserHome::new();
    let state = state();
    let mut project = blank(&state, ProfileTarget::Project);
    let mut payload: ProfilePayload = serde_json::from_str(&project.payload).unwrap();
    payload.claude_project_connection = ClaudeProjectConnection::Direct;
    payload.providers.claude = Some("connection".into());
    payload.models.claude = Some("example-model".into());
    payload
        .project_aliases
        .claude
        .mcp
        .insert("library-id".into(), "native-id".into());
    project.payload = serde_json::to_string(&payload).unwrap();
    state.db.save_profile(&project).unwrap();

    let same_target = ProfileService::duplicate(
        &state,
        &project.id,
        "Project copy",
        ProfileScope::Claude,
        ProfileTarget::Project,
    )
    .unwrap();
    let copied: ProfilePayload = serde_json::from_str(&same_target.payload).unwrap();
    assert_eq!(
        copied.claude_project_connection,
        ClaudeProjectConnection::Direct
    );
    assert_eq!(copied.project_aliases, payload.project_aliases);

    let global = ProfileService::duplicate(
        &state,
        &project.id,
        "Global copy",
        ProfileScope::Claude,
        ProfileTarget::Global,
    )
    .unwrap();
    let copied: ProfilePayload = serde_json::from_str(&global.payload).unwrap();
    assert_eq!(global.target, ProfileTarget::Global);
    assert_ne!(global.id, project.id);
    assert_eq!(
        copied.claude_project_connection,
        ClaudeProjectConnection::Inherit
    );
    assert_eq!(copied.project_aliases, PerApp::default());
    assert_eq!(copied.providers.claude, payload.providers.claude);
    assert_eq!(copied.models.claude, payload.models.claude);

    let independent = ProfileService::duplicate(
        &state,
        &global.id,
        "Independent",
        ProfileScope::Claude,
        ProfileTarget::Project,
    )
    .unwrap();
    let copied: ProfilePayload = serde_json::from_str(&independent.payload).unwrap();
    assert_eq!(independent.target, ProfileTarget::Project);
    assert_eq!(
        copied.claude_project_connection,
        ClaudeProjectConnection::Inherit
    );
    ProfileService::update_config(
        &state,
        &independent.id,
        ProfileScope::Claude,
        config(),
        None,
    )
    .unwrap();
    ProfileService::delete(&state, &independent.id, ProfileScope::Claude).unwrap();
    assert_eq!(
        state.db.get_profile(&global.id).unwrap().unwrap().payload,
        global.payload
    );
    assert_eq!(
        state.db.get_profile(&project.id).unwrap().unwrap().payload,
        project.payload
    );
    assert!(state.db.get_profile(&independent.id).unwrap().is_none());
}

#[test]
#[serial_test::serial]
fn project_profile_cannot_supply_the_effective_global_configuration() {
    let _home = crate::test_support::TempUserHome::new();
    let state = state();
    let project = blank(&state, ProfileTarget::Project);
    state
        .db
        .set_current_profile_id("claude", Some(&project.id))
        .unwrap();
    let provider = crate::provider::Provider::with_id(
        "connection".into(),
        "Connection".into(),
        serde_json::json!({"env": {}}),
        None,
    );
    assert!(ProfileService::effective_provider_for_applied_profile(
        &state,
        &AppType::Claude,
        &provider
    )
    .is_err());
    assert_eq!(
        state.db.get_profile(&project.id).unwrap().unwrap().payload,
        project.payload
    );
}
