use crate::support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};
use cc_switch_lib::{update_settings, AppSettings, AppType, Prompt, PromptService};
use std::fs;

fn shared_prompt() -> Prompt {
    Prompt {
        id: "shared".into(),
        name: "Shared instructions".into(),
        content: "Follow project conventions.".into(),
        description: Some("One definition for all Agents".into()),
        created_at: Some(1),
        updated_at: Some(1),
    }
}

#[test]
fn shared_library_changes_and_reads_never_write_or_backfill_agent_files() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let home = ensure_test_home();
    let state = create_test_state().unwrap();
    let original = shared_prompt();
    PromptService::upsert_prompt(&state, &original.id, original.clone()).unwrap();
    assert!(!home.join(".claude/CLAUDE.md").exists());
    assert!(!home.join(".codex/AGENTS.md").exists());
    for app in [AppType::Claude, AppType::Codex] {
        PromptService::enable_prompt(&state, app, &original.id).unwrap();
    }
    let mut edited = original.clone();
    edited.content = "Updated shared instructions.".into();
    PromptService::upsert_prompt(&state, &edited.id, edited.clone()).unwrap();
    for (app, relative) in [
        (AppType::Claude, ".claude/CLAUDE.md"),
        (AppType::Codex, ".codex/AGENTS.md"),
    ] {
        let path = home.join(relative);
        assert_eq!(fs::read_to_string(&path).unwrap(), original.content);
        assert!(
            !PromptService::get_target(&state, app.clone())
                .unwrap()
                .matches_library
        );
        fs::write(&path, "edited outside").unwrap();
        assert_eq!(
            PromptService::get_prompts(&state).unwrap()[&edited.id].content,
            edited.content
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited outside");
        assert!(PromptService::enable_prompt(&state, app.clone(), &edited.id).is_err());
        // An unreadable document cannot stop the independent library loading.
        fs::write(&path, [0xff, 0xfe, b'A', 0]).unwrap();
        assert_eq!(
            PromptService::get_prompts(&state).unwrap()[&edited.id].content,
            edited.content
        );
        assert!(PromptService::get_current_file_content(app).is_err());
    }
    PromptService::delete_prompt(&state, &edited.id).unwrap();
    assert!(PromptService::get_prompts(&state).unwrap().is_empty());
    assert_eq!(
        fs::read(home.join(".claude/CLAUDE.md")).unwrap(),
        [0xff, 0xfe, b'A', 0]
    );
    assert_eq!(
        fs::read(home.join(".codex/AGENTS.md")).unwrap(),
        [0xff, 0xfe, b'A', 0]
    );
}

#[test]
fn explicit_import_deduplicates_across_agents_without_claiming_or_changing_files() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let home = ensure_test_home();
    let state = create_test_state().unwrap();
    let mut ids = Vec::new();
    for (app, relative) in [
        (AppType::Claude, ".claude/CLAUDE.md"),
        (AppType::Codex, ".codex/AGENTS.md"),
    ] {
        let path = home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "Handwritten shared rules").unwrap();
        let id = PromptService::import_from_file(&state, app.clone()).unwrap();
        ids.push(id);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "Handwritten shared rules"
        );
        assert!(state.db.get_prompt_application(&app).unwrap().is_none());
    }
    assert_eq!(ids[0], ids[1]);
    assert_eq!(PromptService::get_prompts(&state).unwrap().len(), 1);
}

#[test]
fn distribution_preserves_agent_specific_instruction_filenames() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    update_settings(AppSettings::default()).unwrap();
    let home = ensure_test_home();
    let state = create_test_state().unwrap();
    let prompt = shared_prompt();
    PromptService::upsert_prompt(&state, &prompt.id, prompt.clone()).unwrap();
    for (app, relative) in [
        (AppType::Claude, ".claude/CLAUDE.md"),
        (AppType::Codex, ".codex/AGENTS.md"),
        (AppType::Gemini, ".gemini/GEMINI.md"),
        (AppType::Hermes, ".hermes/SOUL.md"),
        (AppType::Pi, ".pi/agent/AGENTS.md"),
    ] {
        PromptService::enable_prompt(&state, app, &prompt.id).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(relative)).unwrap(),
            prompt.content
        );
    }
    assert!(!home.join(".claude/AGENTS.md").exists());
    assert!(!home.join(".codex/CLAUDE.md").exists());
}

#[test]
fn omp_prompts_write_agents_md_in_the_configured_agent_directory() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let agent_dir = ensure_test_home().join("custom-omp/agent");
    update_settings(AppSettings {
        omp_config_dir: Some(agent_dir.to_string_lossy().into_owned()),
        ..AppSettings::default()
    })
    .unwrap();
    let state = create_test_state().unwrap();
    let prompt = shared_prompt();
    PromptService::upsert_prompt(&state, &prompt.id, prompt.clone()).unwrap();
    PromptService::enable_prompt(&state, AppType::Omp, &prompt.id).unwrap();
    assert_eq!(
        fs::read_to_string(agent_dir.join("AGENTS.md")).unwrap(),
        prompt.content
    );
    update_settings(AppSettings::default()).unwrap();
}
