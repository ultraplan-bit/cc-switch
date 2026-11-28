//! One-shot native project import. Reads project files; only writes library/Profile records.

use super::*;
use crate::app_config::{InstalledSkill, McpApps, McpServer, MultiAppConfig};
use crate::database::Profile;
use crate::prompt::Prompt;
use crate::services::profile::{
    normalize_profile_model_catalog, normalize_project_path, profile_model_keys, ProfileConfig,
    ProfileProjectAliases, ProfileService,
};
use crate::services::PromptService;

struct ProjectInput {
    config: ProfileConfig,
    servers: Vec<McpServer>,
    skills: Vec<PathBuf>,
    instruction: Option<String>,
    warnings: Vec<String>,
}

fn read_project_text(root: &Path, relative: &str) -> Result<Option<String>, AppError> {
    let relative = Path::new(relative);
    ensure_project_parent(root, relative.parent().unwrap_or(Path::new(".")), false)?;
    let path = root.join(relative);
    ensure_regular_target(&path)?;
    if !path.exists() {
        return Ok(None);
    }
    crate::codex_config::read_limited_string(&path, 16 * 1024 * 1024).map(Some)
}

fn read_project_input(root: &Path, scope: ProfileScope) -> Result<ProjectInput, AppError> {
    let app = &scope.apps()[0];
    let mut input = ProjectInput {
        config: ProfileConfig {
            claude_project_connection: Default::default(),
            follow_global_resources: false,
            native_config: None,
            provider_id: None,
            model: String::new(),
            model_settings: Some(BTreeMap::new()),
            model_catalog: None,
            mcp_ids: Vec::new(),
            skill_ids: Vec::new(),
            prompt_id: None,
        },
        servers: Vec::new(),
        skills: Vec::new(),
        instruction: None,
        warnings: Vec::new(),
    };
    let mut parsed = MultiAppConfig::default();
    let mut disabled_mcp = HashSet::new();
    let mut disabled_skills = HashSet::new();
    let mut found = false;
    let (skills_dir, instruction_path) = match scope {
        ProfileScope::Claude => {
            let mut settings = Map::new();
            let mut env = Map::new();
            let mut skill_overrides = Map::new();
            let mut rules = Vec::new();
            for file in [".claude/settings.json", CLAUDE_PROJECT_SETTINGS] {
                if let Some(text) = read_project_text(root, file)? {
                    found = true;
                    let value: Value = serde_json::from_str(&text)
                        .map_err(|error| AppError::Config(format!("解析 {file} 失败: {error}")))?;
                    let object = value
                        .as_object()
                        .ok_or_else(|| AppError::Config(format!("{file} 必须是对象")))?;
                    rules.extend(claude_mcp_denial_rules(&value)?);
                    if let Some(overrides) = object.get("skillOverrides") {
                        skill_overrides.extend(
                            overrides
                                .as_object()
                                .ok_or_else(|| {
                                    AppError::Config(format!("{file} 的 skillOverrides 必须是对象"))
                                })?
                                .clone(),
                        );
                    }
                    if let Some(ids) = object
                        .get("disabledMcpjsonServers")
                        .and_then(Value::as_array)
                    {
                        disabled_mcp
                            .extend(ids.iter().filter_map(Value::as_str).map(str::to_string));
                    }
                    if let Some(value) = object.get("env") {
                        env.extend(
                            value
                                .as_object()
                                .ok_or_else(|| {
                                    AppError::Config(format!("{file} 的 env 必须是对象"))
                                })?
                                .clone(),
                        );
                    }
                    settings.extend(object.clone());
                }
            }
            let model = env.get("ANTHROPIC_MODEL").or_else(|| settings.get("model"));
            if let Some(model) = model {
                input.config.model = model
                    .as_str()
                    .ok_or_else(|| AppError::Config("Claude 项目模型必须是字符串".into()))?
                    .into();
            }
            for key in profile_model_keys(app) {
                if let Some(value) = env.get(*key) {
                    input.config.model_settings.as_mut().unwrap().insert(
                        (*key).into(),
                        value
                            .as_str()
                            .ok_or_else(|| {
                                AppError::Config(format!("模型设置 {key} 必须是字符串"))
                            })?
                            .into(),
                    );
                }
            }
            disabled_skills.extend(
                skill_overrides
                    .iter()
                    .filter(|(_, mode)| mode.as_str() == Some("off"))
                    .map(|(name, _)| name.clone()),
            );
            if let Some(text) = read_project_text(root, ".mcp.json")? {
                found = true;
                let value: Value = serde_json::from_str(&text)
                    .map_err(|error| AppError::Config(format!("解析 .mcp.json 失败: {error}")))?;
                if !value.is_object() {
                    return Err(AppError::Config(".mcp.json 必须是对象".into()));
                }
                if let Some(servers) = value.get("mcpServers") {
                    let servers = servers.as_object().ok_or_else(|| {
                        AppError::Config(".mcp.json 的 mcpServers 必须是对象".into())
                    })?;
                    crate::mcp::claude::import_from_document(&mut parsed, &value)?;
                    for (id, spec) in servers {
                        if rules
                            .iter()
                            .any(|rule| claude_mcp_rule_matches(rule, id, Some(spec)))
                        {
                            disabled_mcp.insert(id.clone());
                        }
                        if !parsed
                            .mcp
                            .servers
                            .as_ref()
                            .is_some_and(|items| items.contains_key(id))
                        {
                            input.warnings.push(format!("MCP '{id}' 配置无效，未导入"));
                        }
                    }
                }
            }
            for file in [".claude/CLAUDE.md", "CLAUDE.local.md"] {
                if read_project_text(root, file)?.is_some() {
                    input.warnings.push(format!(
                        "{file} 保留为项目原生指令；本次 Profile 仅导入根目录 CLAUDE.md"
                    ));
                }
            }
            (".claude/skills", "CLAUDE.md")
        }
        ProfileScope::Codex => {
            if let Some(text) = read_project_text(root, ".codex/config.toml")? {
                found = true;
                let document = text.parse::<toml_edit::DocumentMut>().map_err(|error| {
                    AppError::Config(format!("解析 .codex/config.toml 失败: {error}"))
                })?;
                let table: toml::Table =
                    toml::from_str(&text).map_err(|error| AppError::Config(error.to_string()))?;
                if let Some(model) = document.get("model") {
                    input.config.model = model
                        .as_str()
                        .ok_or_else(|| AppError::Config("Codex 项目模型必须是字符串".into()))?
                        .into();
                }
                for key in profile_model_keys(app) {
                    if let Some(value) = codex_model_setting(&document, key)? {
                        input
                            .config
                            .model_settings
                            .as_mut()
                            .unwrap()
                            .insert((*key).into(), value);
                    }
                }
                crate::mcp::codex::import_from_document(&mut parsed, &table)?;
                for servers in [
                    table.get("mcp_servers"),
                    table.get("mcp").and_then(|m| m.get("servers")),
                ]
                .into_iter()
                .flatten()
                {
                    let servers = servers
                        .as_table()
                        .ok_or_else(|| AppError::Config("Codex mcp_servers 必须是表".into()))?;
                    for (id, spec) in servers {
                        if spec.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
                            disabled_mcp.insert(id.clone());
                        }
                        if !parsed
                            .mcp
                            .servers
                            .as_ref()
                            .is_some_and(|items| items.contains_key(id))
                        {
                            input.warnings.push(format!("MCP '{id}' 配置无效，未导入"));
                        }
                    }
                }
                if document.get("model_catalog_json").is_some() {
                    let catalog = crate::codex_config::resolve_cc_switch_catalog_path(
                        &text,
                        &root.join(".codex"),
                    )
                    .and_then(|path| crate::codex_config::read_codex_model_catalog_text(&path).ok())
                    .and_then(|catalog| {
                        crate::codex_config::build_simplified_catalog_from_texts(&text, &catalog)
                    });
                    match catalog {
                        Some(catalog) => {
                            input.config.model_catalog =
                                Some(normalize_profile_model_catalog(catalog)?)
                        }
                        None => input.warnings.push(
                            "项目模型目录不是可安全读取的 CC Switch 目录，未导入；原配置保留"
                                .into(),
                        ),
                    }
                }
            }
            if read_project_text(root, "AGENTS.override.md")?.is_some() {
                input.warnings.push("AGENTS.override.md 优先于 AGENTS.md；本次只导入 AGENTS.md，请在 Agent 中核对实际指令来源".into());
            }
            (".agents/skills", "AGENTS.md")
        }
        ProfileScope::ClaudeDesktop => {
            return Err(AppError::InvalidInput(
                "Claude Desktop 不支持项目配置导入".into(),
            ))
        }
    };
    input.servers = parsed
        .mcp
        .servers
        .unwrap_or_default()
        .into_values()
        .filter(|server| {
            if disabled_mcp.contains(&server.id) {
                input.warnings.push(format!(
                    "MCP '{}' 在项目中已停用，未选入 Profile",
                    server.id
                ));
                false
            } else {
                true
            }
        })
        .collect();
    input.servers.sort_by(|left, right| left.id.cmp(&right.id));
    ensure_project_parent(root, Path::new(skills_dir), false)?;
    let skills_path = root.join(skills_dir);
    if skills_path.exists() {
        found = true;
        for entry in
            fs::read_dir(&skills_path).map_err(|error| AppError::io(&skills_path, error))?
        {
            let entry = entry.map_err(|error| AppError::io(&skills_path, error))?;
            let source = entry.path();
            if disabled_skills.contains(&entry.file_name().to_string_lossy().into_owned()) {
                continue;
            }
            match validate_copy_source(&source) {
                Ok(()) => input.skills.push(source),
                Err(error) => input.warnings.push(format!(
                    "Skill '{}' 未导入：{error}",
                    entry.file_name().to_string_lossy()
                )),
            }
        }
        input.skills.sort();
    }
    if !input.skills.is_empty()
        && matches!(
            crate::settings::get_skill_storage_location(),
            crate::services::skill::SkillStorageLocation::Unified
        )
    {
        input.warnings.push("当前资源库存储在 ~/.agents/skills；导入不会启用或同步 Agent，但原生支持此目录的 Agent 仍可能自动发现新内容。若需独立资源库，请在 Skills 存储设置选择 CC Switch 目录。".into());
    }
    input.instruction = read_project_text(root, instruction_path)?;
    found |= input.instruction.is_some();
    if !found {
        return Err(AppError::InvalidInput(
            "所选目录没有此 Agent 可导入的项目配置、Skills 或指令".into(),
        ));
    }
    Ok(input)
}

fn codex_model_setting(
    document: &toml_edit::DocumentMut,
    key: &str,
) -> Result<Option<String>, AppError> {
    let item = match key.split_once('.') {
        Some((section, field)) => document
            .get(section)
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|table| table.get(field)),
        None => document.get(key),
    };
    item.map(|item| {
        item.as_str()
            .map(str::to_string)
            .or_else(|| {
                crate::services::profile::is_codex_integer_model_setting(key)
                    .then(|| item.as_integer().map(|number| number.to_string()))
                    .flatten()
            })
            .ok_or_else(|| AppError::Config(format!("Codex 模型设置 {key} 类型无效")))
    })
    .transpose()
}

impl ProjectProfileService {
    pub fn import_from_project(
        state: &AppState,
        name: &str,
        scope: ProfileScope,
        project_path: &str,
    ) -> Result<(Profile, Vec<String>), AppError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput("Profile name is empty".into()));
        }
        let project_path = normalize_project_path(project_path)?
            .ok_or_else(|| AppError::InvalidInput("请选择项目目录".into()))?;
        let root = Path::new(&project_path)
            .canonicalize()
            .map_err(|error| AppError::io(Path::new(&project_path), error))?;
        if !root.is_dir() {
            return Err(AppError::InvalidInput("项目路径不是目录".into()));
        }
        let mut input = read_project_input(&root, scope)?;
        input.config.native_config = Some(capture_native_config(&root, &scope.apps()[0])?);
        let app = &scope.apps()[0];
        let now = chrono::Utc::now().timestamp();
        let mut aliases = ProfileProjectAliases::default();
        let mut new_mcp = Vec::<McpServer>::new();
        let mut new_skills = Vec::<(InstalledSkill, String)>::new();
        let mut new_prompt = None::<Prompt>;
        let mut profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            target: crate::database::ProfileTarget::Project,
            name: name.into(),
            payload: String::new(),
            sort_order: None,
            created_at: Some(now),
            updated_at: Some(now),
        };
        let mut profile_saved = false;
        let result = (|| -> Result<Profile, AppError> {
            let (mut existing, unreadable) = state
                .db
                .get_mcp_server_rows()?
                .into_readable_and_unreadable_ids();
            for mut server in input.servers {
                // Enablement belongs to the project selection, not the shared definition.
                if let Some(spec) = server.server.as_object_mut() {
                    spec.remove("enabled");
                }
                let native_id = server.id.clone();
                let reuse = existing
                    .values()
                    .find(|item| item.name == server.name && item.server == server.server);
                let id = if let Some(reuse) = reuse {
                    reuse.id.clone()
                } else {
                    if existing.contains_key(&server.id) || unreadable.contains(&server.id) {
                        server.id = format!("project-{}", uuid::Uuid::new_v4());
                    }
                    server.apps = McpApps::default();
                    state.db.save_mcp_server(&server)?;
                    new_mcp.push(server.clone());
                    existing.insert(server.id.clone(), server.clone());
                    server.id.clone()
                };
                if id != native_id {
                    aliases.mcp.insert(id.clone(), native_id);
                }
                input.config.mcp_ids.push(id);
            }
            for source in &input.skills {
                let native_dir = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| AppError::InvalidInput("Skill 目录名不是有效 UTF-8".into()))?;
                let original_hash = hash_tree(source, true)?;
                let (skill, created) = SkillService::import_from_project(&state.db, source)
                    .map_err(|error| AppError::Config(format!("导入 Skill 失败: {error}")))?;
                if created {
                    new_skills.push((skill.clone(), original_hash));
                }
                if skill.directory != native_dir {
                    aliases.skills.insert(skill.id.clone(), native_dir.into());
                }
                input.config.skill_ids.push(skill.id);
            }
            if let Some(content) = input.instruction {
                let prompts = state.db.get_prompts(app)?;
                let existing = prompts.values().find(|prompt| prompt.content == content);
                let id = if let Some(prompt) = existing {
                    prompt.id.clone()
                } else {
                    let prompt = Prompt {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: format!(
                            "{name} · {}",
                            if scope == ProfileScope::Claude {
                                "CLAUDE.md"
                            } else {
                                "AGENTS.md"
                            }
                        ),
                        content,
                        description: None,
                        enabled: false,
                        created_at: Some(now),
                        updated_at: Some(now),
                    };
                    PromptService::upsert_prompt(state, app.clone(), &prompt.id, prompt.clone())?;
                    new_prompt = Some(prompt.clone());
                    prompt.id
                };
                input.config.prompt_id = Some(id);
            }
            let mut payload = ProfilePayload::default();
            *payload.project_aliases.get_mut(app).unwrap() = aliases;
            profile.payload = serde_json::to_string(&payload)
                .map_err(|error| AppError::Config(error.to_string()))?;
            state.db.save_profile(&profile)?;
            profile_saved = true;
            ProfileService::update_config(state, &profile.id, scope, input.config, None)
        })();
        match result {
            Ok(profile) => {
                // The Profile is independent of the directory it was read from; the
                // directory is only remembered as a deployment target.
                if let Err(error) = state.db.add_profile_project(&project_path) {
                    input
                        .warnings
                        .push(format!("配置已导入，但未能记住此项目目录: {error}"));
                }
                Ok((profile, input.warnings))
            }
            Err(error) => {
                // Roll back only rows/files created here; never touch reused or concurrently edited resources.
                let mut failures = Vec::new();
                if profile_saved {
                    if let Err(error) = state.db.delete_profile(&profile.id) {
                        failures.push(error.to_string());
                    }
                }
                for server in new_mcp {
                    let cleanup = (|| -> Result<(), AppError> {
                        let current = state.db.get_all_mcp_servers()?;
                        if current.get(&server.id).is_some_and(|current| {
                            serde_json::to_value(current).ok() == serde_json::to_value(&server).ok()
                        }) {
                            state.db.delete_mcp_server(&server.id)?;
                        }
                        Ok(())
                    })();
                    if let Err(error) = cleanup {
                        failures.push(error.to_string());
                    }
                }
                for (skill, hash) in new_skills {
                    if let Err(error) =
                        SkillService::rollback_project_import(&state.db, &skill, &hash)
                    {
                        failures.push(error.to_string());
                    }
                }
                if let Some(prompt) = new_prompt {
                    let cleanup = (|| -> Result<(), AppError> {
                        let current = state.db.get_prompts(app)?;
                        if current.get(&prompt.id).is_some_and(|current| {
                            serde_json::to_value(current).ok() == serde_json::to_value(&prompt).ok()
                        }) {
                            state.db.delete_prompt(app, &prompt.id)?;
                        }
                        Ok(())
                    })();
                    if let Err(error) = cleanup {
                        failures.push(error.to_string());
                    }
                }
                if failures.is_empty() {
                    Err(error)
                } else {
                    Err(AppError::Config(format!(
                        "{error}；部分新增资源未能回滚：{}",
                        failures.join("；")
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempUserHome;
    use serial_test::serial;

    fn setup() -> (TempUserHome, AppState, tempfile::TempDir) {
        let home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        (home, state, tempfile::tempdir().unwrap())
    }

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn import(state: &AppState, root: &Path, scope: ProfileScope) -> (Profile, Vec<String>) {
        ProjectProfileService::import_from_project(state, "Imported", scope, root.to_str().unwrap())
            .unwrap()
    }

    #[test]
    #[serial]
    fn project_import_preserves_native_names_and_existing_library_on_redeploy() {
        let (_home, state, project) = setup();
        write(
            project.path(),
            ".claude/settings.json",
            r#"{"model":"shared","env":{"ANTHROPIC_DEFAULT_HAIKU_MODEL":"small"}}"#,
        );
        write(
            project.path(),
            CLAUDE_PROJECT_SETTINGS,
            r#"{"env":{"ANTHROPIC_MODEL":"local"},"permissions":{"allow":["Read"]}}"#,
        );
        let mcp = r#"{"mcpServers":{"files":{"command":"echo","args":["project"]}}}"#;
        write(project.path(), ".mcp.json", mcp);
        write(
            project.path(),
            ".claude/skills/review/SKILL.md",
            "---\nname: review\ndescription: Project review\n---\nProject skill",
        );
        write(project.path(), "CLAUDE.md", "Project instructions");
        let global_server = McpServer {
            id: "files".into(),
            name: "files".into(),
            server: serde_json::json!({"command":"echo","args":["global"]}),
            apps: McpApps {
                claude: true,
                ..McpApps::default()
            },
            description: None,
            homepage: None,
            docs: None,
            tags: vec![],
        };
        state.db.save_mcp_server(&global_server).unwrap();
        let other = tempfile::tempdir().unwrap();
        write(
            other.path(),
            "review/SKILL.md",
            "---\nname: review\ndescription: Global\n---\nGlobal skill",
        );
        let (existing_skill, _) =
            SkillService::import_from_project(&state.db, &other.path().join("review")).unwrap();
        let (profile, _) = import(&state, project.path(), ProfileScope::Claude);
        let payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        assert_eq!(payload.models.claude.as_deref(), Some("local"));
        assert_eq!(
            payload.model_settings.claude.as_ref().unwrap()["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
            "small"
        );
        assert!(payload.providers.claude.is_none());
        assert_eq!(
            payload.project_aliases.claude.mcp.values().next().unwrap(),
            "files"
        );
        assert_eq!(
            payload
                .project_aliases
                .claude
                .skills
                .values()
                .next()
                .unwrap(),
            "review"
        );
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["files"].server,
            global_server.server
        );
        assert!(state.db.get_all_mcp_servers().unwrap()["files"].apps.claude);
        assert_eq!(
            fs::read_to_string(project.path().join(".mcp.json")).unwrap(),
            mcp
        );
        let prompt = &state.db.get_prompts(&AppType::Claude).unwrap()
            [payload.prompts.claude.as_ref().unwrap()];
        assert!(!prompt.enabled);
        assert_eq!(prompt.content, "Project instructions");
        let registered = state.db.get_profile_projects().unwrap();
        assert_eq!(registered.len(), 1, "{registered:?}");
        let status = ProjectProfileService::project_status(&state, &registered[0]);
        assert!(status.available);
        assert!(status.deployed.is_empty());
        let result = ProjectProfileService::deploy_with_connection_choice(
            &state,
            &profile.id,
            ProfileScope::Claude,
            &registered[0],
            Some(ConflictResolution::Overwrite),
            true,
        )
        .unwrap();
        assert!(matches!(result, ProjectDeployResult::Applied { .. }));
        let deployed: Value =
            serde_json::from_str(&fs::read_to_string(project.path().join(".mcp.json")).unwrap())
                .unwrap();
        assert_eq!(deployed["mcpServers"].as_object().unwrap().len(), 1);
        assert_eq!(deployed["mcpServers"]["files"]["args"][0], "project");
        assert_eq!(
            fs::read_dir(project.path().join(".claude/skills"))
                .unwrap()
                .count(),
            1
        );
        let library = SkillService::get_installed_skill_dir(&existing_skill.directory).unwrap();
        assert!(fs::read_to_string(library.join("SKILL.md"))
            .unwrap()
            .contains("Global skill"));
        let settings: Value = serde_json::from_str(
            &fs::read_to_string(project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["skillOverrides"]["review"], "on");
        assert_eq!(settings["permissions"]["allow"][0], "Read");
        let count = state.db.get_all_mcp_servers().unwrap().len();
        let _ = import(&state, project.path(), ProfileScope::Claude);
        assert_eq!(state.db.get_all_mcp_servers().unwrap().len(), count);
    }

    #[test]
    #[serial]
    fn project_import_codex_reuses_native_parser_and_skips_disabled_resources() {
        let (_home, state, project) = setup();
        let config = "model = 'project-model'\nmodel_reasoning_effort = 'high'\n[agents]\nmax_threads = 3\n[mcp_servers.remote]\nurl = 'https://example.test/mcp'\nhttp_headers = { 'X-Workspace' = 'project' }\nenv_http_headers = { Authorization = 'API_TOKEN' }\n[mcp_servers.off]\ncommand = 'echo'\nenabled = false\n";
        write(project.path(), ".codex/config.toml", config);
        write(
            project.path(),
            ".agents/skills/check/SKILL.md",
            "---\nname: check\ndescription: test\n---\nCheck",
        );
        write(project.path(), "AGENTS.md", "Codex instructions");
        write(project.path(), "AGENTS.override.md", "Override");
        let (profile, warnings) = import(&state, project.path(), ProfileScope::Codex);
        let payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        assert_eq!(payload.models.codex.as_deref(), Some("project-model"));
        assert_eq!(
            payload.model_settings.codex.as_ref().unwrap()["agents.max_threads"],
            "3"
        );
        assert_eq!(payload.mcp.codex.as_ref().unwrap(), &["remote"]);
        let library = state.db.get_all_mcp_servers().unwrap();
        assert_eq!(
            library["remote"].server["headers"]["X-Workspace"],
            "project"
        );
        assert_eq!(
            library["remote"].server["env_http_headers"]["Authorization"],
            "API_TOKEN"
        );
        assert!(!library["remote"].apps.codex);
        assert_eq!(
            fs::read_to_string(project.path().join(".codex/config.toml")).unwrap(),
            config
        );
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("AGENTS.override.md")));
        assert!(warnings.iter().any(|warning| warning.contains("off")));
        let _ = import(&state, project.path(), ProfileScope::Codex);
        assert_eq!(state.db.get_all_installed_skills().unwrap().len(), 1);
        assert_eq!(state.db.get_prompts(&AppType::Codex).unwrap().len(), 1);
    }

    #[test]
    #[serial]
    fn project_import_invalid_model_rolls_back_only_new_library_entries() {
        let (_home, state, project) = setup();
        write(
            project.path(),
            ".codex/config.toml",
            "model = 'test'\n[agents]\nmax_threads = -1\n[mcp_servers.new]\ncommand = 'echo'\n",
        );
        write(
            project.path(),
            ".agents/skills/new/SKILL.md",
            "---\nname: new\ndescription: new\n---\nNew",
        );
        write(project.path(), "AGENTS.md", "New instructions");
        let result = ProjectProfileService::import_from_project(
            &state,
            "Invalid",
            ProfileScope::Codex,
            project.path().to_str().unwrap(),
        );
        assert!(result.is_err());
        assert!(state.db.get_all_profiles().unwrap().is_empty());
        assert!(state.db.get_all_mcp_servers().unwrap().is_empty());
        assert!(state.db.get_all_installed_skills().unwrap().is_empty());
        assert!(state.db.get_prompts(&AppType::Codex).unwrap().is_empty());
        assert!(project.path().join(".agents/skills/new/SKILL.md").is_file());
    }

    #[test]
    #[serial]
    fn project_import_rejects_empty_and_malformed_project_before_writes() {
        let (_home, state, project) = setup();
        assert!(ProjectProfileService::import_from_project(
            &state,
            "Empty",
            ProfileScope::Claude,
            project.path().to_str().unwrap()
        )
        .is_err());
        write(project.path(), ".mcp.json", r#"{"mcpServers":42}"#);
        assert!(ProjectProfileService::import_from_project(
            &state,
            "Invalid",
            ProfileScope::Claude,
            project.path().to_str().unwrap()
        )
        .is_err());
        assert!(state.db.get_all_profiles().unwrap().is_empty());
        assert!(state.db.get_all_mcp_servers().unwrap().is_empty());
    }

    #[test]
    #[serial]
    fn project_import_keeps_shared_denials_and_merges_skill_overrides() {
        let (_home, state, project) = setup();
        write(
            project.path(),
            ".claude/settings.json",
            r#"{"deniedMcpServers":[{"serverName":"blocked"}],"skillOverrides":{"hidden":"off","restored":"off"}}"#,
        );
        write(
            project.path(),
            CLAUDE_PROJECT_SETTINGS,
            r#"{"deniedMcpServers":[{"serverName":"other"}],"skillOverrides":{"restored":"on"}}"#,
        );
        write(
            project.path(),
            ".mcp.json",
            r#"{"mcpServers":{"blocked":{"command":"echo"},"other":{"command":"echo"},"selected":{"command":"echo"}}}"#,
        );
        for name in ["hidden", "restored"] {
            write(
                project.path(),
                &format!(".claude/skills/{name}/SKILL.md"),
                &format!("---\nname: {name}\ndescription: test\n---\nTest"),
            );
        }
        let (profile, _) = import(&state, project.path(), ProfileScope::Claude);
        let payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        assert_eq!(payload.mcp.claude.unwrap(), vec!["selected"]);
        let skills = state.db.get_all_installed_skills().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills.values().next().unwrap().directory, "restored");
    }
}
