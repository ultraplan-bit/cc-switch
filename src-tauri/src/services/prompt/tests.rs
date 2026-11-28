use super::*;
use crate::{database::Database, test_support::TestHome};
use serial_test::serial;
use std::sync::Arc;

fn state() -> AppState {
    AppState::new(Arc::new(Database::memory().unwrap()))
}

fn prompt(id: &str, content: &str) -> Prompt {
    Prompt {
        id: id.into(),
        name: id.into(),
        content: content.into(),
        description: None,
        created_at: Some(1),
        updated_at: Some(1),
    }
}

#[test]
#[serial]
fn shared_library_distributes_only_on_request_to_agent_native_filenames() {
    let home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "shared", prompt("shared", "# Shared rules")).unwrap();
    assert!(!home.path().join(".claude/CLAUDE.md").exists());
    assert!(!home.path().join(".codex/AGENTS.md").exists());
    for app in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        let target = PromptService::get_target(&state, app.clone()).unwrap();
        let applied =
            PromptService::apply_prompt(&state, app, "shared", &target.revision, "# Shared rules")
                .unwrap();
        assert!(applied.managed && applied.matches_library);
        assert_eq!(applied.content.as_deref(), Some("# Shared rules"));
    }
    for path in [".claude/CLAUDE.md", ".codex/AGENTS.md", ".gemini/GEMINI.md"] {
        assert_eq!(
            std::fs::read_to_string(home.path().join(path)).unwrap(),
            "# Shared rules"
        );
    }
    assert!(!home.path().join(".claude/AGENTS.md").exists());
    assert!(!home.path().join(".codex/CLAUDE.md").exists());
    PromptService::upsert_prompt(&state, "shared", prompt("shared", "# Updated library")).unwrap();
    let target = PromptService::get_target(&state, AppType::Claude).unwrap();
    assert!(target.managed && !target.matches_library);
    assert_eq!(target.content.as_deref(), Some("# Shared rules"));
    PromptService::delete_prompt(&state, "shared").unwrap();
    assert!(PromptService::get_prompts(&state).unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(home.path().join(".codex/AGENTS.md")).unwrap(),
        "# Shared rules"
    );
    PromptService::remove_prompt_application(&state, AppType::Claude, &target.revision).unwrap();
    assert_eq!(
        std::fs::read_to_string(home.path().join(".claude/CLAUDE.md")).unwrap(),
        ""
    );
}

#[test]
#[serial]
fn external_edits_are_never_backfilled_or_cleared_and_stale_previews_fail() {
    let _home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "shared", prompt("shared", "saved")).unwrap();
    PromptService::enable_prompt(&state, AppType::Claude, "shared").unwrap();
    let before = PromptService::get_target(&state, AppType::Claude).unwrap();
    std::fs::write(&before.path, "external edit").unwrap();
    assert_eq!(
        PromptService::get_prompts(&state).unwrap()["shared"].content,
        "saved"
    );
    assert!(PromptService::apply_prompt(
        &state,
        AppType::Claude,
        "shared",
        &before.revision,
        "saved"
    )
    .is_err());
    assert!(PromptService::enable_prompt(&state, AppType::Claude, "shared").is_err());
    assert!(PromptService::disable_prompt(&state, AppType::Claude).is_err());
    let latest = PromptService::get_target(&state, AppType::Claude).unwrap();
    assert!(!latest.managed && !latest.matches_library);
    assert!(
        PromptService::remove_prompt_application(&state, AppType::Claude, &latest.revision)
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(&latest.path).unwrap(),
        "external edit"
    );
    let imported = PromptService::import_from_file(&state, AppType::Claude).unwrap();
    assert_ne!(imported, "shared");
    assert_eq!(
        PromptService::import_from_file(&state, AppType::Claude).unwrap(),
        imported
    );
    assert_eq!(
        PromptService::get_prompts(&state).unwrap()["shared"].content,
        "saved"
    );
    PromptService::apply_prompt(&state, AppType::Claude, "shared", &latest.revision, "saved")
        .unwrap();
    assert_eq!(std::fs::read_to_string(&latest.path).unwrap(), "saved");
}

#[test]
#[serial]
fn invalid_ids_and_unreadable_files_do_not_change_library_or_files() {
    let _home = TestHome::new();
    let state = state();
    assert!(PromptService::upsert_prompt(&state, "a", prompt("b", "rules")).is_err());
    assert!(PromptService::get_prompts(&state).unwrap().is_empty());
    let path = prompt_file_path(&AppType::Claude).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    PromptService::upsert_prompt(&state, "shared", prompt("shared", "saved")).unwrap();
    assert_eq!(PromptService::get_prompts(&state).unwrap().len(), 1);
    assert!(PromptService::get_target(&state, AppType::Claude).is_err());
    assert!(PromptService::import_from_file(&state, AppType::Claude).is_err());
    assert!(path.is_dir());
}

#[test]
#[serial]
fn failed_application_commit_restores_previous_file_and_application() {
    let _home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "a", prompt("a", "first")).unwrap();
    PromptService::upsert_prompt(&state, "b", prompt("b", "second")).unwrap();
    PromptService::enable_prompt(&state, AppType::Claude, "a").unwrap();
    let before = PromptService::get_target(&state, AppType::Claude).unwrap();
    state.db.conn.lock().unwrap().execute_batch(
        "CREATE TRIGGER fail_application BEFORE UPDATE ON prompt_applications BEGIN SELECT RAISE(ABORT, 'test failure'); END;
         CREATE TRIGGER fail_application_insert BEFORE INSERT ON prompt_applications BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
    ).unwrap();
    assert!(
        PromptService::apply_prompt(&state, AppType::Claude, "b", &before.revision, "second")
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&before.path).unwrap(), "first");
    let restored = PromptService::get_target(&state, AppType::Claude).unwrap();
    assert_eq!(restored.applied_prompt_id.as_deref(), Some("a"));
    assert!(restored.matches_library);
    let missing = PromptService::get_target(&state, AppType::Codex).unwrap();
    assert!(
        PromptService::apply_prompt(&state, AppType::Codex, "b", &missing.revision, "second")
            .is_err()
    );
    assert!(!Path::new(&missing.path).exists());
}

#[test]
#[serial]
fn failed_removal_commit_keeps_managed_contents_and_selection() {
    let _home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "a", prompt("a", "first")).unwrap();
    PromptService::enable_prompt(&state, AppType::Claude, "a").unwrap();
    let before = PromptService::get_target(&state, AppType::Claude).unwrap();
    state.db.conn.lock().unwrap().execute_batch(
        "CREATE TRIGGER fail_removal BEFORE DELETE ON prompt_applications BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
    ).unwrap();
    assert!(
        PromptService::remove_prompt_application(&state, AppType::Claude, &before.revision)
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&before.path).unwrap(), "first");
    assert!(
        PromptService::get_target(&state, AppType::Claude)
            .unwrap()
            .matches_library
    );
}

#[test]
#[serial]
fn minimax_limit_is_checked_on_distribution_not_library_storage() {
    let _home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "large", prompt("large", &"x".repeat(32769))).unwrap();
    assert!(PromptService::enable_prompt(&state, AppType::Mcode, "large").is_err());
    assert!(!prompt_file_path(&AppType::Mcode).unwrap().exists());
    PromptService::enable_prompt(&state, AppType::Claude, "large").unwrap();
}

#[test]
#[serial]
fn pi_uses_shared_library_with_native_guard_and_no_implicit_activation() {
    let _home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "a", prompt("a", "rules")).unwrap();
    let initial = PromptService::get_target(&state, AppType::Pi).unwrap();
    let applied =
        PromptService::apply_prompt(&state, AppType::Pi, "a", &initial.revision, "rules").unwrap();
    assert!(applied.matches_library);
    assert!(applied.path.ends_with("AGENTS.md"));
    std::fs::write(&applied.path, "native edit").unwrap();
    assert_eq!(
        PromptService::get_prompts(&state).unwrap()["a"].content,
        "rules"
    );
    assert!(PromptService::disable_prompt(&state, AppType::Pi).is_err());
    let latest = PromptService::get_target(&state, AppType::Pi).unwrap();
    PromptService::apply_prompt(&state, AppType::Pi, "a", &latest.revision, "rules").unwrap();
    PromptService::disable_prompt(&state, AppType::Pi).unwrap();
    assert_eq!(std::fs::read_to_string(&applied.path).unwrap(), "");
}

#[test]
fn revision_binds_destination_even_when_both_files_are_missing() {
    assert_ne!(
        target_revision(Path::new("first/CLAUDE.md"), None).unwrap(),
        target_revision(Path::new("second/CLAUDE.md"), None).unwrap()
    );
}

#[test]
#[serial]
fn profile_application_does_not_claim_matching_handwritten_instructions() {
    let _home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "a", prompt("a", "shared rules")).unwrap();
    let path = prompt_file_path(&AppType::Claude).unwrap();
    write_text_file(&path, "shared rules").unwrap();
    PromptService::enable_prompt(&state, AppType::Claude, "a").unwrap();
    assert!(state
        .db
        .get_prompt_application(&AppType::Claude)
        .unwrap()
        .is_none());
    PromptService::disable_prompt(&state, AppType::Claude).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "shared rules");
}

#[test]
#[serial]
fn changing_library_content_invalidates_distribution_preview() {
    let _home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "a", prompt("a", "reviewed")).unwrap();
    let preview = PromptService::get_target(&state, AppType::Claude).unwrap();
    PromptService::upsert_prompt(&state, "a", prompt("a", "updated elsewhere")).unwrap();
    assert!(PromptService::apply_prompt(
        &state,
        AppType::Claude,
        "a",
        &preview.revision,
        "reviewed"
    )
    .is_err());
    assert!(!Path::new(&preview.path).exists());
    assert!(state
        .db
        .get_prompt_application(&AppType::Claude)
        .unwrap()
        .is_none());
}

#[test]
#[serial]
fn changing_agent_directory_invalidates_distribution_preview_and_ownership() {
    let home = TestHome::new();
    let state = state();
    PromptService::upsert_prompt(&state, "a", prompt("a", "rules")).unwrap();
    let old = PromptService::get_target(&state, AppType::Claude).unwrap();
    let new_dir = home.path().join("other-claude");
    crate::settings::update_settings(crate::settings::AppSettings {
        claude_config_dir: Some(new_dir.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        PromptService::apply_prompt(&state, AppType::Claude, "a", &old.revision, "rules").is_err()
    );
    assert!(!new_dir.join("CLAUDE.md").exists());
    assert!(!Path::new(&old.path).exists());
    let current = PromptService::get_target(&state, AppType::Claude).unwrap();
    PromptService::apply_prompt(&state, AppType::Claude, "a", &current.revision, "rules").unwrap();
    crate::settings::update_settings(crate::settings::AppSettings::default()).unwrap();
    assert!(PromptService::get_target(&state, AppType::Claude)
        .unwrap()
        .applied_prompt_id
        .is_none());
    PromptService::disable_prompt(&state, AppType::Claude).unwrap();
    assert_eq!(
        std::fs::read_to_string(new_dir.join("CLAUDE.md")).unwrap(),
        "rules"
    );
}

#[test]
fn target_identity_stays_stable_when_missing_directories_and_file_are_created() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("new-agent/config/CLAUDE.md");
    let missing_identity = target_identity(&path).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    assert_eq!(target_identity(&path).unwrap(), missing_identity);
    std::fs::write(&path, "rules").unwrap();
    assert_eq!(target_identity(&path).unwrap(), missing_identity);
}

#[cfg(any(unix, windows))]
fn link_agent_directory(target: &Path, link: &Path) {
    crate::agy_config::create_dir_link(target, link).unwrap();
}

#[cfg(any(unix, windows))]
fn unlink_agent_directory(link: &Path) {
    #[cfg(unix)]
    std::fs::remove_file(link).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(link).unwrap();
}

#[cfg(any(unix, windows))]
#[test]
#[serial]
fn parent_directory_link_changes_invalidate_missing_target_previews() {
    let home = TestHome::new();
    let state = state();
    let first = home.path().join("first-agent");
    let second = home.path().join("second-agent");
    let link = home.path().join("agent-link");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    link_agent_directory(&first, &link);
    crate::settings::update_settings(crate::settings::AppSettings {
        claude_config_dir: Some(link.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap();
    PromptService::upsert_prompt(&state, "rules", prompt("rules", "shared rules")).unwrap();
    let preview = PromptService::get_target(&state, AppType::Claude).unwrap();
    unlink_agent_directory(&link);
    link_agent_directory(&second, &link);
    let current = PromptService::get_target(&state, AppType::Claude).unwrap();
    assert_ne!(preview.revision, current.revision);
    assert!(PromptService::apply_prompt(
        &state,
        AppType::Claude,
        "rules",
        &preview.revision,
        "shared rules"
    )
    .is_err());
    assert!(!first.join("CLAUDE.md").exists());
    assert!(!second.join("CLAUDE.md").exists());
    let applied = PromptService::apply_prompt(
        &state,
        AppType::Claude,
        "rules",
        &current.revision,
        "shared rules",
    )
    .unwrap();
    assert!(applied.managed && applied.matches_library);
    assert_eq!(
        std::fs::read_to_string(second.join("CLAUDE.md")).unwrap(),
        "shared rules"
    );
    unlink_agent_directory(&link);
}

#[cfg(any(unix, windows))]
#[test]
#[serial]
fn parent_directory_link_changes_do_not_transfer_ownership_to_identical_files() {
    let home = TestHome::new();
    let state = state();
    let first = home.path().join("first-agent");
    let second = home.path().join("second-agent");
    let link = home.path().join("agent-link");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    link_agent_directory(&first, &link);
    crate::settings::update_settings(crate::settings::AppSettings {
        claude_config_dir: Some(link.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap();
    PromptService::upsert_prompt(&state, "rules", prompt("rules", "shared rules")).unwrap();
    PromptService::enable_prompt(&state, AppType::Claude, "rules").unwrap();
    let applied = PromptService::get_target(&state, AppType::Claude).unwrap();
    std::fs::write(second.join("CLAUDE.md"), "shared rules").unwrap();
    unlink_agent_directory(&link);
    link_agent_directory(&second, &link);
    let redirected = PromptService::get_target(&state, AppType::Claude).unwrap();
    assert_ne!(applied.revision, redirected.revision);
    assert!(!redirected.managed);
    assert!(redirected.applied_prompt_id.is_none());
    assert!(
        PromptService::remove_prompt_application(&state, AppType::Claude, &applied.revision)
            .is_err()
    );
    assert!(PromptService::remove_prompt_application(
        &state,
        AppType::Claude,
        &redirected.revision
    )
    .is_err());
    PromptService::disable_prompt(&state, AppType::Claude).unwrap();
    for root in [&first, &second] {
        assert_eq!(
            std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "shared rules"
        );
    }
    unlink_agent_directory(&link);
    link_agent_directory(&first, &link);
    assert!(
        PromptService::get_target(&state, AppType::Claude)
            .unwrap()
            .managed
    );
    unlink_agent_directory(&link);
}
