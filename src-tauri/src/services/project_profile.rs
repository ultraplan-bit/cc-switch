//! Materialize a Profile's MCP servers and Skills into one trusted project.
//!
//! This is deliberately separate from `ProfileService::apply`: project output
//! must not change the global enabled flags or the user's global MCP/Skills.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::services::profile::{resource_label, ProfilePayload, ProfileScope, NO_ACTIVE_PROMPT};
use crate::services::skill::SkillService;
use crate::store::AppState;

pub(crate) mod connection;
mod import;
mod native;
pub(crate) mod readiness;
pub(crate) use native::capture_native_config;

static PROJECT_DEPLOY_LOCK: Mutex<()> = Mutex::new(());
const CLAUDE_PROJECT_SETTINGS: &str = ".claude/settings.local.json";

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictResolution {
    Overwrite,
    KeepCurrent,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeployConflict {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ProjectDeployResult {
    NeedsConnection {
        connection: readiness::InheritedConnection,
    },
    NeedsResolution {
        conflicts: Vec<ProjectDeployConflict>,
        warnings: Vec<String>,
    },
    Applied {
        changed: Vec<String>,
        warnings: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        connection: Option<AppliedConnection>,
    },
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedConnection {
    mode: &'static str,
    #[serde(flatten)]
    connection: readiness::InheritedConnection,
}

/// The Profile a project directory last received for one Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployedProfile {
    pub profile_id: String,
    /// The Profile changed since, or no longer exists.
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub path: String,
    /// The directory exists and can be resolved.
    pub available: bool,
    /// Keyed by Agent scope (`claude` / `codex`).
    pub deployed: BTreeMap<&'static str, DeployedProfile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectManifest {
    #[serde(default)]
    claude_connection_hashes: BTreeMap<String, String>,
    #[serde(default)]
    native_config_hashes: BTreeMap<String, String>,
    version: u8,
    project_root: String,
    app: String,
    mcp_servers: BTreeMap<String, String>,
    skills: BTreeMap<String, ManagedSkill>,
    #[serde(default)]
    claude_skill_overrides: BTreeMap<String, ManagedSkill>,
    #[serde(default)]
    claude_mcp_denials: BTreeMap<String, String>,
    #[serde(default)]
    claude_settings_file: Option<String>,
    model_hash: Option<String>,
    #[serde(default)]
    claude_env_model_hash: Option<String>,
    #[serde(default)]
    model_settings_hashes: BTreeMap<String, String>,
    #[serde(default)]
    codex_model_catalog_hash: Option<String>,
    #[serde(default)]
    codex_model_catalog_pointer_hash: Option<String>,
    instruction_hash: Option<String>,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default)]
    profile_scope_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedSkill {
    id: String,
    hash: String,
}

#[derive(Debug, Clone)]
struct McpOperation {
    id: String,
    desired: Option<Value>,
    desired_hash: Option<String>,
    current_hash: Option<String>,
    conflict: Option<ProjectDeployConflict>,
    no_op: bool,
}

#[derive(Debug, Clone)]
struct SkillOperation {
    relative_path: String,
    id: String,
    source: Option<PathBuf>,
    desired_hash: Option<String>,
    current_state: Option<String>,
    conflict: Option<ProjectDeployConflict>,
    no_op: bool,
}

#[derive(Debug, Clone)]
struct SkillOverrideOperation {
    name: String,
    id: String,
    desired: Option<String>,
    desired_hash: Option<String>,
    conflict: Option<ProjectDeployConflict>,
    no_op: bool,
}

struct SkillUndo {
    destination: PathBuf,
    backup: Option<PathBuf>,
    applied_state: Option<String>,
}

#[derive(Debug, Clone)]
struct ModelOperation {
    desired: Option<String>,
    desired_hash: Option<String>,
    conflict: Option<ProjectDeployConflict>,
    no_op: bool,
}

#[derive(Debug, Clone)]
struct ModelSettingOperation {
    path: String,
    key: String,
    operation: ModelOperation,
}

#[derive(Debug, Clone)]
struct ModelCatalogOperation {
    path: PathBuf,
    relative_path: String,
    desired: Option<Vec<u8>>,
    desired_hash: Option<String>,
    conflict: Option<ProjectDeployConflict>,
    original: Option<Vec<u8>>,
    current_hash: Option<String>,
    no_op: bool,
    pointer: ModelOperation,
}

#[derive(Debug, Clone)]
struct InstructionOperation {
    desired: Option<String>,
    current_hash: Option<String>,
    desired_hash: Option<String>,
    conflict: Option<ProjectDeployConflict>,
    no_op: bool,
    relative_path: String,
}

struct ProjectFileUndo {
    path: PathBuf,
    original: Option<Vec<u8>>,
    applied_hash: Option<String>,
}

enum ProjectMcpDocument {
    Claude {
        path: PathBuf,
        root: Value,
        initial_hash: Option<String>,
        changed: bool,
    },
    Codex {
        path: PathBuf,
        document: toml_edit::DocumentMut,
        initial_hash: Option<String>,
        changed: bool,
    },
}

struct ClaudeSettingsDocument {
    path: PathBuf,
    root: Value,
    initial_hash: Option<String>,
    changed: bool,
}

impl ClaudeSettingsDocument {
    fn open(root: &Path) -> Result<Self, AppError> {
        let relative = Path::new(".claude");
        ensure_project_parent(root, relative, false)?;
        let path = root.join(CLAUDE_PROJECT_SETTINGS);
        ensure_regular_target(&path)?;
        let bytes = read_optional(&path)?;
        let value = match &bytes {
            Some(bytes) => serde_json::from_slice::<Value>(bytes).map_err(|err| {
                AppError::Config(format!("解析 Claude 项目 settings.local.json 失败: {err}"))
            })?,
            None => Value::Object(Map::new()),
        };
        if !value.is_object() {
            return Err(AppError::Config(
                "Claude 项目 settings.local.json 根值必须是 JSON 对象".to_string(),
            ));
        }
        if value.get("model").is_some_and(|model| !model.is_string()) {
            return Err(AppError::Config(
                "Claude 项目 settings.local.json 的 model 必须是字符串".to_string(),
            ));
        }
        if value.get("env").is_some_and(|env| !env.is_object())
            || value
                .get("env")
                .and_then(|env| env.get("ANTHROPIC_MODEL"))
                .is_some_and(|model| !model.is_string())
        {
            return Err(AppError::Config(
                "Claude 项目 settings.local.json 的 env 必须是对象，env.ANTHROPIC_MODEL 必须是字符串"
                    .to_string(),
            ));
        }
        Ok(Self {
            path,
            root: value,
            initial_hash: bytes.as_deref().map(crate::live::engine::sha256_hex),
            changed: false,
        })
    }

    fn model(&self) -> Option<String> {
        self.root.get("model")?.as_str().map(str::to_string)
    }

    fn env_model(&self) -> Option<String> {
        self.root
            .get("env")?
            .get("ANTHROPIC_MODEL")?
            .as_str()
            .map(str::to_string)
    }

    fn env_setting(&self, key: &str) -> Result<Option<String>, AppError> {
        let Some(value) = self.root.get("env").and_then(|env| env.get(key)) else {
            return Ok(None);
        };
        value
            .as_str()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| {
                AppError::Config(format!(
                    "Claude 项目 settings.local.json 的 env.{key} 必须是字符串"
                ))
            })
    }

    fn set_env_model(&mut self, model: Option<&str>) -> Result<(), AppError> {
        self.set_env_setting("ANTHROPIC_MODEL", model)
    }

    fn set_env_setting(&mut self, key: &str, value: Option<&str>) -> Result<(), AppError> {
        let root = self.root.as_object_mut().ok_or_else(|| {
            AppError::Config("Claude 项目 settings.local.json 根值必须是 JSON 对象".to_string())
        })?;
        if value.is_none() && !root.contains_key("env") {
            return Ok(());
        }
        let env = root
            .entry("env")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                AppError::Config("Claude 项目 settings.local.json 的 env 必须是对象".into())
            })?;
        match value {
            Some(value) => {
                env.insert(key.into(), Value::String(value.into()));
                self.changed = true;
            }
            None => self.changed |= env.remove(key).is_some(),
        }
        Ok(())
    }

    fn set_model(&mut self, model: Option<&str>) -> Result<(), AppError> {
        let root = self.root.as_object_mut().ok_or_else(|| {
            AppError::Config("Claude 项目 settings.local.json 根值必须是 JSON 对象".to_string())
        })?;
        match model {
            Some(model) => {
                root.insert("model".to_string(), Value::String(model.to_string()));
                self.changed = true;
            }
            None => self.changed |= root.remove("model").is_some(),
        }
        Ok(())
    }

    fn save(&mut self, root: &Path) -> Result<Option<String>, AppError> {
        if !self.changed {
            return Ok(None);
        }
        ensure_project_parent(root, Path::new(".claude"), true)?;
        let bytes = serde_json::to_vec_pretty(&self.root).map_err(|err| {
            AppError::Config(format!("生成 Claude 项目 settings.local.json 失败: {err}"))
        })?;
        write_if_unchanged(&self.path, self.initial_hash.as_deref(), &bytes)?;
        let hash = crate::live::engine::sha256_hex(&bytes);
        self.initial_hash = Some(hash.clone());
        self.changed = false;
        Ok(Some(hash))
    }
}

impl ProjectMcpDocument {
    fn open(root: &Path, app: &AppType) -> Result<Self, AppError> {
        match app {
            AppType::Claude => {
                let path = root.join(".mcp.json");
                ensure_regular_target(&path)?;
                let bytes = read_optional(&path)?;
                let root_value = match &bytes {
                    Some(bytes) => serde_json::from_slice::<Value>(bytes).map_err(|err| {
                        AppError::Config(format!("解析项目 .mcp.json 失败: {err}"))
                    })?,
                    None => Value::Object(Map::new()),
                };
                let Some(root_obj) = root_value.as_object() else {
                    return Err(AppError::Config(
                        "项目 .mcp.json 根值必须是 JSON 对象".to_string(),
                    ));
                };
                if root_obj
                    .get("mcpServers")
                    .is_some_and(|value| !value.is_object())
                {
                    return Err(AppError::Config(
                        "项目 .mcp.json 的 mcpServers 必须是对象".to_string(),
                    ));
                }
                Ok(Self::Claude {
                    path,
                    root: root_value,
                    initial_hash: bytes.as_deref().map(crate::live::engine::sha256_hex),
                    changed: false,
                })
            }
            AppType::Codex => {
                let path = root.join(".codex").join("config.toml");
                ensure_project_parent(root, Path::new(".codex"), false)?;
                ensure_regular_target(&path)?;
                let bytes = read_optional(&path)?;
                let document = match &bytes {
                    Some(bytes) => std::str::from_utf8(bytes)
                        .map_err(|err| {
                            AppError::Config(format!("Codex 项目 config.toml 不是 UTF-8: {err}"))
                        })?
                        .parse::<toml_edit::DocumentMut>()
                        .map_err(|err| {
                            AppError::Config(format!("解析 Codex 项目 config.toml 失败: {err}"))
                        })?,
                    None => toml_edit::DocumentMut::new(),
                };
                if document
                    .get("mcp_servers")
                    .is_some_and(|item| item.as_table_like().is_none())
                {
                    return Err(AppError::Config(
                        "Codex 项目 config.toml 的 mcp_servers 必须是表".to_string(),
                    ));
                }
                if document
                    .get("model")
                    .is_some_and(|model| model.as_str().is_none())
                {
                    return Err(AppError::Config(
                        "Codex 项目 config.toml 的 model 必须是字符串".to_string(),
                    ));
                }
                Ok(Self::Codex {
                    path,
                    document,
                    initial_hash: bytes.as_deref().map(crate::live::engine::sha256_hex),
                    changed: false,
                })
            }
            _ => Err(AppError::InvalidInput(
                "项目部署仅支持 Claude Code 和 Codex".to_string(),
            )),
        }
    }

    fn entry_hash(&self, id: &str) -> Result<Option<String>, AppError> {
        let value = match self {
            Self::Claude { root, .. } => root
                .get("mcpServers")
                .and_then(Value::as_object)
                .and_then(|servers| servers.get(id))
                .cloned(),
            Self::Codex { document, .. } => {
                return codex_entry_hash(document, id);
            }
        };
        value
            .map(|value| {
                serde_json::to_vec(&value)
                    .map(|bytes| crate::live::engine::sha256_hex(&bytes))
                    .map_err(|err| {
                        AppError::Config(format!("计算 MCP 条目 '{id}' 指纹失败: {err}"))
                    })
            })
            .transpose()
    }

    fn hash_after_set(&self, id: &str, value: &Value) -> Result<String, AppError> {
        let mut clone = self.clone_document()?;
        clone.set_entry(id, value)?;
        clone
            .entry_hash(id)?
            .ok_or_else(|| AppError::Config(format!("生成 MCP 条目 '{id}' 失败")))
    }

    fn clone_document(&self) -> Result<Self, AppError> {
        match self {
            Self::Claude {
                path,
                root,
                initial_hash,
                changed,
            } => Ok(Self::Claude {
                path: path.clone(),
                root: root.clone(),
                initial_hash: initial_hash.clone(),
                changed: *changed,
            }),
            Self::Codex {
                path,
                document,
                initial_hash,
                changed,
            } => Ok(Self::Codex {
                path: path.clone(),
                document: document.clone(),
                initial_hash: initial_hash.clone(),
                changed: *changed,
            }),
        }
    }

    fn set_entry(&mut self, id: &str, value: &Value) -> Result<(), AppError> {
        match self {
            Self::Claude { root, changed, .. } => {
                let root = root.as_object_mut().ok_or_else(|| {
                    AppError::Config("项目 .mcp.json 根值必须是 JSON 对象".to_string())
                })?;
                let servers = root
                    .entry("mcpServers")
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or_else(|| {
                        AppError::Config("项目 .mcp.json 的 mcpServers 必须是对象".to_string())
                    })?;
                servers.insert(id.to_string(), value.clone());
                *changed = true;
            }
            Self::Codex {
                document, changed, ..
            } => {
                let table = crate::mcp::codex::json_server_to_codex_toml_table(value)?;
                crate::mcp::codex::upsert_mcp_server_table(document, id, table)?;
                *changed = true;
            }
        }
        Ok(())
    }

    fn remove_entry(&mut self, id: &str) {
        match self {
            Self::Claude { root, changed, .. } => {
                if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
                    *changed |= servers.remove(id).is_some();
                }
            }
            Self::Codex {
                document, changed, ..
            } => {
                let before = codex_entry_hash(document, id).ok().flatten();
                crate::mcp::codex::remove_mcp_server_from_doc(document, id);
                let after = codex_entry_hash(document, id).ok().flatten();
                *changed |= before != after;
            }
        }
    }

    fn model(&self) -> Option<String> {
        match self {
            Self::Claude { .. } => None,
            Self::Codex { document, .. } => document
                .get("model")
                .and_then(toml_edit::Item::as_str)
                .map(str::to_string),
        }
    }

    fn model_setting(&self, path: &str) -> Result<Option<String>, AppError> {
        let Self::Codex { document, .. } = self else {
            return Ok(None);
        };
        let item = if let Some((section, key)) = path.split_once('.') {
            document
                .get(section)
                .and_then(toml_edit::Item::as_table_like)
                .and_then(|table| table.get(key))
        } else {
            document.get(path)
        };
        let Some(item) = item else {
            return Ok(None);
        };
        let value = if crate::services::profile::is_codex_integer_model_setting(path) {
            item.as_integer().map(|value| value.to_string())
        } else {
            item.as_str().map(str::to_string)
        };
        value.map(Some).ok_or_else(|| {
            AppError::Config(format!("Codex 项目 config.toml 的模型设置 {path} 类型无效"))
        })
    }

    fn model_catalog_pointer(&self) -> Option<String> {
        let Self::Codex { document, .. } = self else {
            return None;
        };
        document
            .get("model_catalog_json")
            .and_then(toml_edit::Item::as_str)
            .map(str::to_string)
    }

    fn set_model(&mut self, model: Option<&str>) -> Result<(), AppError> {
        match self {
            Self::Claude { .. } => Err(AppError::InvalidInput(
                "Claude Code model must be written to settings.local.json".to_string(),
            )),
            Self::Codex {
                document, changed, ..
            } => {
                match model {
                    Some(model) => {
                        document["model"] = toml_edit::value(model);
                        *changed = true;
                    }
                    None => *changed |= document.remove("model").is_some(),
                }
                Ok(())
            }
        }
    }

    fn set_model_setting(&mut self, path: &str, value: Option<&String>) -> Result<(), AppError> {
        match self {
            Self::Claude { .. } => Err(AppError::InvalidInput(
                "Claude Code model settings must be written to settings.local.json".to_string(),
            )),
            Self::Codex {
                document, changed, ..
            } => {
                crate::services::profile::set_codex_model_setting(document, path, value)?;
                *changed = true;
                Ok(())
            }
        }
    }

    fn set_model_catalog_pointer(&mut self, value: Option<&str>) -> Result<(), AppError> {
        match self {
            Self::Claude { .. } => Err(AppError::InvalidInput(
                "Codex model catalog pointer cannot be written to Claude config".to_string(),
            )),
            Self::Codex {
                path,
                document,
                changed,
                ..
            } => {
                match value {
                    Some(value) => {
                        let config_dir = path.parent().ok_or_else(|| {
                            AppError::InvalidInput("Codex project config path invalid".into())
                        })?;
                        let pointer =
                            crate::live::project::codex::catalog_pointer_for(config_dir, document);
                        if value != pointer {
                            return Err(AppError::InvalidInput(
                                "Codex project model catalog pointer changed while planning".into(),
                            ));
                        }
                        document["model_catalog_json"] = toml_edit::value(value);
                        *changed = true;
                    }
                    None => *changed |= document.remove("model_catalog_json").is_some(),
                }
                Ok(())
            }
        }
    }

    fn save(self, root: &Path) -> Result<bool, AppError> {
        match self {
            Self::Claude {
                path,
                root: value,
                initial_hash,
                changed,
            } => {
                if !changed {
                    return Ok(false);
                }
                ensure_project_parent(root, Path::new("."), true)?;
                let bytes = serde_json::to_vec_pretty(&value)
                    .map_err(|err| AppError::Config(format!("生成项目 .mcp.json 失败: {err}")))?;
                write_if_unchanged(&path, initial_hash.as_deref(), &bytes)?;
                Ok(true)
            }
            Self::Codex {
                path,
                document,
                initial_hash,
                changed,
            } => {
                if !changed {
                    return Ok(false);
                }
                ensure_project_parent(root, Path::new(".codex"), true)?;
                write_if_unchanged(
                    &path,
                    initial_hash.as_deref(),
                    document.to_string().as_bytes(),
                )?;
                Ok(true)
            }
        }
    }
}

pub struct ProjectProfileService;

impl ProjectProfileService {
    /// What a registered project directory currently has deployed, per Agent.
    /// The last deployed Profile is shared by sessions in the same project and Agent.
    #[cfg(test)]
    pub fn project_status(state: &AppState, path: &str) -> ProjectStatus {
        Self::project_status_with(path, |id| {
            state
                .db
                .get_profile(id)
                .ok()
                .flatten()
                .map(|profile| profile.payload)
        })
    }

    /// `payload_of` returns the stored payload of a Profile id, or None when it is gone.
    pub(crate) fn project_status_with(
        path: &str,
        payload_of: impl Fn(&str) -> Option<String>,
    ) -> ProjectStatus {
        let mut status = ProjectStatus {
            path: path.to_string(),
            available: false,
            deployed: BTreeMap::new(),
        };
        let root = match Path::new(path).canonicalize() {
            Ok(root) if root.is_dir() => root,
            result => {
                log::warn!("读取项目部署状态失败，目录不可用: {result:?}");
                return status;
            }
        };
        status.available = true;
        let Ok(_guard) = PROJECT_DEPLOY_LOCK.lock() else {
            log::warn!("读取项目部署状态失败，部署锁已损坏");
            return status;
        };
        for scope in [ProfileScope::Claude, ProfileScope::Codex] {
            let manifest = match load_manifest(&root, scope.as_str()) {
                Ok(manifest) => manifest,
                Err(error) => {
                    log::warn!("读取项目的 {} 部署状态失败: {error}", scope.as_str());
                    continue;
                }
            };
            if scope == ProfileScope::Claude
                && manifest.claude_settings_file.as_deref() != Some(CLAUDE_PROJECT_SETTINGS)
            {
                continue;
            }
            let Some(profile_id) = manifest.profile_id.clone() else {
                continue;
            };
            let current = payload_of(&profile_id)
                .and_then(|payload| serde_json::from_str::<ProfilePayload>(&payload).ok())
                .is_some_and(|payload| manifest_matches_payload(&manifest, &payload, scope));
            status.deployed.insert(
                scope.as_str(),
                DeployedProfile {
                    profile_id,
                    stale: !current,
                },
            );
        }
        status
    }

    /// Deploy a Profile to a registered project directory.
    pub fn deploy(
        state: &AppState,
        profile_id: &str,
        scope: ProfileScope,
        project_path: &str,
        resolution: Option<ConflictResolution>,
    ) -> Result<ProjectDeployResult, AppError> {
        Self::deploy_with_connection_choice(
            state,
            profile_id,
            scope,
            project_path,
            resolution,
            false,
        )
    }

    pub fn deploy_with_connection_choice(
        state: &AppState,
        profile_id: &str,
        scope: ProfileScope,
        project_path: &str,
        resolution: Option<ConflictResolution>,
        allow_missing_connection: bool,
    ) -> Result<ProjectDeployResult, AppError> {
        if matches!(resolution, Some(ConflictResolution::Skip)) {
            return Ok(ProjectDeployResult::Skipped);
        }
        let project_path = crate::services::profile::normalize_project_path(project_path)?
            .ok_or_else(|| AppError::InvalidInput("请选择项目目录".to_string()))?;
        if !state.db.is_profile_project(&project_path)? {
            return Err(AppError::InvalidInput("请先登记项目目录".into()));
        }
        let _guard = PROJECT_DEPLOY_LOCK
            .lock()
            .map_err(|_| AppError::Config("项目部署锁已损坏".to_string()))?;
        let (app, app_name) = match scope {
            ProfileScope::Claude => (AppType::Claude, "claude"),
            ProfileScope::Codex => (AppType::Codex, "codex"),
            _ => {
                return Err(AppError::InvalidInput(
                    "项目部署仅支持 Claude Code 和 Codex".to_string(),
                ))
            }
        };

        let profile = state
            .db
            .get_profile(profile_id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Profile not found: {profile_id}")))?;
        crate::services::profile::ProfileService::require_target(
            &profile,
            crate::database::ProfileTarget::Project,
        )?;
        let project_root = Path::new(&project_path)
            .canonicalize()
            .map_err(|err| AppError::io(Path::new(&project_path), err))?;
        if !project_root.is_dir() {
            return Err(AppError::InvalidInput("项目目录不可用".to_string()));
        }

        let payload: ProfilePayload = serde_json::from_str(&profile.payload)
            .map_err(|err| AppError::Config(format!("解析 Profile payload 失败: {err}")))?;
        if !payload.scope_captured(scope) {
            return Err(AppError::InvalidInput(format!(
                "Profile「{}」没有 Claude Code 或 Codex 的配置快照",
                profile.name
            )));
        }
        let inherited = (app == AppType::Claude && !connection::is_direct(&payload, &app))
            .then(|| readiness::inspect(state, Some(&project_root)));
        if let Some(inherited) = inherited.as_ref() {
            if inherited.status == readiness::ConnectionStatus::Missing
                && profile_selects_model(&payload)
                && !allow_missing_connection
            {
                return Ok(ProjectDeployResult::NeedsConnection {
                    connection: inherited.clone(),
                });
            }
        }
        let mut manifest = load_manifest(&project_root, app_name)?;
        let previous_profile_id = manifest.profile_id.clone();
        let previous_scope_hash = manifest.profile_scope_hash.clone();
        let previous_settings_file = manifest.claude_settings_file.clone();
        let previous_model_settings_hashes = manifest.model_settings_hashes.clone();
        let previous_native_hashes = manifest.native_config_hashes.clone();
        let previous_connection_hashes = manifest.claude_connection_hashes.clone();
        let previous_catalog_hash = manifest.codex_model_catalog_hash.clone();
        let previous_catalog_pointer_hash = manifest.codex_model_catalog_pointer_hash.clone();
        if app == AppType::Claude {
            manifest.claude_settings_file = Some(CLAUDE_PROJECT_SETTINGS.to_string());
        }

        let mut warnings = Vec::new();
        let mut changed = Vec::new();
        let mcp_ids = payload.mcp.get(&app).and_then(Option::as_ref);
        let skill_ids = payload.skills.get(&app).and_then(Option::as_ref);
        let aliases = payload
            .project_aliases
            .get(&app)
            .expect("validated profile app");
        let names = payload
            .resource_names
            .get(&app)
            .expect("validated profile app");
        let direct_provider = if connection::is_direct(&payload, &app) {
            connection::ensure_untracked(&project_root)?;
            Some(connection::provider(state, &payload)?)
        } else {
            None
        };
        let direct_native = direct_provider
            .as_ref()
            .map(|provider| connection::native_behavior(&project_root, &payload, provider))
            .transpose()?;
        let native_config = direct_native
            .as_deref()
            .or_else(|| payload.native_config.get(&app).and_then(Option::as_deref));
        let mut desired_model = desired_project_model(state, &payload, &app, &mut warnings)?;
        let mut desired_model_settings =
            desired_project_model_settings(state, &payload, &app, &mut warnings)?;
        if app == AppType::Claude
            && direct_provider.is_none()
            && !manifest.claude_connection_hashes.is_empty()
            && desired_model.is_none()
        {
            // A previously pinned provider is no longer the global connection.
            // Drop only its managed models along with its connection credentials.
            desired_model = Some(None);
            desired_model_settings = Some(BTreeMap::new());
            warnings.push("恢复全局连接时，已撤下与当前全局供应商不匹配的托管项目模型。".into());
        }
        let desired_catalog = desired_codex_model_catalog(state, &payload, &app, &mut warnings)?;
        let desired_instruction = desired_instruction(state, &payload, &app, &mut warnings)?;
        if app == AppType::Codex
            && desired_instruction.is_some()
            && fs::metadata(project_root.join("AGENTS.override.md"))
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
        {
            warnings.push(
                "项目已有 AGENTS.override.md，Codex 会优先读取它并忽略同目录的 AGENTS.md；请在 Agent 中核对实际指令来源。本次不会修改 AGENTS.override.md。"
                    .to_string(),
            );
        }

        let mut mcp_doc = if mcp_ids.is_some()
            || (app == AppType::Codex && native_config.is_some())
            || (app == AppType::Codex
                && (desired_model.is_some()
                    || desired_model_settings.is_some()
                    || desired_catalog.is_some()))
        {
            Some(ProjectMcpDocument::open(&project_root, &app)?)
        } else {
            None
        };
        let mut settings_doc = if app == AppType::Claude
            && (desired_model.is_some()
                || desired_model_settings.is_some()
                || skill_ids.is_some()
                || mcp_ids.is_some()
                || native_config.is_some()
                || direct_provider.is_some()
                || !manifest.claude_connection_hashes.is_empty())
        {
            Some(ClaudeSettingsDocument::open(&project_root)?)
        } else {
            None
        };

        let connection_ops = if let Some(settings) = settings_doc.as_ref() {
            connection::plan(direct_provider.as_ref(), settings, &manifest)?
        } else {
            Vec::new()
        };

        let (model_path, current_model) = match app {
            AppType::Claude => (
                ".claude/settings.local.json",
                settings_doc
                    .as_ref()
                    .and_then(ClaudeSettingsDocument::model),
            ),
            AppType::Codex => (
                ".codex/config.toml",
                mcp_doc.as_ref().and_then(ProjectMcpDocument::model),
            ),
            _ => unreachable!("scope was validated above"),
        };
        let model_op = plan_model_op(model_path, desired_model.clone(), current_model, &manifest);
        let env_model_op = if app == AppType::Claude {
            plan_model_field_op(
                ".claude/settings.local.json#env.ANTHROPIC_MODEL",
                desired_model,
                settings_doc
                    .as_ref()
                    .and_then(ClaudeSettingsDocument::env_model),
                manifest.claude_env_model_hash.as_ref(),
            )
        } else {
            None
        };
        let model_setting_ops = plan_model_setting_ops(
            &app,
            desired_model_settings.as_ref(),
            settings_doc.as_ref(),
            mcp_doc.as_ref(),
            &manifest,
        )?;
        let native_ops = native::plan(
            &app,
            native_config,
            settings_doc.as_ref(),
            mcp_doc.as_ref(),
            &manifest,
        )?;
        let catalog_op = plan_model_catalog_op(
            &project_root,
            &app,
            desired_catalog,
            mcp_doc.as_ref(),
            &manifest,
            &mut warnings,
        )?;
        let instruction_op =
            plan_instruction_op(&project_root, &app, desired_instruction, &manifest)?;

        let desired_mcp = if let Some(ids) = mcp_ids {
            load_desired_mcp(
                state.db.as_ref(),
                ids,
                &app,
                &names.mcp,
                &aliases.mcp,
                &mut warnings,
            )?
        } else {
            DesiredMcp::default()
        };
        let blocked_mcp = desired_mcp.blocked_ids.clone();
        let mcp_ops = if mcp_ids.is_some() {
            let doc = mcp_doc
                .as_ref()
                .ok_or_else(|| AppError::Config("缺少项目 MCP 配置文档".to_string()))?;
            plan_mcp_ops(
                doc,
                &manifest,
                &desired_mcp.entries,
                &blocked_mcp,
                state.db.as_ref(),
                &mut warnings,
            )?
        } else {
            Vec::new()
        };

        let mcp_denial_ops = if app == AppType::Claude {
            match (settings_doc.as_ref(), mcp_ids) {
                (Some(settings), Some(ids)) => plan_claude_mcp_denials(
                    &project_root,
                    settings,
                    &manifest,
                    state.db.as_ref(),
                    &ids.iter()
                        .map(|id| aliases.mcp.get(id).unwrap_or(id).clone())
                        .collect::<Vec<_>>(),
                    &desired_mcp,
                    &mut warnings,
                )?,
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let desired_skills = if let Some(ids) = skill_ids {
            load_desired_skills(
                state.db.as_ref(),
                ids,
                &names.skills,
                &aliases.skills,
                &mut warnings,
            )?
        } else {
            DesiredSkills::default()
        };
        let skill_ops = if skill_ids.is_some() {
            plan_skill_ops(&project_root, &app, &manifest, &desired_skills)?
        } else {
            Vec::new()
        };
        let skill_override_ops = if app == AppType::Claude {
            match (settings_doc.as_ref(), skill_ids) {
                (Some(settings), Some(ids)) => plan_claude_skill_overrides(
                    &project_root,
                    settings,
                    &manifest,
                    state.db.as_ref(),
                    &desired_skills,
                    ids,
                    &mut warnings,
                )?,
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let conflicts = mcp_ops
            .iter()
            .filter_map(|op| op.conflict.clone())
            .chain(skill_ops.iter().filter_map(|op| op.conflict.clone()))
            .chain(
                skill_override_ops
                    .iter()
                    .filter_map(|op| op.conflict.clone()),
            )
            .chain(model_op.iter().filter_map(|op| op.conflict.clone()))
            .chain(env_model_op.iter().filter_map(|op| op.conflict.clone()))
            .chain(
                model_setting_ops
                    .iter()
                    .filter_map(|op| op.operation.conflict.clone()),
            )
            .chain(catalog_op.as_ref().into_iter().flat_map(|op| {
                [op.conflict.clone(), op.pointer.conflict.clone()]
                    .into_iter()
                    .flatten()
            }))
            .chain(mcp_denial_ops.iter().filter_map(|op| op.conflict.clone()))
            .chain(instruction_op.iter().filter_map(|op| op.conflict.clone()))
            .chain(
                native_ops
                    .iter()
                    .filter_map(|op| op.operation.conflict.clone()),
            )
            .chain(
                connection_ops
                    .iter()
                    .filter_map(|op| op.operation.conflict.clone()),
            )
            .collect::<Vec<_>>();
        if !conflicts.is_empty() && resolution.is_none() {
            return Ok(ProjectDeployResult::NeedsResolution {
                conflicts,
                warnings,
            });
        }
        if matches!(resolution, Some(ConflictResolution::Skip)) {
            return Ok(ProjectDeployResult::Skipped);
        }

        let manifest_file = manifest_path(&project_root, app_name)?;
        ensure_regular_target(&manifest_file)?;
        let original_manifest = read_optional(&manifest_file)?;
        let mut skill_undo = Vec::new();
        let mut file_undo = Vec::new();
        // Address, authentication and model must remain a coherent connection.
        let keep_connection = !connection_ops.is_empty()
            && matches!(resolution, Some(ConflictResolution::KeepCurrent))
            && (connection_ops
                .iter()
                .any(|op| op.operation.conflict.is_some())
                || model_op.as_ref().is_some_and(|op| op.conflict.is_some())
                || env_model_op
                    .as_ref()
                    .is_some_and(|op| op.conflict.is_some())
                || model_setting_ops
                    .iter()
                    .any(|op| op.operation.conflict.is_some()));
        if keep_connection {
            warnings.push(
                "已保留项目现有连接与模型；供应商地址、凭据和模型未更新，请处理冲突后重新应用。"
                    .into(),
            );
        }
        let deployment = (|| {
            if !keep_connection && !connection_ops.is_empty() {
                if direct_provider.is_some() {
                    connection::ensure_ignored(&project_root, &mut file_undo, &mut changed)?;
                }
                connection::stage(
                    &connection_ops,
                    settings_doc
                        .as_mut()
                        .expect("connection requires Claude settings"),
                    &mut manifest,
                    &mut changed,
                )?;
            }
            apply_skill_ops(
                &project_root,
                &mut manifest,
                &skill_ops,
                resolution,
                app_name,
                &mut changed,
                &mut skill_undo,
            )?;

            if !keep_connection
                && (model_op.is_some()
                    || env_model_op.is_some()
                    || !model_setting_ops.is_empty()
                    || catalog_op.is_some()
                    || !connection_ops.is_empty())
            {
                apply_model_op(
                    &project_root,
                    &app,
                    app_name,
                    model_op.as_ref(),
                    env_model_op.as_ref(),
                    &model_setting_ops,
                    catalog_op.as_ref(),
                    resolution,
                    settings_doc.as_mut(),
                    mcp_doc.as_mut(),
                    &mut manifest,
                    &mut changed,
                    &mut file_undo,
                )?;
            }

            if let Some(operation) = instruction_op.as_ref() {
                apply_instruction_op(
                    &project_root,
                    app_name,
                    operation,
                    resolution,
                    &mut manifest,
                    &mut changed,
                    &mut file_undo,
                )?;
            }

            if let Some(settings) = settings_doc.as_mut() {
                apply_claude_skill_overrides(
                    &project_root,
                    settings,
                    &mut manifest,
                    &skill_override_ops,
                    resolution,
                    &mut changed,
                    &mut file_undo,
                )?;
            }

            if let Some(settings) = settings_doc.as_mut() {
                apply_claude_mcp_denials(
                    &project_root,
                    settings,
                    &mut manifest,
                    &mcp_denial_ops,
                    resolution,
                    &mut changed,
                    &mut file_undo,
                )?;
            }

            if let Some(doc) = mcp_doc.as_mut() {
                apply_mcp_ops(
                    doc,
                    &mut manifest,
                    &mcp_ops,
                    resolution,
                    &project_root,
                    app_name,
                    &mut changed,
                )?;
            }
            native::apply(
                &project_root,
                &app,
                &native_ops,
                resolution,
                settings_doc.as_mut(),
                mcp_doc.as_mut(),
                &mut manifest,
                &mut changed,
                &mut file_undo,
            )?;
            // Persist the active selection before the last project write, so a
            // failure can restore it together with the original ownership manifest.
            manifest.profile_id = Some(profile_id.to_string());
            manifest.profile_scope_hash =
                (!keep_connection).then(|| profile_scope_hash(&payload, scope));
            save_manifest(&project_root, app_name, &manifest)?;
            if let Some(doc) = mcp_doc.take() {
                if doc.save(&project_root)? {
                    changed.push(match app {
                        AppType::Claude => ".mcp.json".to_string(),
                        _ => ".codex/config.toml".to_string(),
                    });
                }
            }
            Ok::<(), AppError>(())
        })();

        if let Err(error) = deployment {
            let mut rollback_errors = rollback_skill_ops(&skill_undo);
            rollback_errors.extend(rollback_project_files(&file_undo));
            if rollback_errors.is_empty() {
                if let Err(restore_error) =
                    restore_manifest(&manifest_file, original_manifest.as_deref())
                {
                    rollback_errors.push(format!("恢复部署清单失败: {restore_error}"));
                }
            } else {
                manifest.profile_id = previous_profile_id;
                manifest.profile_scope_hash = previous_scope_hash;
                manifest.claude_settings_file = previous_settings_file;
                manifest.model_settings_hashes = previous_model_settings_hashes;
                manifest.native_config_hashes = previous_native_hashes;
                manifest.claude_connection_hashes = previous_connection_hashes;
                manifest.codex_model_catalog_hash = previous_catalog_hash;
                manifest.codex_model_catalog_pointer_hash = previous_catalog_pointer_hash;
                if let Err(restore_error) = save_manifest(&project_root, app_name, &manifest) {
                    rollback_errors
                        .push(format!("恢复此前部署的 Profile 标记失败: {restore_error}"));
                }
                rollback_errors.push("为保留部分失败的部署状态，未恢复旧部署清单".to_string());
            }
            return if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(AppError::Config(format!(
                    "项目部署失败: {error}; 回滚未完全成功: {}",
                    rollback_errors.join("；")
                )))
            };
        }
        cleanup_skill_backups(&skill_undo);
        let connection = if app != AppType::Claude || keep_connection {
            None
        } else if let Some(provider) = direct_provider {
            Some(AppliedConnection {
                mode: "direct",
                connection: readiness::InheritedConnection {
                    status: readiness::ConnectionStatus::Configured,
                    source: Some(readiness::ConnectionSource::Project),
                    provider_name: Some(provider.name),
                    reason: None,
                },
            })
        } else {
            inherited.map(|connection| AppliedConnection {
                mode: "inherit",
                connection,
            })
        };
        Ok(ProjectDeployResult::Applied {
            changed,
            warnings,
            connection,
        })
    }
}

fn profile_selects_model(payload: &ProfilePayload) -> bool {
    (payload.providers.claude.is_some() && payload.models.claude.is_none())
        || payload
            .models
            .claude
            .as_ref()
            .is_some_and(|model| !model.trim().is_empty())
        || payload
            .model_settings
            .claude
            .as_ref()
            .is_some_and(|settings| {
                settings
                    .iter()
                    .any(|(key, value)| key.ends_with("_MODEL") && !value.trim().is_empty())
            })
}

/// Whether the deployment recorded in a manifest still equals the Profile's
/// configuration for that Agent.
fn manifest_matches_payload(
    manifest: &ProjectManifest,
    payload: &ProfilePayload,
    scope: ProfileScope,
) -> bool {
    manifest.profile_scope_hash.as_ref() == Some(&profile_scope_hash(payload, scope))
}

fn profile_scope_hash(payload: &ProfilePayload, scope: ProfileScope) -> String {
    let app = &scope.apps()[0];
    let mut snapshot = serde_json::json!({
        "provider": payload.providers.get(app),
        "model": payload.models.get(app),
        "modelSettings": payload.model_settings.get(app),
        "modelCatalog": payload.model_catalog.get(app),
        "mcp": normalized_scope_ids(payload, app, true),
        "skills": normalized_scope_ids(payload, app, false),
        "prompt": payload.prompts.get(app),
    });
    if scope == ProfileScope::Claude {
        snapshot["claudeProjectConnection"] =
            serde_json::to_value(payload.claude_project_connection)
                .expect("serializable connection mode");
    }
    if let Some(aliases) = payload.project_aliases.get(app) {
        if !aliases.mcp.is_empty() || !aliases.skills.is_empty() {
            snapshot["projectAliases"] =
                serde_json::to_value(aliases).expect("serializable aliases");
        }
    }
    if let Some(Some(native)) = payload.native_config.get(app) {
        snapshot["nativeConfig"] = Value::String(native.clone());
    }
    text_hash(&snapshot.to_string())
}

fn normalized_scope_ids(payload: &ProfilePayload, app: &AppType, mcp: bool) -> Option<Vec<String>> {
    let ids = if mcp {
        payload.mcp.get(app)
    } else {
        payload.skills.get(app)
    };
    ids.and_then(Option::as_ref).map(|ids| {
        let mut ids = ids.clone();
        ids.sort();
        ids.dedup();
        ids
    })
}

pub(crate) fn desired_project_model(
    state: &AppState,
    payload: &ProfilePayload,
    app: &AppType,
    warnings: &mut Vec<String>,
) -> Result<Option<Option<String>>, AppError> {
    if connection::is_direct(payload, app) {
        return connection::model(payload, &connection::provider(state, payload)?)
            .map(|model| Some(Some(model)));
    }
    let selected = payload.models.get(app).and_then(Option::as_deref);
    if let Some(model) = selected {
        if model.is_empty() {
            return Ok(Some(None));
        }
        match crate::mode::stack::resolve(
            state.db.as_ref(),
            &crate::live::engine::DeviceStore::for_device(),
            app,
            model,
        )? {
            crate::mode::stack::Resolved::Plain => {}
            crate::mode::stack::Resolved::Hit(target) => {
                if payload
                    .providers
                    .get(app)
                    .and_then(Option::as_deref)
                    .is_some_and(|id| id != target.provider.id)
                {
                    warnings.push("Profile 的聚合模型与所选供应商不匹配，保留项目模型".to_string());
                    return Ok(None);
                }
                return Ok(Some(Some(model.to_string())));
            }
            crate::mode::stack::Resolved::Miss(reason) => {
                warnings.push(reason.message(model));
                return Ok(None);
            }
        }
    }
    let Some(provider_id) = payload.providers.get(app).and_then(Option::as_deref) else {
        if let Some(model) = selected {
            return Ok(Some(Some(model.to_string())));
        }
        // A captured empty side clears only its previously managed model; an
        // uncaptured side leaves project configuration alone.
        return Ok(ProfileScope::for_app(app)
            .is_some_and(|scope| payload.scope_captured(scope))
            .then_some(None));
    };
    let provider = match state.db.read_provider(app, provider_id)? {
        Some(Ok(provider)) => provider,
        Some(Err(_)) | None => {
            warnings.push(format!(
                "所选模型供应商已删除或不可读，项目模型保持不变 ({provider_id})"
            ));
            return Ok(None);
        }
    };

    let mode = crate::mode::current::mode_state(app);
    let stack = crate::mode::state::stack(
        &crate::live::engine::DeviceStore::for_device(),
        app.as_str(),
    )?;
    let stack_member = if mode.is_proxy() && stack.enabled {
        crate::mode::stack::members(state.db.as_ref(), app, &stack, mode.proxy_route.as_deref())?
            .into_iter()
            .find(|member| member.provider.id == provider_id)
    } else {
        None
    };
    let current = crate::mode::current::provider_for(
        state.db.as_ref(),
        app,
        crate::mode::current::Purpose::InUse,
    )?;
    if stack_member.is_none() && current.as_deref() != Some(provider_id) {
        warnings.push(format!(
            "Profile 的模型供应商当前未连接或未加入聚合，项目模型保持不变 ({})",
            provider.name
        ));
        return Ok(None);
    }

    let model = if let Some(model) = selected {
        if let Some(member) = stack_member
            .as_ref()
            .filter(|member| !(app == &AppType::Codex && member.route))
        {
            let upstream =
                crate::proxy::model_mapper::strip_one_m_suffix_for_upstream(model).trim();
            Some(crate::mode::stack::encode(
                app,
                &member.key,
                upstream,
                crate::live::project::claude::has_one_m_marker(model),
            ))
        } else {
            Some(model.to_string())
        }
    } else if let Some(member) =
        stack_member.filter(|member| !(app == &AppType::Codex && member.route))
    {
        member.model_ids.into_iter().next()
    } else {
        match app {
            AppType::Claude => match provider
                .settings_config
                .get("env")
                .and_then(|env| env.get("ANTHROPIC_MODEL"))
            {
                Some(Value::String(model)) => Some(model.clone()),
                Some(_) => {
                    warnings.push(format!(
                        "供应商默认模型格式无效，项目模型保持不变 ({})",
                        provider.name
                    ));
                    return Ok(None);
                }
                None => None,
            },
            AppType::Codex => {
                let config = match provider.settings_config.get("config") {
                    Some(Value::String(config)) => config.as_str(),
                    None => "",
                    Some(_) => {
                        warnings.push(format!(
                            "供应商配置格式无效，项目模型保持不变 ({})",
                            provider.name
                        ));
                        return Ok(None);
                    }
                };
                let config = match config.parse::<toml_edit::DocumentMut>() {
                    Ok(config) => config,
                    Err(_) => {
                        warnings.push(format!(
                            "供应商配置无法解析，项目模型保持不变 ({})",
                            provider.name
                        ));
                        return Ok(None);
                    }
                };
                match config.get("model") {
                    Some(model) => match model.as_str() {
                        Some(model) => Some(model.to_string()),
                        None => {
                            warnings.push(format!(
                                "供应商默认模型格式无效，项目模型保持不变 ({})",
                                provider.name
                            ));
                            return Ok(None);
                        }
                    },
                    None => None,
                }
            }
            _ => None,
        }
    };

    if model.is_none() {
        warnings.push(format!(
            "Profile 中所选供应商没有默认模型，仅清理此前托管的项目模型 ({})",
            provider.name
        ));
    }
    Ok(Some(model))
}

fn desired_project_model_settings(
    state: &AppState,
    payload: &ProfilePayload,
    app: &AppType,
    warnings: &mut Vec<String>,
) -> Result<Option<BTreeMap<String, String>>, AppError> {
    if connection::is_direct(payload, app) {
        let provider = connection::provider(state, payload)?;
        let model = connection::model(payload, &provider)?;
        let mut settings =
            crate::services::profile::provider_model_settings(state, app, &provider.id)?
                .ok_or_else(|| AppError::InvalidInput("供应商的项目模型设置不可读".into()))?;
        settings.extend(payload.model_settings.claude.clone().unwrap_or_default());
        for key in crate::services::profile::CLAUDE_PROFILE_MODEL_KEYS {
            let value = settings.entry((*key).to_string()).or_default();
            if key.ends_with("_MODEL") {
                if value.is_empty() {
                    *value = model.clone();
                }
                connection::validate_model(value)?;
            }
        }
        return Ok(Some(settings));
    }
    let Some(profile_settings) = payload.model_settings.get(app).and_then(Option::as_ref) else {
        return Ok(None);
    };
    let provider_defaults =
        if let Some(provider_id) = payload.providers.get(app).and_then(Option::as_deref) {
            let Some(settings) =
                crate::services::profile::provider_model_settings(state, app, provider_id)?
            else {
                warnings.push(format!(
                    "所选供应商的模型设置已删除或不可读，保留当前项目模型设置 ({provider_id})"
                ));
                return Ok(None);
            };
            settings
        } else {
            BTreeMap::new()
        };
    let mut settings = provider_defaults;
    settings.extend(profile_settings.clone());
    for key in crate::services::profile::profile_model_keys(app) {
        settings.entry((*key).to_string()).or_default();
    }
    Ok(Some(settings))
}

fn desired_codex_model_catalog(
    state: &AppState,
    payload: &ProfilePayload,
    app: &AppType,
    warnings: &mut Vec<String>,
) -> Result<Option<Option<Value>>, AppError> {
    if app != &AppType::Codex {
        return Ok(None);
    }
    let Some(Some(profile_catalog)) = payload.model_catalog.get(app) else {
        return Ok(None);
    };
    let profile_catalog =
        crate::services::profile::normalize_profile_model_catalog(profile_catalog.clone())?;
    let (mut settings, mut config, tool_profile) = if let Some(provider_id) =
        payload.providers.get(app).and_then(Option::as_deref)
    {
        let Some(Ok(provider)) = state.db.read_provider(app, provider_id)? else {
            warnings.push(format!(
                "Codex 供应商已删除或不可读，保留项目模型目录 ({provider_id})"
            ));
            return Ok(None);
        };
        let config = match provider.settings_config.get("config") {
            Some(Value::String(config)) => config
                .parse::<toml_edit::DocumentMut>()
                .map_err(|error| AppError::Config(format!("解析 Codex 供应商配置失败: {error}")))?,
            None => toml_edit::DocumentMut::new(),
            Some(_) => {
                warnings.push(format!(
                    "Codex 供应商 config 格式无效，保留项目模型目录 ({provider_id})"
                ));
                return Ok(None);
            }
        };
        let tool_profile = crate::proxy::providers::resolve_codex_catalog_tool_profile(&provider);
        (provider.settings_config, config, tool_profile)
    } else {
        // Imported Profiles carry their own catalog and keep connection settings
        // in the native project. Generating model rows must not require a global provider.
        (
            serde_json::json!({}),
            toml_edit::DocumentMut::new(),
            crate::codex_config::CodexCatalogToolProfile::NativeResponses,
        )
    };
    settings["modelCatalog"] = profile_catalog;
    if let Some(model_settings) = payload.model_settings.codex.as_ref() {
        for (key, value) in model_settings {
            crate::services::profile::set_codex_model_setting(
                &mut config,
                key,
                (!value.is_empty()).then_some(value),
            )?;
        }
    }
    if let Some(model) = payload.models.codex.as_ref() {
        if model.is_empty() {
            config.remove("model");
        } else {
            config["model"] = toml_edit::value(model.as_str());
        }
    }
    let catalog = crate::codex_config::plan_codex_model_catalog(
        &settings,
        &config.to_string(),
        tool_profile,
    )?
    .catalog;
    Ok(Some(catalog))
}

fn desired_instruction(
    state: &AppState,
    payload: &ProfilePayload,
    app: &AppType,
    warnings: &mut Vec<String>,
) -> Result<Option<Option<String>>, AppError> {
    let Some(prompt_id) = payload.prompts.get(app).and_then(Option::as_deref) else {
        return Ok(None);
    };
    if prompt_id == NO_ACTIVE_PROMPT {
        return Ok(Some(None));
    }
    let prompts = state.db.get_prompts()?;
    match prompts.get(prompt_id) {
        Some(prompt) => Ok(Some(Some(prompt.content.clone()))),
        None => {
            let name = payload
                .resource_names
                .get(app)
                .and_then(|names| names.prompts.get(prompt_id));
            warnings.push(format!(
                "项目指令 Prompt '{}' 已删除，保持项目文件现状",
                resource_label(prompt_id, name)
            ));
            Ok(None)
        }
    }
}

fn text_hash(text: &str) -> String {
    crate::live::engine::sha256_hex(text.as_bytes())
}

fn plan_model_op(
    model_path: &str,
    desired: Option<Option<String>>,
    current: Option<String>,
    manifest: &ProjectManifest,
) -> Option<ModelOperation> {
    plan_model_field_op(
        &format!("{model_path}#model"),
        desired,
        current,
        manifest.model_hash.as_ref(),
    )
}

fn plan_model_setting_ops(
    app: &AppType,
    desired: Option<&BTreeMap<String, String>>,
    settings: Option<&ClaudeSettingsDocument>,
    codex: Option<&ProjectMcpDocument>,
    manifest: &ProjectManifest,
) -> Result<Vec<ModelSettingOperation>, AppError> {
    let Some(desired) = desired else {
        return Ok(Vec::new());
    };
    let mut operations = Vec::new();
    for key in crate::services::profile::profile_model_keys(app) {
        let (path, current) = match app {
            AppType::Claude => {
                let path = format!("{CLAUDE_PROJECT_SETTINGS}#env.{key}");
                let current = settings
                    .map(|settings| settings.env_setting(key))
                    .transpose()?
                    .flatten();
                (path, current)
            }
            AppType::Codex => {
                let path = format!(".codex/config.toml#{key}");
                let current = codex
                    .map(|document| document.model_setting(key))
                    .transpose()?
                    .flatten();
                (path, current)
            }
            _ => continue,
        };
        let value = desired.get(*key).filter(|value| !value.is_empty()).cloned();
        let Some(operation) = plan_model_field_op(
            &path,
            Some(value),
            current,
            manifest.model_settings_hashes.get(&path),
        ) else {
            continue;
        };
        operations.push(ModelSettingOperation {
            path,
            key: (*key).to_string(),
            operation,
        });
    }
    Ok(operations)
}

fn plan_model_catalog_op(
    root: &Path,
    app: &AppType,
    desired: Option<Option<Value>>,
    codex: Option<&ProjectMcpDocument>,
    manifest: &ProjectManifest,
    warnings: &mut Vec<String>,
) -> Result<Option<ModelCatalogOperation>, AppError> {
    let Some(desired) = desired else {
        return Ok(None);
    };
    if app != &AppType::Codex {
        return Ok(None);
    }
    let Some(document) = codex else {
        return Err(AppError::Config(
            "Codex model catalog planning requires project config.toml".into(),
        ));
    };
    let relative_path = ".codex/cc-switch-model-catalog.json";
    let path = root.join(relative_path);
    ensure_project_parent(root, Path::new(".codex"), false)?;
    ensure_regular_target(&path)?;
    let original = read_optional(&path)?;
    let current_hash = original.as_deref().map(crate::live::engine::sha256_hex);
    let desired = match desired {
        Some(catalog) => Some(serde_json::to_vec_pretty(&catalog).map_err(|error| {
            AppError::Config(format!("序列化项目 Codex 模型目录失败: {error}"))
        })?),
        None => None,
    };
    let desired_hash = desired.as_deref().map(crate::live::engine::sha256_hex);
    let conflict = match (&current_hash, manifest.codex_model_catalog_hash.as_ref()) {
        (Some(current), Some(previous)) if current != previous => Some(ProjectDeployConflict {
            path: relative_path.to_string(),
            reason: "项目模型目录在上次部署后被修改".to_string(),
        }),
        (Some(current), None) if desired_hash.as_ref() != Some(current) => {
            Some(ProjectDeployConflict {
                path: relative_path.to_string(),
                reason: "项目中已有同名 Codex 模型目录".to_string(),
            })
        }
        (Some(current), Some(previous)) if desired_hash.is_none() && current == previous => None,
        _ => None,
    };
    let pointer = if desired.is_some() {
        let config_path = root.join(".codex/config.toml");
        let config_dir = config_path
            .parent()
            .ok_or_else(|| AppError::InvalidInput("Codex project config path invalid".into()))?;
        let ProjectMcpDocument::Codex { document, .. } = document else {
            return Err(AppError::Config(
                "Codex project config has wrong document type".into(),
            ));
        };
        let desired_pointer =
            crate::live::project::codex::catalog_pointer_for(config_dir, document);
        if Path::new(&desired_pointer).is_absolute() {
            warnings.push(
                "Codex 项目模型目录指针使用本机绝对路径；将项目复制到其他电脑后需要重新部署 Profile。目录文件位于 .codex/cc-switch-model-catalog.json。"
                    .to_string(),
            );
        }
        plan_model_field_op(
            ".codex/config.toml#model_catalog_json",
            Some(Some(desired_pointer)),
            document
                .get("model_catalog_json")
                .and_then(toml_edit::Item::as_str)
                .map(str::to_string),
            manifest.codex_model_catalog_pointer_hash.as_ref(),
        )
        .expect("catalog pointer is always planned")
    } else {
        let current_pointer = document.model_catalog_pointer();
        let mut operation = plan_model_field_op(
            ".codex/config.toml#model_catalog_json",
            Some(None),
            current_pointer.clone(),
            manifest.codex_model_catalog_pointer_hash.as_ref(),
        )
        .expect("catalog pointer is always planned");
        if current_pointer.is_some() && manifest.codex_model_catalog_pointer_hash.is_none() {
            operation.conflict = Some(ProjectDeployConflict {
                path: ".codex/config.toml#model_catalog_json".into(),
                reason: "项目中已有用户配置的 Codex 模型目录".into(),
            });
            operation.no_op = false;
        }
        operation
    };
    let no_op = current_hash == desired_hash;
    Ok(Some(ModelCatalogOperation {
        path,
        relative_path: relative_path.to_string(),
        desired,
        desired_hash,
        conflict,
        original,
        current_hash,
        no_op,
        pointer,
    }))
}

fn plan_model_field_op(
    field_path: &str,
    desired: Option<Option<String>>,
    current: Option<String>,
    previous_hash: Option<&String>,
) -> Option<ModelOperation> {
    let desired = desired?;
    let desired_hash = desired.as_deref().map(text_hash);
    let current_hash = current.as_deref().map(text_hash);
    let conflict = match (&current_hash, previous_hash) {
        (Some(current), Some(previous)) if current != previous => Some(ProjectDeployConflict {
            path: field_path.to_string(),
            reason: "项目设置在上次部署后被修改".to_string(),
        }),
        (Some(current), None)
            if desired_hash
                .as_ref()
                .is_some_and(|desired| current != desired) =>
        {
            Some(ProjectDeployConflict {
                path: field_path.to_string(),
                reason: "项目中已有不同的设置值".to_string(),
            })
        }
        _ => None,
    };
    Some(ModelOperation {
        desired_hash: desired_hash.clone(),
        desired,
        no_op: (desired_hash.is_none() && previous_hash.is_none()) || current_hash == desired_hash,
        conflict,
    })
}

fn instruction_relative_path(app: &AppType) -> Result<&'static str, AppError> {
    match app {
        AppType::Claude => Ok("CLAUDE.md"),
        AppType::Codex => Ok("AGENTS.md"),
        _ => Err(AppError::InvalidInput(
            "项目指令文件仅支持 Claude Code 和 Codex".to_string(),
        )),
    }
}

fn plan_instruction_op(
    root: &Path,
    app: &AppType,
    desired: Option<Option<String>>,
    manifest: &ProjectManifest,
) -> Result<Option<InstructionOperation>, AppError> {
    let Some(desired) = desired else {
        return Ok(None);
    };
    let relative_path = instruction_relative_path(app)?.to_string();
    let path = root.join(&relative_path);
    ensure_regular_target(&path)?;
    let current = read_optional(&path)?;
    let current_hash = current.as_deref().map(crate::live::engine::sha256_hex);
    let desired_hash = desired
        .as_deref()
        .map(|text| crate::live::engine::sha256_hex(text.as_bytes()));
    let conflict = match (&desired_hash, &current_hash, &manifest.instruction_hash) {
        (Some(_), Some(current), Some(previous)) if current != previous => {
            Some(ProjectDeployConflict {
                path: relative_path.clone(),
                reason: "项目指令文件在上次部署后被修改".to_string(),
            })
        }
        (Some(desired), Some(current), None) if current != desired => Some(ProjectDeployConflict {
            path: relative_path.clone(),
            reason: "项目中已有同名指令文件".to_string(),
        }),
        (None, Some(current), Some(previous)) if current != previous => {
            Some(ProjectDeployConflict {
                path: relative_path.clone(),
                reason: "项目指令文件在上次部署后被修改".to_string(),
            })
        }
        _ => None,
    };
    Ok(Some(InstructionOperation {
        no_op: (desired.is_none() && manifest.instruction_hash.is_none())
            || current_hash == desired_hash,
        desired,
        current_hash,
        desired_hash,
        conflict,
        relative_path,
    }))
}

#[derive(Default)]
struct DesiredMcp {
    entries: HashMap<String, Value>,
    blocked_ids: HashSet<String>,
}

fn load_desired_mcp(
    db: &Database,
    ids: &[String],
    app: &AppType,
    names: &BTreeMap<String, String>,
    aliases: &BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<DesiredMcp, AppError> {
    let rows = db.get_mcp_server_rows()?;
    let mut desired = DesiredMcp::default();
    if app == &AppType::Codex {
        warnings.push(
            "Codex 项目 MCP 会与全局配置合并；此 Profile 未托管的全局 MCP 仍按原生规则继承。"
                .to_string(),
        );
    }
    for id in ids {
        let native_id = aliases.get(id).unwrap_or(id);
        if native_id.is_empty() || native_id.len() > 1024 || native_id.chars().any(char::is_control)
        {
            return Err(AppError::InvalidInput("项目 MCP 名称无效".into()));
        }
        let Some(server) = rows.readable.get(id) else {
            desired.blocked_ids.insert(native_id.clone());
            let unreadable = rows.unreadable.iter().find(|entry| &entry.id == id);
            let label = resource_label(
                id,
                unreadable
                    .map(|entry| &entry.name)
                    .or_else(|| names.get(id)),
            );
            let reason = if unreadable.is_some() {
                "配置不可读"
            } else {
                "已删除"
            };
            warnings.push(format!("MCP '{label}' {reason}，跳过并保留项目现状"));
            continue;
        };
        let mut spec = match project_safe_mcp_spec(&server.server, app) {
            Ok(spec) => spec,
            Err(reason) => {
                desired.blocked_ids.insert(native_id.clone());
                warnings.push(format!("MCP「{}」未部署：{reason}", server.name));
                continue;
            }
        };
        if app == &AppType::Codex {
            // A project selection must override an inherited enabled=false.
            spec["enabled"] = Value::Bool(true);
        }
        if desired.entries.insert(native_id.clone(), spec).is_some() {
            return Err(AppError::InvalidInput(format!(
                "多个 MCP 使用同一项目名称: {native_id}"
            )));
        }
    }
    Ok(desired)
}

fn project_safe_mcp_spec(spec: &Value, app: &AppType) -> Result<Value, String> {
    let mut spec = spec
        .get("server")
        .filter(|value| value.is_object())
        .unwrap_or(spec)
        .clone();
    let Some(object) = spec.as_object_mut() else {
        return Err("服务器配置不是对象".to_string());
    };

    for key in [
        "enabled",
        "source",
        "id",
        "name",
        "description",
        "tags",
        "homepage",
        "docs",
    ] {
        object.remove(key);
    }

    if app == &AppType::Codex
        && object
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| url.contains("${"))
    {
        return Err("Codex 不会在 URL 中展开 ${变量}；请提供固定 URL 并用 bearer_token_env_var 或 env_http_headers 引用凭据".to_string());
    }
    if let Some(env) = object.get("env") {
        let Some(env) = env.as_object() else {
            return Err("env 必须是对象；为避免凭据泄露，已跳过".to_string());
        };
        let has_private_values = env.iter().any(|(name, value)| match value.as_str() {
            Some(value) if is_secret_name(name) => {
                !(value.is_empty() || app == &AppType::Claude && is_env_reference(value))
            }
            Some(_) => contains_inline_secret(value),
            None => true,
        });
        if has_private_values {
            return Err("环境变量含明文凭据或无效值；Claude 请改用 ${变量名}，Codex 请用 env_vars 转发用户环境变量".to_string());
        }
    }

    for key in ["headers", "http_headers"] {
        if let Some(headers) = object.get(key) {
            let Some(headers) = headers.as_object() else {
                return Err(format!(
                    "{key} 必须是对象；为避免把凭据写入项目文件，已跳过"
                ));
            };
            if headers.iter().any(|(name, value)| match value.as_str() {
                Some(value) if is_secret_name(name) => {
                    !(value.is_empty() || app == &AppType::Claude && is_credential_reference(value))
                }
                Some(_) => contains_inline_secret(value),
                None => true,
            }) {
                return Err("HTTP 请求头含明文凭据或无效值；Claude 可用 Bearer ${TOKEN}，Codex 请用 bearer_token_env_var 或 env_http_headers".to_string());
            }
        }
    }

    if contains_inline_secret(&spec) {
        return Err("包含疑似凭据；为避免写入项目文件，已跳过".to_string());
    }
    if app == &AppType::Codex {
        // The shared library uses Claude's header key; Codex's native key is
        // http_headers. Explicit Codex headers take precedence on duplicate keys.
        if let Some(Value::Object(headers)) = spec
            .as_object_mut()
            .and_then(|object| object.remove("headers"))
        {
            let object = spec.as_object_mut().expect("validated MCP object");
            let native = object
                .entry("http_headers")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(native) = native.as_object_mut() {
                for (key, value) in headers {
                    native.entry(key).or_insert(value);
                }
            }
        }
    }
    Ok(spec)
}

fn is_env_reference(value: &str) -> bool {
    let Some(inner) = value
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
    else {
        return false;
    };
    is_env_name(inner)
}

fn is_credential_reference(value: &str) -> bool {
    is_env_reference(value)
        || ["Bearer ", "Basic "]
            .iter()
            .any(|prefix| value.strip_prefix(prefix).is_some_and(is_env_reference))
}

fn is_secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace('-', "_");
    if name.ends_with("_key") {
        return true;
    }
    [
        "api_key",
        "apikey",
        "access_key",
        "access_token",
        "auth_token",
        "token",
        "password",
        "secret",
        "credential",
        "authorization",
        "auth",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

fn has_sensitive_env_default(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find('}') else {
            break;
        };
        if rest[..end]
            .split_once(":-")
            .is_some_and(|(name, fallback)| {
                !fallback.is_empty() && (is_secret_name(name) || url_contains_credentials(fallback))
            })
        {
            return true;
        }
        rest = &rest[end + 1..];
    }
    false
}

fn argument_has_inline_secret(text: &str) -> bool {
    text.split_whitespace().any(|argument| {
        let Some((flag, value)) = argument.split_once('=') else {
            return false;
        };
        // A whole URL is checked as a URL, not as a key=value flag.
        if flag.contains("://") {
            return false;
        }
        if value.contains("://") && url_contains_credentials(value) {
            return true;
        }
        let flag = flag
            .trim_start_matches('-')
            .to_ascii_lowercase()
            .replace('-', "_");
        [
            "api_key",
            "apikey",
            "access_token",
            "auth_token",
            "token",
            "password",
            "secret",
            "auth",
        ]
        .iter()
        .any(|marker| flag.contains(marker))
            && !value.is_empty()
            && !is_env_reference(value)
    })
}

fn contains_inline_secret(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let key_lower = key.to_ascii_lowercase().replace('-', "_");
            if key_lower == "bearer_token_env_var" {
                return !value.as_str().is_some_and(is_env_name);
            }
            if key_lower == "env_http_headers" {
                return !value.as_object().is_some_and(|headers| {
                    headers
                        .values()
                        .all(|value| value.as_str().is_some_and(is_env_name))
                });
            }
            if key_lower == "url" && url_contains_credentials(value.as_str().unwrap_or_default()) {
                return true;
            }
            let secret_key = is_secret_name(&key_lower);
            if secret_key {
                return match value {
                    Value::String(value) => !value.is_empty() && !is_credential_reference(value),
                    Value::Object(value) => !value.is_empty(),
                    Value::Array(value) => !value.is_empty(),
                    Value::Null => false,
                    _ => true,
                };
            }
            contains_inline_secret(value)
        }),
        Value::Array(items) => {
            items.iter().any(contains_inline_secret)
                || items.windows(2).any(|pair| {
                    let flag = pair[0].as_str().unwrap_or_default().to_ascii_lowercase();
                    let value = pair[1].as_str().unwrap_or_default();
                    ["--api-key", "--token", "--password", "--secret", "--auth"]
                        .iter()
                        .any(|marker| flag.contains(marker))
                        && !value.is_empty()
                        && !is_env_reference(value)
                })
        }
        Value::String(text) => {
            if is_credential_reference(text) {
                return false;
            }
            if url::Url::parse(text).is_ok() && text.contains("://") {
                return url_contains_credentials(text) || has_sensitive_env_default(text);
            }
            let lower = text.to_ascii_lowercase();
            (text.contains("://") && url_contains_credentials(text))
                || has_sensitive_env_default(text)
                || argument_has_inline_secret(text)
                || [
                    "sk-",
                    "ghp_",
                    "gho_",
                    "bearer ",
                    "access_token=",
                    "api_key=",
                ]
                .iter()
                .any(|marker| lower.contains(marker))
        }
        _ => false,
    }
}

fn is_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn url_contains_credentials(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if (!url.username().is_empty() && !is_env_reference(url.username()))
        || url
            .password()
            .is_some_and(|password| !is_env_reference(password))
    {
        return true;
    }
    url.query_pairs().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        ["token", "key", "secret", "password", "auth", "credential"]
            .iter()
            .any(|marker| key.contains(marker))
            && !value.is_empty()
            && !is_env_reference(&value)
    })
}

#[derive(Default)]
struct DesiredSkills {
    entries: HashMap<String, DesiredSkill>,
    blocked_ids: HashSet<String>,
}

struct DesiredSkill {
    id: String,
    directory: String,
    source: PathBuf,
    hash: String,
}

fn load_desired_skills(
    db: &Database,
    ids: &[String],
    names: &BTreeMap<String, String>,
    aliases: &BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<DesiredSkills, AppError> {
    let installed = db.get_all_installed_skills()?;
    let mut desired = DesiredSkills::default();
    for id in ids {
        let Some(skill) = installed.get(id) else {
            desired.blocked_ids.insert(id.clone());
            warnings.push(format!(
                "Skill '{}' 已删除，跳过并保留项目现状",
                resource_label(id, names.get(id))
            ));
            continue;
        };
        let source = SkillService::get_installed_skill_dir(&skill.directory)
            .map_err(|err| AppError::Config(format!("解析 Skill 目录失败: {err}")))?;
        let hash = match hash_tree(&source, true) {
            Ok(hash) => hash,
            Err(err) => {
                desired.blocked_ids.insert(id.clone());
                warnings.push(format!("Skill '{}' 未部署：{err}", skill.name));
                continue;
            }
        };
        if !source.join("SKILL.md").is_file() {
            desired.blocked_ids.insert(id.clone());
            warnings.push(format!("Skill '{}' 缺少 SKILL.md，已跳过", skill.name));
            continue;
        }
        desired.entries.insert(
            id.clone(),
            DesiredSkill {
                id: id.clone(),
                directory: aliases.get(id).unwrap_or(&skill.directory).clone(),
                source,
                hash,
            },
        );
    }
    Ok(desired)
}

fn plan_claude_skill_overrides(
    root: &Path,
    settings: &ClaudeSettingsDocument,
    manifest: &ProjectManifest,
    db: &Database,
    desired_skills: &DesiredSkills,
    selected_ids: &[String],
    warnings: &mut Vec<String>,
) -> Result<Vec<SkillOverrideOperation>, AppError> {
    let current = match settings.root.get("skillOverrides") {
        Some(value) => Some(value.as_object().ok_or_else(|| {
            AppError::Config("Claude 项目 skillOverrides 必须是对象".to_string())
        })?),
        None => None,
    };
    let installed = db.get_all_installed_skills()?;
    let selected = selected_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let previous_ids = manifest
        .claude_skill_overrides
        .values()
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let mut blocked_ids = desired_skills.blocked_ids.clone();
    let mut desired = BTreeMap::<String, (String, String)>::new();
    for id in &previous_ids {
        if !installed.contains_key(*id) && blocked_ids.insert((*id).to_string()) {
            warnings.push(format!(
                "Skill '{id}' 已删除，保留此前的项目 skillOverrides"
            ));
        }
    }
    for skill in installed.values() {
        if (!skill.apps.claude
            && !selected.contains(skill.id.as_str())
            && !previous_ids.contains(skill.id.as_str()))
            || blocked_ids.contains(&skill.id)
        {
            continue;
        }
        let source = match SkillService::get_installed_skill_dir(&skill.directory) {
            Ok(source) => source,
            Err(error) => {
                blocked_ids.insert(skill.id.clone());
                warnings.push(format!(
                    "Skill '{}' 无法读取，保留项目 skillOverrides：{error}",
                    skill.name
                ));
                continue;
            }
        };
        let skill_md = source.join("SKILL.md");
        if (!desired_skills.entries.contains_key(&skill.id) && hash_tree(&source, true).is_err())
            || fs::read_to_string(&skill_md).is_err()
        {
            blocked_ids.insert(skill.id.clone());
            warnings.push(format!(
                "Skill '{}' 的源文件不可读，保留项目 skillOverrides",
                skill.name
            ));
            continue;
        }
        // Claude 2.1.326 matches skillOverrides against the installed invocation
        // ID (directory), even when frontmatter declares a different display alias.
        let name = desired_skills
            .entries
            .get(&skill.id)
            .map(|desired| desired.directory.as_str())
            .unwrap_or(&skill.directory);
        if name.trim().is_empty() || name.contains(':') {
            blocked_ids.insert(skill.id.clone());
            warnings.push(format!(
                "Skill '{}' 的名称不可用于项目 skillOverrides，已跳过",
                skill.name
            ));
            continue;
        }
        let mode = if selected.contains(skill.id.as_str()) {
            "on"
        } else if skill.apps.claude {
            "off"
        } else {
            // This previously managed resource no longer needs a project override.
            continue;
        };
        if let Some((other_id, other_mode)) = desired.get(name) {
            if other_id != &skill.id {
                warnings.push(format!("多个 Skill 使用相同调用名 '{name}'；Claude 按调用名控制，项目选中的同名 Skill 将优先启用"));
            }
            if other_mode == "on" {
                continue;
            }
        }
        desired.insert(name.to_string(), (skill.id.clone(), mode.to_string()));
    }
    let mut names = desired.keys().cloned().collect::<HashSet<_>>();
    names.extend(
        manifest
            .claude_skill_overrides
            .iter()
            .filter_map(|(name, entry)| {
                (!blocked_ids.contains(&entry.id) && !name.contains(':') && !name.trim().is_empty())
                    .then_some(name.clone())
            }),
    );
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort();
    // Native overrides address names, not paths. Do not change an unregistered
    // hand-written skill that happens to share a managed resource's name.
    let global_directories = installed
        .values()
        .filter(|skill| skill.apps.claude)
        .map(|skill| skill.directory.clone())
        .collect::<HashSet<_>>();
    let mut project_directories = desired_skills
        .entries
        .values()
        .map(|skill| skill.directory.clone())
        .collect::<HashSet<_>>();
    project_directories.extend(manifest.skills.keys().filter_map(|path| {
        Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }));
    let mut protected_names = HashSet::new();
    let global = SkillService::get_app_skills_dir(&AppType::Claude)
        .map_err(|error| AppError::Config(error.to_string()))?;
    for (base, known_directories) in [
        (global, global_directories),
        (root.join(".claude/skills"), project_directories),
    ] {
        let entries = match fs::read_dir(&base) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                protected_names.extend(names.iter().cloned());
                warnings
                    .push("无法核对未登记的 Claude Skills，保留项目 skillOverrides".to_string());
                continue;
            }
        };
        for entry in entries.flatten() {
            let directory = entry.file_name().to_string_lossy().into_owned();
            let skill_md = entry.path().join("SKILL.md");
            if known_directories.contains(&directory) || !skill_md.is_file() {
                continue;
            }
            if names.contains(&directory) && protected_names.insert(directory.clone()) {
                warnings.push(format!("Skill '{directory}' 与未登记手写技能同名，Claude 无法按路径区分；保留项目 skillOverrides"));
            }
        }
    }
    let operations = names
        .into_iter()
        .filter_map(|name| {
            let previous = manifest.claude_skill_overrides.get(&name);
            if protected_names.contains(&name)
                || previous.is_some_and(|entry| blocked_ids.contains(&entry.id))
            {
                return None;
            }
            let target = desired.get(&name);
            let mode = target.map(|(_, mode)| mode.clone());
            let desired_hash = mode
                .as_ref()
                .map(|mode| text_hash(&Value::String(mode.clone()).to_string()));
            let current_hash = current
                .and_then(|values| values.get(&name))
                .map(|value| text_hash(&value.to_string()));
            let previous_hash = previous.map(|entry| &entry.hash);
            let reason = match (&current_hash, previous_hash) {
                (Some(current), Some(previous)) if current != previous => {
                    Some("该 Skill 开关在上次部署后被修改")
                }
                (None, Some(_)) if desired_hash.is_some() => {
                    Some("该 Skill 开关在上次部署后被删除")
                }
                (Some(current), None)
                    if desired_hash
                        .as_ref()
                        .is_some_and(|desired| current != desired) =>
                {
                    Some("项目中已有不同的 Skill 开关")
                }
                _ => None,
            };
            Some(SkillOverrideOperation {
                id: target
                    .map(|(id, _)| id.clone())
                    .unwrap_or_else(|| manifest.claude_skill_overrides[&name].id.clone()),
                name: name.clone(),
                desired: mode,
                desired_hash: desired_hash.clone(),
                conflict: reason.map(|reason| ProjectDeployConflict {
                    path: format!(".claude/settings.local.json#skillOverrides.{name}"),
                    reason: reason.to_string(),
                }),
                no_op: current_hash == desired_hash,
            })
        })
        .collect::<Vec<_>>();
    Ok(operations)
}

fn plan_mcp_ops(
    document: &ProjectMcpDocument,
    manifest: &ProjectManifest,
    desired: &HashMap<String, Value>,
    blocked_ids: &HashSet<String>,
    db: &Database,
    warnings: &mut Vec<String>,
) -> Result<Vec<McpOperation>, AppError> {
    let mut ids = desired.keys().cloned().collect::<HashSet<_>>();
    ids.extend(
        manifest
            .mcp_servers
            .keys()
            .filter(|id| !blocked_ids.contains(*id))
            .cloned(),
    );
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort();

    ids.into_iter()
        .map(|id| {
            let prior_hash = manifest.mcp_servers.get(&id);
            let mut desired_value = desired.get(&id).cloned();
            if desired_value.is_none() && prior_hash.is_some() {
                if let ProjectMcpDocument::Codex { document, .. } = document {
                    // Deleting a project table would reactivate its global counterpart.
                    // Keep the full launcher/transport and own an explicit disabled entry.
                    desired_value = codex_entry_value(document, &id)?;
                    if desired_value.is_none() {
                        let rows = db.get_mcp_server_rows()?;
                        if let Some(server) = rows.readable.get(&id) {
                            match project_safe_mcp_spec(&server.server, &AppType::Codex) {
                                Ok(spec) => desired_value = Some(spec),
                                Err(_) => warnings.push(format!(
                                    "MCP '{id}' 的项目条目已移除，资源配置无法安全恢复禁用项；全局同名配置仍可能继承。"
                                )),
                            }
                        } else {
                            warnings.push(format!(
                                "MCP '{id}' 的项目条目和资源定义均不可用，无法创建禁用项；全局同名配置仍可能继承。"
                            ));
                        }
                    }
                    if let Some(value) = desired_value.as_mut() {
                        let object = value.as_object_mut().ok_or_else(|| {
                            AppError::Config(format!("Codex MCP 条目 '{id}' 必须是表"))
                        })?;
                        object.insert("enabled".to_string(), Value::Bool(false));
                    }
                }
            }
            let desired_hash = desired_value
                .as_ref()
                .map(|value| document.hash_after_set(&id, value))
                .transpose()?;
            let current_hash = document.entry_hash(&id)?;
            let conflict = match (&desired_value, &current_hash, prior_hash) {
                (Some(_), Some(current), Some(prior)) if current != prior => {
                    Some(mcp_conflict(document, &id, "该条目在上次部署后被修改"))
                }
                (Some(_), Some(current), None) if desired_hash.as_ref() != Some(current) => {
                    Some(mcp_conflict(document, &id, "项目中已有同名 MCP 条目"))
                }
                (None, Some(current), Some(prior)) if current != prior => {
                    Some(mcp_conflict(document, &id, "该条目在上次部署后被修改"))
                }
                _ => None,
            };
            let no_op = desired_value.is_some() && desired_hash.as_ref() == current_hash.as_ref();
            Ok(McpOperation {
                id,
                desired: desired_value,
                desired_hash,
                current_hash,
                conflict,
                no_op,
            })
        })
        .collect()
}

fn mcp_conflict(document: &ProjectMcpDocument, id: &str, reason: &str) -> ProjectDeployConflict {
    let file = match document {
        ProjectMcpDocument::Claude { .. } => ".mcp.json",
        ProjectMcpDocument::Codex { .. } => ".codex/config.toml",
    };
    ProjectDeployConflict {
        path: format!("{file}#mcp_servers.{id}"),
        reason: reason.to_string(),
    }
}

fn apply_mcp_ops(
    document: &mut ProjectMcpDocument,
    manifest: &mut ProjectManifest,
    operations: &[McpOperation],
    resolution: Option<ConflictResolution>,
    root: &Path,
    app_name: &str,
    changed: &mut Vec<String>,
) -> Result<(), AppError> {
    for operation in operations {
        if operation.conflict.is_some() {
            match resolution {
                Some(ConflictResolution::KeepCurrent) => {
                    manifest.mcp_servers.remove(&operation.id);
                    save_manifest(root, app_name, manifest)?;
                    continue;
                }
                Some(ConflictResolution::Overwrite) => {}
                Some(ConflictResolution::Skip) | None => continue,
            }
        }
        if operation.no_op {
            if operation.conflict.is_some()
                && matches!(resolution, Some(ConflictResolution::Overwrite))
            {
                if let Some(hash) = operation.desired_hash.as_ref() {
                    manifest
                        .mcp_servers
                        .insert(operation.id.clone(), hash.clone());
                }
            }
            if operation.desired.is_none() {
                manifest.mcp_servers.remove(&operation.id);
            }
            save_manifest(root, app_name, manifest)?;
            continue;
        }
        match &operation.desired {
            Some(value) => {
                document.set_entry(&operation.id, value)?;
                let hash = document
                    .entry_hash(&operation.id)?
                    .or_else(|| operation.desired_hash.clone())
                    .ok_or_else(|| AppError::Config("部署后读取 MCP 条目失败".to_string()))?;
                manifest.mcp_servers.insert(operation.id.clone(), hash);
            }
            None => {
                document.remove_entry(&operation.id);
                manifest.mcp_servers.remove(&operation.id);
            }
        }
        // Record intended ownership before changing project files. A crash or write
        // failure then leaves a detectable mismatch instead of an untracked overwrite.
        save_manifest(root, app_name, manifest)?;
        if operation.current_hash != operation.desired_hash {
            changed.push(format!("MCP {}", operation.id));
        }
    }
    save_manifest(root, app_name, manifest)?;
    Ok(())
}

fn plan_skill_ops(
    root: &Path,
    app: &AppType,
    manifest: &ProjectManifest,
    desired: &DesiredSkills,
) -> Result<Vec<SkillOperation>, AppError> {
    let mut desired_paths = HashMap::new();
    let mut destination_keys = HashSet::new();
    for skill in desired.entries.values() {
        let relative_path = skill_relative_path(app, &skill.directory)?
            .to_string_lossy()
            .replace('\\', "/");
        #[cfg(windows)]
        let destination_key = relative_path.to_lowercase();
        #[cfg(not(windows))]
        let destination_key = relative_path.clone();
        if !destination_keys.insert(destination_key) {
            return Err(AppError::InvalidInput(format!(
                "多个 Skill 使用同一项目目录: {relative_path}"
            )));
        }
        desired_paths.insert(relative_path, skill);
    }
    #[cfg(windows)]
    for path in manifest.skills.keys() {
        if !desired_paths.contains_key(path) && destination_keys.contains(&path.to_lowercase()) {
            // Otherwise one operation replaces the directory and another removes
            // the same Windows directory under its previous spelling.
            return Err(AppError::InvalidInput(format!(
                "Skill 项目目录与此前部署的大小写冲突: {path}"
            )));
        }
    }

    let mut paths = desired_paths.keys().cloned().collect::<HashSet<_>>();
    paths.extend(manifest.skills.iter().filter_map(|(path, skill)| {
        (!desired.blocked_ids.contains(&skill.id)).then_some(path.clone())
    }));
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort();

    paths
        .into_iter()
        .map(|relative| {
            let parent = Path::new(&relative).parent().unwrap_or(Path::new("."));
            ensure_project_parent(root, parent, false)?;
            let destination = root.join(&relative);
            let current_state = path_state(&destination)?;
            let managed = manifest.skills.get(&relative);
            let desired_skill = desired_paths.get(&relative).copied();
            let desired_hash = desired_skill.map(|skill| skill.hash.clone());
            let conflict = match (desired_skill, &current_state, managed) {
                (Some(_), Some(current), Some(prior)) if current != &prior.hash => {
                    Some(skill_conflict(&relative, "Skill 目录在上次部署后被修改"))
                }
                (Some(_), Some(current), None) if desired_hash.as_ref() != Some(current) => {
                    Some(skill_conflict(&relative, "项目中已有同名 Skill 目录"))
                }
                (None, Some(current), Some(prior)) if current != &prior.hash => {
                    Some(skill_conflict(&relative, "Skill 目录在上次部署后被修改"))
                }
                _ => None,
            };
            let id = desired_skill
                .map(|skill| skill.id.clone())
                .or_else(|| managed.map(|skill| skill.id.clone()))
                .unwrap_or_default();
            let no_op = desired_skill.is_some() && desired_hash.as_ref() == current_state.as_ref()
                || desired_skill.is_none() && current_state.is_none();
            Ok(SkillOperation {
                relative_path: relative,
                id,
                source: desired_skill.map(|skill| skill.source.clone()),
                desired_hash,
                current_state,
                conflict,
                no_op,
            })
        })
        .collect()
}

fn skill_conflict(path: &str, reason: &str) -> ProjectDeployConflict {
    ProjectDeployConflict {
        path: path.to_string(),
        reason: reason.to_string(),
    }
}

fn apply_skill_ops(
    root: &Path,
    manifest: &mut ProjectManifest,
    operations: &[SkillOperation],
    resolution: Option<ConflictResolution>,
    app_name: &str,
    changed: &mut Vec<String>,
    undo: &mut Vec<SkillUndo>,
) -> Result<(), AppError> {
    for operation in operations {
        if operation.conflict.is_some() {
            match resolution {
                Some(ConflictResolution::KeepCurrent) => {
                    manifest.skills.remove(&operation.relative_path);
                    save_manifest(root, app_name, manifest)?;
                    continue;
                }
                Some(ConflictResolution::Overwrite) => {}
                Some(ConflictResolution::Skip) | None => continue,
            }
        }
        if operation.no_op {
            if operation.conflict.is_some()
                && matches!(resolution, Some(ConflictResolution::Overwrite))
            {
                if let Some(hash) = operation.desired_hash.as_ref() {
                    manifest.skills.insert(
                        operation.relative_path.clone(),
                        ManagedSkill {
                            id: operation.id.clone(),
                            hash: hash.clone(),
                        },
                    );
                }
            }
            if operation.source.is_none() {
                manifest.skills.remove(&operation.relative_path);
            }
            save_manifest(root, app_name, manifest)?;
            continue;
        }
        let destination = root.join(&operation.relative_path);
        ensure_project_parent(
            root,
            Path::new(&operation.relative_path)
                .parent()
                .unwrap_or(Path::new(".")),
            true,
        )?;
        verify_path_state(&destination, operation.current_state.as_deref())?;
        let previous = manifest.skills.get(&operation.relative_path).cloned();
        match (&operation.source, &operation.desired_hash) {
            (Some(_), Some(hash)) => {
                manifest.skills.insert(
                    operation.relative_path.clone(),
                    ManagedSkill {
                        id: operation.id.clone(),
                        hash: hash.clone(),
                    },
                );
            }
            (None, _) => {
                manifest.skills.remove(&operation.relative_path);
            }
            _ => {}
        }
        if let Err(error) = save_manifest(root, app_name, manifest) {
            restore_skill_manifest_entry(manifest, &operation.relative_path, previous);
            return Err(error);
        }
        let operation_result = (|| -> Result<(), AppError> {
            match &operation.source {
                Some(source) => {
                    let desired_hash = operation
                        .desired_hash
                        .as_deref()
                        .ok_or_else(|| AppError::Config("缺少项目 Skill 目标指纹".to_string()))?;
                    let backup = replace_skill_directory(
                        source,
                        &destination,
                        Some(desired_hash),
                        operation.current_state.as_deref(),
                    )?;
                    undo.push(SkillUndo {
                        destination: destination.clone(),
                        backup,
                        applied_state: Some(desired_hash.to_string()),
                    });
                    verify_path_state(&destination, Some(desired_hash))?;
                    manifest.skills.insert(
                        operation.relative_path.clone(),
                        ManagedSkill {
                            id: operation.id.clone(),
                            hash: desired_hash.to_string(),
                        },
                    );
                }
                None => {
                    if operation.current_state.is_some() {
                        verify_path_state(&destination, operation.current_state.as_deref())?;
                        let backup = backup_path(&destination);
                        fs::rename(&destination, &backup)
                            .map_err(|err| AppError::io(&destination, err))?;
                        undo.push(SkillUndo {
                            destination: destination.clone(),
                            backup: Some(backup.clone()),
                            applied_state: None,
                        });
                        // Verify the renamed directory before deleting its backup;
                        // a concurrent edit must be restored by the batch rollback.
                        verify_path_state(&backup, operation.current_state.as_deref())?;
                    }
                    manifest.skills.remove(&operation.relative_path);
                }
            }
            Ok(())
        })();
        if let Err(error) = operation_result {
            restore_skill_manifest_entry(manifest, &operation.relative_path, previous);
            if let Err(restore_error) = save_manifest(root, app_name, manifest) {
                return Err(AppError::Config(format!(
                    "项目 Skill 部署失败: {error}; 恢复部署清单也失败: {restore_error}"
                )));
            }
            return Err(error);
        }
        changed.push(operation.relative_path.clone());
    }
    Ok(())
}

fn restore_skill_manifest_entry(
    manifest: &mut ProjectManifest,
    path: &str,
    previous: Option<ManagedSkill>,
) {
    match previous {
        Some(previous) => {
            manifest.skills.insert(path.to_string(), previous);
        }
        None => {
            manifest.skills.remove(path);
        }
    }
}

fn backup_path(destination: &Path) -> PathBuf {
    let parent = destination.parent().unwrap_or(Path::new("."));
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill");
    parent.join(format!(".{name}.cc-switch-{}.bak", uuid::Uuid::new_v4()))
}

fn rollback_skill_ops(undo: &[SkillUndo]) -> Vec<String> {
    let mut errors = Vec::new();
    for operation in undo.iter().rev() {
        if let Err(error) =
            verify_path_state(&operation.destination, operation.applied_state.as_deref())
        {
            errors.push(format!(
                "未覆盖部署期间变化的 Skill {}: {error}{}",
                operation.destination.display(),
                operation
                    .backup
                    .as_ref()
                    .map(|path| format!("；原目录保留在 {}", path.display()))
                    .unwrap_or_default()
            ));
            continue;
        }
        if let Err(error) = remove_project_path(&operation.destination) {
            errors.push(format!(
                "移除失败的 Skill {} 时出错: {error}{}",
                operation.destination.display(),
                operation
                    .backup
                    .as_ref()
                    .map(|path| format!("；原目录保留在 {}", path.display()))
                    .unwrap_or_default()
            ));
            continue;
        }
        if let Some(backup) = operation.backup.as_ref() {
            if let Err(error) = fs::rename(backup, &operation.destination) {
                errors.push(format!(
                    "恢复 Skill {} 失败，原目录保留在 {}: {error}",
                    operation.destination.display(),
                    backup.display()
                ));
            }
        }
    }
    errors
}

fn apply_claude_skill_overrides(
    root: &Path,
    settings: &mut ClaudeSettingsDocument,
    manifest: &mut ProjectManifest,
    operations: &[SkillOverrideOperation],
    resolution: Option<ConflictResolution>,
    changed: &mut Vec<String>,
    undo: &mut Vec<ProjectFileUndo>,
) -> Result<(), AppError> {
    if operations.is_empty() || matches!(resolution, Some(ConflictResolution::Skip)) {
        return Ok(());
    }
    let original = read_optional(&settings.path)?;
    for operation in operations {
        if operation.conflict.is_some() {
            match resolution {
                Some(ConflictResolution::KeepCurrent) => {
                    manifest.claude_skill_overrides.remove(&operation.name);
                    continue;
                }
                Some(ConflictResolution::Overwrite) => {}
                _ => continue,
            }
        }
        if operation.no_op
            && !manifest
                .claude_skill_overrides
                .contains_key(&operation.name)
            && operation.conflict.is_none()
        {
            continue; // Matching hand-written fields remain owned by the user.
        }
        if !operation.no_op {
            let settings_root = settings.root.as_object_mut().ok_or_else(|| {
                AppError::Config("Claude 项目 settings.local.json 根值必须是对象".to_string())
            })?;
            match operation.desired.as_ref() {
                Some(mode) => {
                    let values = settings_root
                        .entry("skillOverrides")
                        .or_insert_with(|| Value::Object(Map::new()))
                        .as_object_mut()
                        .ok_or_else(|| {
                            AppError::Config("Claude 项目 skillOverrides 必须是对象".to_string())
                        })?;
                    values.insert(operation.name.clone(), Value::String(mode.clone()));
                }
                None => {
                    if let Some(values) = settings_root
                        .get_mut("skillOverrides")
                        .and_then(Value::as_object_mut)
                    {
                        values.remove(&operation.name);
                    }
                }
            }
            settings.changed = true;
            changed.push(format!(
                ".claude/settings.local.json#skillOverrides.{}",
                operation.name
            ));
        }
        match operation.desired_hash.as_ref() {
            Some(hash) => {
                manifest.claude_skill_overrides.insert(
                    operation.name.clone(),
                    ManagedSkill {
                        id: operation.id.clone(),
                        hash: hash.clone(),
                    },
                );
            }
            None => {
                manifest.claude_skill_overrides.remove(&operation.name);
            }
        }
    }
    save_manifest(root, "claude", manifest)?;
    if let Some(applied_hash) = settings.save(root)? {
        undo.push(ProjectFileUndo {
            path: settings.path.clone(),
            original,
            applied_hash: Some(applied_hash),
        });
        changed.push(".claude/settings.local.json".to_string());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_model_op(
    root: &Path,
    app: &AppType,
    app_name: &str,
    operation: Option<&ModelOperation>,
    env_operation: Option<&ModelOperation>,
    model_settings: &[ModelSettingOperation],
    catalog: Option<&ModelCatalogOperation>,
    resolution: Option<ConflictResolution>,
    settings_doc: Option<&mut ClaudeSettingsDocument>,
    mcp_doc: Option<&mut ProjectMcpDocument>,
    manifest: &mut ProjectManifest,
    changed: &mut Vec<String>,
    undo: &mut Vec<ProjectFileUndo>,
) -> Result<(), AppError> {
    match app {
        AppType::Claude => {
            let settings = settings_doc.ok_or_else(|| {
                AppError::Config("缺少 Claude 项目 settings.local.json".to_string())
            })?;
            // Both fields select the same effective model. Keeping a conflict
            // must not preserve `model` but then override it through a new env value.
            let keep_model = matches!(resolution, Some(ConflictResolution::KeepCurrent))
                && (operation.is_some_and(|operation| operation.conflict.is_some())
                    || env_operation.is_some_and(|operation| operation.conflict.is_some()));
            if keep_model {
                manifest.model_hash = None;
                manifest.claude_env_model_hash = None;
            }
            let path = settings.path.clone();
            let original = read_optional(&path)?;
            if !keep_model {
                if let Some(operation) = operation {
                    if stage_model_field(operation, resolution, &mut manifest.model_hash) {
                        settings.set_model(operation.desired.as_deref())?;
                        changed.push(".claude/settings.local.json#model".to_string());
                    }
                }
                if let Some(env_operation) = env_operation {
                    if stage_model_field(
                        env_operation,
                        resolution,
                        &mut manifest.claude_env_model_hash,
                    ) {
                        settings.set_env_model(env_operation.desired.as_deref())?;
                        changed.push(".claude/settings.local.json#env.ANTHROPIC_MODEL".to_string());
                    }
                }
            }
            for item in model_settings {
                let mut managed_hash = manifest.model_settings_hashes.remove(&item.path);
                if stage_model_field(&item.operation, resolution, &mut managed_hash) {
                    settings.set_env_setting(&item.key, item.operation.desired.as_deref())?;
                    changed.push(item.path.clone());
                }
                if let Some(hash) = managed_hash {
                    manifest
                        .model_settings_hashes
                        .insert(item.path.clone(), hash);
                }
            }
            if catalog.is_some() {
                return Err(AppError::InvalidInput(
                    "Codex model catalog cannot be deployed to Claude settings".into(),
                ));
            }
            save_manifest(root, app_name, manifest)?;
            if let Some(applied_hash) = settings.save(root)? {
                undo.push(ProjectFileUndo {
                    path,
                    original,
                    applied_hash: Some(applied_hash),
                });
                changed.push(".claude/settings.local.json".to_string());
            }
        }
        AppType::Codex => {
            let document = mcp_doc
                .ok_or_else(|| AppError::Config("缺少 Codex 项目 config.toml".to_string()))?;
            if let Some(operation) = operation {
                if stage_model_field(operation, resolution, &mut manifest.model_hash) {
                    document.set_model(operation.desired.as_deref())?;
                    changed.push(".codex/config.toml#model".to_string());
                }
            }
            for item in model_settings {
                let mut managed_hash = manifest.model_settings_hashes.remove(&item.path);
                if stage_model_field(&item.operation, resolution, &mut managed_hash) {
                    document.set_model_setting(&item.key, item.operation.desired.as_ref())?;
                    changed.push(item.path.clone());
                }
                if let Some(hash) = managed_hash {
                    manifest
                        .model_settings_hashes
                        .insert(item.path.clone(), hash);
                }
            }
            if let Some(catalog) = catalog {
                apply_codex_model_catalog(
                    root, catalog, resolution, document, manifest, changed, undo,
                )?;
            }
            save_manifest(root, app_name, manifest)?;
        }
        _ => {
            return Err(AppError::InvalidInput(
                "项目模型仅支持 Claude Code 和 Codex".to_string(),
            ))
        }
    }
    Ok(())
}

fn apply_codex_model_catalog(
    root: &Path,
    operation: &ModelCatalogOperation,
    resolution: Option<ConflictResolution>,
    document: &mut ProjectMcpDocument,
    manifest: &mut ProjectManifest,
    changed: &mut Vec<String>,
    undo: &mut Vec<ProjectFileUndo>,
) -> Result<(), AppError> {
    if matches!(resolution, Some(ConflictResolution::Skip)) {
        return Ok(());
    }
    let keep_current = matches!(resolution, Some(ConflictResolution::KeepCurrent))
        && (operation.conflict.is_some() || operation.pointer.conflict.is_some());
    if keep_current {
        manifest.codex_model_catalog_hash = None;
        manifest.codex_model_catalog_pointer_hash = None;
        return Ok(());
    }
    if operation.conflict.is_some() && !matches!(resolution, Some(ConflictResolution::Overwrite)) {
        return Ok(());
    }
    let desired_pointer = operation.pointer.desired.as_deref();
    if stage_model_field(
        &operation.pointer,
        resolution,
        &mut manifest.codex_model_catalog_pointer_hash,
    ) {
        document.set_model_catalog_pointer(desired_pointer)?;
        changed.push(".codex/config.toml#model_catalog_json".into());
    }

    if operation.conflict.is_some() && !matches!(resolution, Some(ConflictResolution::Overwrite)) {
        return Ok(());
    }
    let current = read_optional(&operation.path)?;
    let current_hash = current.as_deref().map(crate::live::engine::sha256_hex);
    if current_hash != operation.current_hash {
        return Err(AppError::InvalidInput(format!(
            "项目模型目录在部署期间发生变化，请重试: {}",
            operation.path.display()
        )));
    }
    if operation.desired_hash != current_hash {
        match operation.desired.as_deref() {
            Some(bytes) => {
                ensure_project_parent(root, Path::new(".codex"), true)?;
                write_if_unchanged(&operation.path, operation.current_hash.as_deref(), bytes)?;
            }
            None => {
                ensure_regular_target(&operation.path)?;
                if operation.current_hash.is_some() {
                    fs::remove_file(&operation.path)
                        .map_err(|error| AppError::io(&operation.path, error))?;
                }
            }
        }
        undo.push(ProjectFileUndo {
            path: operation.path.clone(),
            original: operation.original.clone(),
            applied_hash: operation.desired_hash.clone(),
        });
        changed.push(operation.relative_path.clone());
    }
    if operation.no_op
        && manifest.codex_model_catalog_hash.is_none()
        && operation.conflict.is_none()
    {
        // A matching user-owned catalog remains user-owned.
    } else {
        manifest.codex_model_catalog_hash = operation.desired_hash.clone();
    }
    Ok(())
}

fn stage_model_field(
    operation: &ModelOperation,
    resolution: Option<ConflictResolution>,
    managed_hash: &mut Option<String>,
) -> bool {
    if matches!(resolution, Some(ConflictResolution::Skip)) {
        return false;
    }
    if operation.conflict.is_some() {
        match resolution {
            Some(ConflictResolution::KeepCurrent) => {
                *managed_hash = None;
                return false;
            }
            Some(ConflictResolution::Overwrite) => {}
            _ => return false,
        }
    }
    if operation.no_op {
        if managed_hash.is_some() || operation.conflict.is_some() {
            *managed_hash = operation.desired_hash.clone();
        }
        return false;
    }
    *managed_hash = operation.desired_hash.clone();
    true
}

fn claude_mcp_denial_rules(settings: &Value) -> Result<Vec<Value>, AppError> {
    let Some(value) = settings.get("deniedMcpServers") else {
        return Ok(Vec::new());
    };
    let Some(rules) = value.as_array() else {
        return Err(AppError::Config(
            "Claude deniedMcpServers 必须是规则数组".into(),
        ));
    };
    if rules.iter().any(|rule| {
        let Some(rule) = rule.as_object() else {
            return true;
        };
        if rule.len() != 1 {
            return true;
        }
        match rule.iter().next() {
            Some((key, value)) if key == "serverName" || key == "serverUrl" => !value.is_string(),
            Some((key, value)) if key == "serverCommand" => !value
                .as_array()
                .is_some_and(|args| args.iter().all(Value::is_string)),
            _ => true,
        }
    }) {
        return Err(AppError::Config("Claude deniedMcpServers 每条规则必须包含一个 serverName、serverUrl 或 serverCommand 条件".into()));
    }
    Ok(rules.clone())
}

fn claude_mcp_denial_group(rules: &[Value], id: &str) -> Option<Value> {
    let group: Vec<Value> = rules
        .iter()
        .filter(|rule| rule.get("serverName").and_then(Value::as_str) == Some(id))
        .cloned()
        .collect();
    (!group.is_empty()).then_some(Value::Array(group))
}

fn claude_mcp_rule_matches(rule: &Value, id: &str, spec: Option<&Value>) -> bool {
    if rule.get("serverName").and_then(Value::as_str) == Some(id) {
        return true;
    }
    let Some(spec) = spec else { return false };
    if let (Some(pattern), Some(url)) = (
        rule.get("serverUrl").and_then(Value::as_str),
        spec.get("url").and_then(Value::as_str),
    ) {
        let pattern = format!("^{}$", regex::escape(pattern).replace("\\*", ".*"));
        if regex::Regex::new(&pattern).is_ok_and(|pattern| pattern.is_match(url)) {
            return true;
        }
    }
    if let (Some(expected), Some(command)) = (
        rule.get("serverCommand").and_then(Value::as_array),
        spec.get("command").and_then(Value::as_str),
    ) {
        let actual: Vec<Value> = std::iter::once(Value::String(command.into()))
            .chain(
                spec.get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .cloned(),
            )
            .collect();
        return expected == &actual;
    }
    false
}

fn plan_claude_mcp_denials(
    root: &Path,
    settings: &ClaudeSettingsDocument,
    manifest: &ProjectManifest,
    db: &Database,
    selected: &[String],
    desired: &DesiredMcp,
    warnings: &mut Vec<String>,
) -> Result<Vec<McpOperation>, AppError> {
    // https://code.claude.com/docs/en/settings-reference#deniedmcpservers:
    // project deny rules filter inherited user servers without changing global prefs.
    let rules = claude_mcp_denial_rules(&settings.root)?;
    let rows = db.get_mcp_server_rows()?;
    let global: HashSet<String> = rows
        .readable
        .values()
        .filter(|server| server.apps.is_enabled_for(&AppType::Claude))
        .map(|server| server.id.clone())
        .collect();
    let selected_set: HashSet<&str> = selected.iter().map(String::as_str).collect();
    let mut names: Vec<String> = global
        .iter()
        .cloned()
        .chain(manifest.claude_mcp_denials.keys().cloned())
        .chain(selected.iter().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    names.sort();
    let mut operations = Vec::new();
    for id in names {
        if desired.blocked_ids.contains(&id) {
            continue;
        }
        let previous = manifest.claude_mcp_denials.get(&id);
        let is_selected = selected_set.contains(id.as_str());
        let current = claude_mcp_denial_group(&rules, &id);
        let target = (!is_selected && (global.contains(&id) || previous.is_some()))
            .then(|| serde_json::json!([{ "serverName": id }]));
        let serialize = |value: &Value| {
            serde_json::to_string(value).map_err(|error| {
                AppError::Config(format!("序列化 Claude MCP 禁用规则失败: {error}"))
            })
        };
        let current_text = current.as_ref().map(serialize).transpose()?;
        let target_text = target.as_ref().map(serialize).transpose()?;
        let current_hash = current_text.as_deref().map(text_hash);
        let mut field = plan_model_field_op(
            &format!(".claude/settings.local.json#deniedMcpServers[{id}]"),
            Some(target_text),
            current_text,
            previous,
        )
        .ok_or_else(|| AppError::Config("缺少 Claude MCP 禁用规划".into()))?;
        if is_selected && current.is_some() && previous.is_none() {
            field.conflict = Some(ProjectDeployConflict {
                path: format!(".claude/settings.local.json#deniedMcpServers[{id}]"),
                reason: "项目中已有手写规则禁止所选 MCP".into(),
            });
            field.no_op = false;
        }
        operations.push(McpOperation {
            id,
            desired: target,
            desired_hash: field.desired_hash,
            current_hash,
            conflict: field.conflict,
            no_op: field.no_op,
        });
    }
    if !selected.is_empty() {
        for (source, path) in [
            (
                "Claude 用户设置",
                crate::config::get_claude_config_dir().join("settings.json"),
            ),
            (".claude/settings.json", root.join(".claude/settings.json")),
        ] {
            let source_rules = ensure_regular_target(&path)
                .and_then(|()| read_optional(&path))
                .and_then(|bytes| {
                    bytes
                        .map(|bytes| {
                            serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                                AppError::Config(format!("读取 Claude MCP 规则失败: {error}"))
                            })
                        })
                        .transpose()
                })
                .and_then(|value| value.as_ref().map(claude_mcp_denial_rules).transpose());
            match source_rules {
                Ok(Some(source_rules)) => {
                    for id in selected {
                        if source_rules
                            .iter()
                            .any(|rule| claude_mcp_rule_matches(rule, id, desired.entries.get(id)))
                        {
                            warnings.push(format!("MCP '{id}' 被 {source} 的 deniedMcpServers 规则阻止；保留该规则不作修改"));
                        }
                    }
                }
                Err(_) => warnings.push(format!(
                    "{source} 的 MCP 禁用规则不可读，无法确认所选 MCP 是否可用"
                )),
                _ => {}
            }
        }
        for id in selected {
            if rules.iter().any(|rule| {
                rule.get("serverName").is_none()
                    && claude_mcp_rule_matches(rule, id, desired.entries.get(id))
            }) {
                warnings.push(format!(
                    "MCP '{id}' 被项目中的 serverCommand 或 serverUrl 禁用规则阻止；保留该手写规则"
                ));
            }
        }
    }
    Ok(operations)
}

fn apply_claude_mcp_denials(
    root: &Path,
    settings: &mut ClaudeSettingsDocument,
    manifest: &mut ProjectManifest,
    operations: &[McpOperation],
    resolution: Option<ConflictResolution>,
    changed: &mut Vec<String>,
    undo: &mut Vec<ProjectFileUndo>,
) -> Result<(), AppError> {
    if operations.is_empty() || matches!(resolution, Some(ConflictResolution::Skip)) {
        return Ok(());
    }
    let original = read_optional(&settings.path)?;
    for operation in operations {
        let mut owned = manifest.claude_mcp_denials.get(&operation.id).cloned();
        let field = ModelOperation {
            desired: None,
            desired_hash: operation.desired_hash.clone(),
            conflict: operation.conflict.clone(),
            no_op: operation.no_op,
        };
        if stage_model_field(&field, resolution, &mut owned) {
            let rules = settings
                .root
                .as_object_mut()
                .ok_or_else(|| AppError::Config("Claude 项目设置必须是对象".into()))?
                .entry("deniedMcpServers")
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or_else(|| AppError::Config("Claude deniedMcpServers 必须是数组".into()))?;
            rules.retain(|rule| {
                rule.get("serverName").and_then(Value::as_str) != Some(operation.id.as_str())
            });
            if let Some(Value::Array(target)) = operation.desired.as_ref() {
                rules.extend(target.iter().cloned());
            }
            settings.changed = true;
        }
        match owned {
            Some(hash) => {
                manifest
                    .claude_mcp_denials
                    .insert(operation.id.clone(), hash);
            }
            None => {
                manifest.claude_mcp_denials.remove(&operation.id);
            }
        }
    }
    save_manifest(root, "claude", manifest)?;
    if let Some(hash) = settings.save(root)? {
        undo.push(ProjectFileUndo {
            path: settings.path.clone(),
            original,
            applied_hash: Some(hash),
        });
        if !changed
            .iter()
            .any(|path| path == ".claude/settings.local.json")
        {
            changed.push(".claude/settings.local.json".into());
        }
    }
    Ok(())
}

fn apply_instruction_op(
    root: &Path,
    app_name: &str,
    operation: &InstructionOperation,
    resolution: Option<ConflictResolution>,
    manifest: &mut ProjectManifest,
    changed: &mut Vec<String>,
    undo: &mut Vec<ProjectFileUndo>,
) -> Result<(), AppError> {
    if operation.conflict.is_some() {
        match resolution {
            Some(ConflictResolution::KeepCurrent) => {
                manifest.instruction_hash = None;
                return save_manifest(root, app_name, manifest);
            }
            Some(ConflictResolution::Overwrite) => {}
            Some(ConflictResolution::Skip) | None => return Ok(()),
        }
    }
    if operation.no_op {
        if manifest.instruction_hash.is_some() || operation.conflict.is_some() {
            manifest.instruction_hash = operation.desired_hash.clone();
        }
        return save_manifest(root, app_name, manifest);
    }

    let path = root.join(&operation.relative_path);
    ensure_regular_target(&path)?;
    let original = read_optional(&path)?;
    manifest.instruction_hash = operation.desired_hash.clone();
    save_manifest(root, app_name, manifest)?;
    match operation.desired.as_deref() {
        Some(content) => {
            write_if_unchanged(&path, operation.current_hash.as_deref(), content.as_bytes())?;
        }
        None => {
            let current = read_optional(&path)?;
            let current_hash = current.as_deref().map(crate::live::engine::sha256_hex);
            if current_hash != operation.current_hash {
                return Err(AppError::InvalidInput(format!(
                    "项目文件在部署期间发生变化，请重试: {}",
                    path.display()
                )));
            }
            fs::remove_file(&path).map_err(|error| AppError::io(&path, error))?;
        }
    }
    undo.push(ProjectFileUndo {
        path,
        original,
        applied_hash: operation.desired_hash.clone(),
    });
    changed.push(operation.relative_path.clone());
    Ok(())
}

fn rollback_project_files(undo: &[ProjectFileUndo]) -> Vec<String> {
    let mut errors = Vec::new();
    for operation in undo.iter().rev() {
        if !errors.is_empty()
            && operation
                .path
                .file_name()
                .is_some_and(|name| name == ".gitignore")
            && operation
                .path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == ".claude")
        {
            // An externally edited settings file may still contain the new key.
            // Keep its Git protection when any preceding rollback was incomplete.
            errors.push(
                "部分项目文件未能回滚，已保留 .claude/.gitignore 中的凭据文件忽略规则".into(),
            );
            continue;
        }
        let current = match read_optional(&operation.path) {
            Ok(current) => current,
            Err(error) => {
                errors.push(format!(
                    "读取回滚文件失败 {}: {error}",
                    operation.path.display()
                ));
                continue;
            }
        };
        let current_hash = current.as_deref().map(crate::live::engine::sha256_hex);
        if current_hash != operation.applied_hash {
            errors.push(format!(
                "项目文件在部署期间发生变化，保留现状 {}",
                operation.path.display()
            ));
            continue;
        }
        let restored = match operation.original.as_deref() {
            Some(bytes) => {
                write_if_unchanged(&operation.path, operation.applied_hash.as_deref(), bytes)
            }
            None => fs::remove_file(&operation.path)
                .map_err(|error| AppError::io(&operation.path, error)),
        };
        if let Err(error) = restored {
            errors.push(format!(
                "恢复项目文件失败 {}: {error}",
                operation.path.display()
            ));
        }
    }
    errors
}

fn restore_manifest(path: &Path, original: Option<&[u8]>) -> Result<(), AppError> {
    match original {
        Some(bytes) => {
            ensure_regular_target(path)?;
            crate::config::atomic_write_private(path, bytes)
        }
        None => remove_project_path(path),
    }
}

fn cleanup_skill_backups(undo: &[SkillUndo]) {
    for backup in undo
        .iter()
        .filter_map(|operation| operation.backup.as_ref())
    {
        if let Err(error) = remove_project_path(backup) {
            log::warn!("清理项目 Skill 备份失败: {}: {error}", backup.display());
        }
    }
}

fn skill_relative_path(app: &AppType, directory: &str) -> Result<PathBuf, AppError> {
    let directory_path = Path::new(directory);
    if directory_path.is_absolute()
        || directory_path.components().count() != 1
        || !matches!(
            directory_path.components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(AppError::InvalidInput(format!(
            "不安全的 Skill 目录名: {directory}"
        )));
    }
    let relative = match app {
        AppType::Claude => PathBuf::from(".claude").join("skills").join(directory_path),
        AppType::Codex => PathBuf::from(".agents").join("skills").join(directory_path),
        _ => {
            return Err(AppError::InvalidInput(
                "项目 Skills 仅支持 Claude Code 和 Codex".to_string(),
            ))
        }
    };
    Ok(relative)
}

fn ensure_project_parent(root: &Path, relative: &Path, create: bool) -> Result<(), AppError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(AppError::InvalidInput(format!(
                        "项目路径包含符号链接或非目录，拒绝写入: {}",
                        current.display()
                    )));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(&current).map_err(|err| AppError::io(&current, err))?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(AppError::io(&current, err)),
        }
        let canonical = current
            .canonicalize()
            .map_err(|err| AppError::io(&current, err))?;
        if !canonical.starts_with(root) {
            return Err(AppError::InvalidInput(format!(
                "项目路径越出项目根目录: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

fn ensure_regular_target(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::InvalidInput(format!(
            "项目配置是符号链接，拒绝跟随写入: {}",
            path.display()
        ))),
        Ok(metadata) if !metadata.is_file() => Err(AppError::InvalidInput(format!(
            "项目配置目标不是普通文件: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn write_if_unchanged(path: &Path, expected: Option<&str>, bytes: &[u8]) -> Result<(), AppError> {
    ensure_regular_target(path)?;
    let private = path
        .file_name()
        .is_some_and(|name| name == "settings.local.json");
    let staged = crate::config::stage_write(path, bytes, private.then_some(0o600), private)?;
    if staged.target() != path {
        staged.discard();
        return Err(AppError::InvalidInput(format!(
            "项目配置路径变成了符号链接，拒绝写入: {}",
            path.display()
        )));
    }
    if let Err(error) = ensure_regular_target(path) {
        staged.discard();
        return Err(error);
    }
    let current = match read_optional(path) {
        Ok(current) => current,
        Err(error) => {
            staged.discard();
            return Err(error);
        }
    };
    let current_hash = current.as_deref().map(crate::live::engine::sha256_hex);
    if current_hash.as_deref() != expected {
        staged.discard();
        return Err(AppError::InvalidInput(format!(
            "项目文件在部署期间发生变化，请重试: {}",
            path.display()
        )));
    }
    let temporary = staged.tmp_path().to_path_buf();
    match staged.commit() {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(temporary);
            Err(error)
        }
    }
}

fn codex_entry_hash(
    document: &toml_edit::DocumentMut,
    id: &str,
) -> Result<Option<String>, AppError> {
    let Some(item) = document
        .get("mcp_servers")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|servers| servers.get(id))
    else {
        return Ok(None);
    };
    let mut wrapper = toml_edit::DocumentMut::new();
    wrapper["mcp_servers"] = toml_edit::table();
    wrapper["mcp_servers"][id] = item.clone();
    let parsed: toml::Value = toml::from_str(&wrapper.to_string())
        .map_err(|err| AppError::Config(format!("解析 Codex MCP 条目 '{id}' 失败: {err}")))?;
    let value = parsed
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .and_then(|servers| servers.get(id))
        .ok_or_else(|| AppError::Config(format!("Codex MCP 条目 '{id}' 丢失")))?;
    let bytes = serde_json::to_vec(value)
        .map_err(|err| AppError::Config(format!("序列化 Codex MCP 条目 '{id}' 失败: {err}")))?;
    Ok(Some(crate::live::engine::sha256_hex(&bytes)))
}

fn codex_entry_value(
    document: &toml_edit::DocumentMut,
    id: &str,
) -> Result<Option<Value>, AppError> {
    let Some(item) = document
        .get("mcp_servers")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|servers| servers.get(id))
    else {
        return Ok(None);
    };
    let mut wrapper = toml_edit::DocumentMut::new();
    wrapper["mcp_servers"] = toml_edit::table();
    wrapper["mcp_servers"][id] = item.clone();
    let parsed: toml::Value = toml::from_str(&wrapper.to_string())
        .map_err(|err| AppError::Config(format!("解析 Codex MCP 条目 '{id}' 失败: {err}")))?;
    parsed
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .and_then(|servers| servers.get(id))
        .map(|value| {
            serde_json::to_value(value)
                .map_err(|err| AppError::Config(format!("转换 Codex MCP 条目 '{id}' 失败: {err}")))
        })
        .transpose()
}

fn path_state(path: &Path) -> Result<Option<String>, AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(path).map_err(|err| AppError::io(path, err))?;
            let mut bytes = b"symlink\0".to_vec();
            bytes.extend(target.to_string_lossy().as_bytes());
            Ok(Some(crate::live::engine::sha256_hex(&bytes)))
        }
        Ok(metadata) if metadata.is_dir() => hash_tree(path, false).map(Some),
        Ok(metadata) if metadata.is_file() => {
            let bytes = fs::read(path).map_err(|err| AppError::io(path, err))?;
            let mut data = b"file\0".to_vec();
            data.extend(bytes);
            Ok(Some(crate::live::engine::sha256_hex(&data)))
        }
        Ok(_) => Ok(Some(crate::live::engine::sha256_hex(b"other"))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn verify_path_state(path: &Path, expected: Option<&str>) -> Result<(), AppError> {
    let actual = path_state(path)?;
    if actual.as_deref() != expected {
        return Err(AppError::InvalidInput(format!(
            "项目目录在部署期间发生变化，请重试: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn hash_tree(path: &Path, reject_symlinks: bool) -> Result<String, AppError> {
    use sha2::{Digest, Sha256};
    fn collect(
        base: &Path,
        current: &Path,
        reject_symlinks: bool,
        items: &mut Vec<(PathBuf, Vec<u8>)>,
    ) -> Result<(), AppError> {
        for entry in fs::read_dir(current).map_err(|err| AppError::io(current, err))? {
            let entry = entry.map_err(|err| AppError::io(current, err))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|err| AppError::io(&path, err))?;
            let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            if metadata.file_type().is_symlink() {
                if reject_symlinks {
                    return Err(AppError::InvalidInput(format!(
                        "Skill 源目录包含符号链接: {}",
                        path.display()
                    )));
                }
                let target = fs::read_link(&path).map_err(|err| AppError::io(&path, err))?;
                items.push((
                    relative,
                    [b"link\0".as_slice(), target.to_string_lossy().as_bytes()].concat(),
                ));
            } else if metadata.is_dir() {
                items.push((relative.clone(), b"dir".to_vec()));
                collect(base, &path, reject_symlinks, items)?;
            } else if metadata.is_file() {
                let mut content = b"file\0".to_vec();
                content.extend(fs::read(&path).map_err(|err| AppError::io(&path, err))?);
                items.push((relative, content));
            } else {
                items.push((relative, b"other".to_vec()));
            }
        }
        Ok(())
    }

    let mut entries = Vec::new();
    let metadata = fs::symlink_metadata(path).map_err(|err| AppError::io(path, err))?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::InvalidInput(format!(
            "Skill 根目录是符号链接: {}",
            path.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "Skill 根目录不是目录: {}",
            path.display()
        )));
    }
    collect(path, path, reject_symlinks, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, content) in entries {
        hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        hasher.update(b"\0");
        hasher.update(&content);
        hasher.update(b"\0");
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn validate_copy_source(source: &Path) -> Result<(), AppError> {
    let _ = hash_tree(source, true)?;
    if !source.join("SKILL.md").is_file() {
        return Err(AppError::InvalidInput(format!(
            "Skill 源目录缺少 SKILL.md: {}",
            source.display()
        )));
    }
    Ok(())
}

pub(crate) fn copy_tree(source: &Path, target: &Path) -> Result<(), AppError> {
    fs::create_dir_all(target).map_err(|err| AppError::io(target, err))?;
    for entry in fs::read_dir(source).map_err(|err| AppError::io(source, err))? {
        let entry = entry.map_err(|err| AppError::io(source, err))?;
        let path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path).map_err(|err| AppError::io(&path, err))?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::InvalidInput(format!(
                "Skill 源目录包含符号链接: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            copy_tree(&path, &target_path)?;
        } else if metadata.is_file() {
            fs::copy(&path, &target_path).map_err(|err| AppError::io(&path, err))?;
        }
    }
    Ok(())
}

fn replace_skill_directory(
    source: &Path,
    destination: &Path,
    desired_hash: Option<&str>,
    expected_current_state: Option<&str>,
) -> Result<Option<PathBuf>, AppError> {
    validate_copy_source(source)?;
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::InvalidInput("无效的 Skill 目标路径".to_string()))?;
    fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
    let nonce = uuid::Uuid::new_v4();
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill");
    let staged = parent.join(format!(".{name}.cc-switch-{nonce}.tmp"));
    let backup = parent.join(format!(".{name}.cc-switch-{nonce}.bak"));
    if let Err(err) = copy_tree(source, &staged) {
        let _ = fs::remove_dir_all(&staged);
        return Err(err);
    }
    if let Some(expected) = desired_hash {
        let staged_hash = path_state(&staged);
        if !matches!(staged_hash, Ok(Some(ref hash)) if hash == expected) {
            let _ = fs::remove_dir_all(&staged);
            return Err(AppError::InvalidInput(
                "Skill 源在部署期间发生变化，请重试".to_string(),
            ));
        }
    }

    if let Err(error) = verify_path_state(destination, expected_current_state) {
        let _ = fs::remove_dir_all(&staged);
        return Err(error);
    }
    let had_destination = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            let _ = fs::remove_dir_all(&staged);
            return Err(AppError::io(destination, err));
        }
    };
    if had_destination {
        if let Err(err) = fs::rename(destination, &backup) {
            let _ = fs::remove_dir_all(&staged);
            return Err(AppError::io(destination, err));
        }
        if let Err(error) = verify_path_state(&backup, expected_current_state) {
            let restore = match fs::symlink_metadata(destination) {
                Ok(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "目标目录已被重新创建",
                )),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    fs::rename(&backup, destination)
                }
                Err(err) => Err(err),
            };
            let _ = fs::remove_dir_all(&staged);
            return match restore {
                Ok(()) => Err(error),
                Err(restore_error) => Err(AppError::Config(format!(
                    "项目 Skill 目录在部署期间发生变化: {error}; 并发内容保留在备份路径 {}，恢复失败: {restore_error}",
                    backup.display()
                ))),
            };
        }
    }
    if let Err(err) = fs::rename(&staged, destination) {
        if had_destination {
            if let Err(restore_error) = fs::rename(&backup, destination) {
                let _ = fs::remove_dir_all(&staged);
                return Err(AppError::Config(format!(
                    "写入项目 Skill 目录失败: {err}; 原目录保留在 {}，恢复失败: {restore_error}",
                    backup.display()
                )));
            }
        }
        let _ = fs::remove_dir_all(&staged);
        return Err(AppError::io(destination, err));
    }
    Ok(had_destination.then_some(backup))
}

fn remove_project_path(path: &Path) -> Result<(), AppError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(AppError::io(path, err)),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|err| AppError::io(path, err))
    } else {
        fs::remove_file(path).map_err(|err| AppError::io(path, err))
    }
}

fn load_manifest(root: &Path, app: &str) -> Result<ProjectManifest, AppError> {
    let path = manifest_path(root, app)?;
    let Some(bytes) = read_optional(&path)? else {
        return Ok(ProjectManifest {
            version: 1,
            project_root: root.to_string_lossy().into_owned(),
            app: app.to_string(),
            claude_settings_file: (app == "claude").then(|| CLAUDE_PROJECT_SETTINGS.to_string()),
            ..ProjectManifest::default()
        });
    };
    let manifest: ProjectManifest = serde_json::from_slice(&bytes)
        .map_err(|err| AppError::Config(format!("解析项目部署清单失败: {err}")))?;
    if manifest.version != 1
        || manifest.project_root != root.to_string_lossy()
        || manifest.app != app
    {
        return Err(AppError::Config("项目部署清单与当前项目不匹配".to_string()));
    }
    if app == "claude" && manifest.claude_settings_file.as_deref() != Some(CLAUDE_PROJECT_SETTINGS)
    {
        return Err(AppError::Config(
            "项目部署清单的 Claude 设置路径无效".into(),
        ));
    }
    let app_type = match app {
        "claude" => AppType::Claude,
        "codex" => AppType::Codex,
        _ => return Err(AppError::Config("项目部署清单 Agent 无效".to_string())),
    };
    for relative in manifest.skills.keys() {
        let directory = Path::new(relative)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::Config("项目部署清单含无效 Skill 路径".to_string()))?;
        let expected = skill_relative_path(&app_type, directory)?
            .to_string_lossy()
            .replace('\\', "/");
        if relative != &expected {
            return Err(AppError::Config(format!(
                "项目部署清单含越界 Skill 路径: {relative}"
            )));
        }
    }
    Ok(manifest)
}

fn save_manifest(root: &Path, app: &str, manifest: &ProjectManifest) -> Result<(), AppError> {
    let path = manifest_path(root, app)?;
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|err| AppError::Config(format!("序列化项目部署清单失败: {err}")))?;
    crate::config::atomic_write_private(&path, &bytes)
}

fn manifest_path(root: &Path, app: &str) -> Result<PathBuf, AppError> {
    let key = format!("{}\0{app}", root.to_string_lossy());
    let digest = crate::live::engine::sha256_hex(key.as_bytes());
    let dir = crate::config::get_home_dir()
        .join(".cc-switch")
        .join("project-profile-manifests");
    fs::create_dir_all(&dir).map_err(|err| AppError::io(&dir, err))?;
    Ok(dir.join(format!("{digest}.json")))
}

/// Test fixture: Profiles are no longer bound to a directory, so tests name the
/// project a seeded Profile is deployed to once instead of at every call.
#[cfg(test)]
pub(crate) mod test_projects {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static DEFAULT: RefCell<Option<String>> = const { RefCell::new(None) };
        static BOUND: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    }

    /// Project used by Profiles without their own binding.
    pub(crate) fn use_project(root: &Path) {
        DEFAULT.with(|default| *default.borrow_mut() = Some(root.to_string_lossy().into_owned()));
    }

    pub(crate) fn bind(profile_id: &str, root: &Path) {
        BOUND.with(|bound| {
            bound
                .borrow_mut()
                .insert(profile_id.to_string(), root.to_string_lossy().into_owned())
        });
    }

    fn root_of(profile_id: &str) -> String {
        BOUND
            .with(|bound| bound.borrow().get(profile_id).cloned())
            .or_else(|| DEFAULT.with(|default| default.borrow().clone()))
            .expect("test Profile has a project")
    }

    pub(crate) fn deploy(
        state: &AppState,
        profile_id: &str,
        scope: ProfileScope,
        resolution: Option<ConflictResolution>,
    ) -> Result<ProjectDeployResult, AppError> {
        let path = root_of(profile_id);
        crate::services::profile::ProfileService::add_project(state, &path)?;
        // These fixtures exercise file deployment independently of API setup.
        // The connection prompt is covered through the public default entry below.
        ProjectProfileService::deploy_with_connection_choice(
            state, profile_id, scope, &path, resolution, true,
        )
    }

    /// Agents whose project deployment is this exact Profile revision.
    pub(crate) fn deployed_scopes(profile: &crate::database::Profile) -> Vec<ProfileScope> {
        let status = ProjectProfileService::project_status_with(&root_of(&profile.id), |id| {
            (id == profile.id).then(|| profile.payload.clone())
        });
        [ProfileScope::Claude, ProfileScope::Codex]
            .into_iter()
            .filter(|scope| {
                status
                    .deployed
                    .get(scope.as_str())
                    .is_some_and(|deployed| deployed.profile_id == profile.id && !deployed.stale)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{InstalledSkill, McpApps, McpServer, SkillApps};
    use crate::database::Profile;
    use crate::test_support::TempUserHome;
    use serial_test::serial;

    fn seed_profile(
        state: &AppState,
        root: &Path,
        scope: ProfileScope,
        mcp_ids: Option<Vec<String>>,
        skill_ids: Option<Vec<String>>,
    ) {
        let app = scope.apps()[0].clone();
        let mut payload = ProfilePayload::default();
        *payload.mcp.get_mut(&app).unwrap() = mcp_ids;
        *payload.skills.get_mut(&app).unwrap() = skill_ids;
        let profile = Profile {
            id: "profile".to_string(),
            target: crate::database::ProfileTarget::Project,
            name: "Project profile".to_string(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            created_at: Some(1),
            updated_at: Some(1),
        };
        state.db.save_profile(&profile).unwrap();
        test_projects::use_project(root);
        test_projects::bind(&profile.id, root);
    }

    fn seed_mcp_and_skill(state: &AppState) {
        state
            .db
            .save_mcp_server(&McpServer {
                id: "filesystem".to_string(),
                name: "Filesystem".to_string(),
                server: serde_json::json!({
                    "command": "echo",
                    "args": ["hello"]
                }),
                apps: McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: Vec::new(),
            })
            .unwrap();

        let source = SkillService::get_installed_skill_dir("project-skill").unwrap();
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: project-skill\ndescription: test\n---\n",
        )
        .unwrap();
        fs::write(source.join("helper.txt"), "managed content").unwrap();
        state
            .db
            .save_skill(&InstalledSkill {
                id: "skill-id".to_string(),
                name: "project-skill".to_string(),
                description: Some("test".to_string()),
                directory: "project-skill".to_string(),
                repo_host: None,
                repo_owner: None,
                repo_name: None,
                repo_branch: None,
                readme_url: None,
                apps: SkillApps::default(),
                installed_at: 1,
                content_hash: None,
                updated_at: 0,
            })
            .unwrap();
    }

    #[test]
    #[serial]
    fn shared_prompt_deploys_to_agent_specific_project_files_and_changes_only_on_reapply() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let mut prompt = crate::prompt::Prompt {
            id: "shared-instructions".into(),
            name: "Shared instructions".into(),
            content: "Shared project rules".into(),
            description: None,
            created_at: None,
            updated_at: None,
        };
        state.db.save_prompt(&prompt).unwrap();
        seed_profile(
            &state,
            &root,
            ProfileScope::Claude,
            Some(vec![]),
            Some(vec![]),
        );
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.mcp.codex = Some(vec![]);
        payload.skills.codex = Some(vec![]);
        payload.prompts.claude = Some(prompt.id.clone());
        payload.prompts.codex = Some(prompt.id.clone());
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        for scope in [ProfileScope::Claude, ProfileScope::Codex] {
            assert!(matches!(
                test_projects::deploy(&state, &profile.id, scope, None).unwrap(),
                ProjectDeployResult::Applied { .. }
            ));
        }
        for filename in ["CLAUDE.md", "AGENTS.md"] {
            assert_eq!(
                fs::read_to_string(root.join(filename)).unwrap(),
                prompt.content
            );
        }
        prompt.content = "New library revision".into();
        state.db.save_prompt(&prompt).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "Shared project rules"
        );
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "Shared project rules"
        );
        test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            prompt.content
        );
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "Shared project rules"
        );
        state.db.delete_prompt(&prompt.id).unwrap();
        let result = test_projects::deploy(&state, &profile.id, ProfileScope::Codex, None).unwrap();
        assert!(
            matches!(result, ProjectDeployResult::Applied { warnings, .. } if !warnings.is_empty())
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            prompt.content
        );
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "Shared project rules"
        );
        for app in [AppType::Claude, AppType::Codex] {
            assert!(state.db.get_prompt_application(&app).unwrap().is_none());
        }
    }

    fn test_state() -> (TempUserHome, AppState) {
        let home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        (home, state)
    }

    #[test]
    #[serial]
    fn missing_connection_prompts_before_model_deployment_and_allows_explicit_settings_only() {
        let (_home, state) = test_state();
        let _env = crate::test_support::EnvVars::new()
            .remove("CLAUDE_CONFIG_DIR")
            .remove("ANTHROPIC_BASE_URL")
            .remove("ANTHROPIC_AUTH_TOKEN")
            .remove("ANTHROPIC_API_KEY")
            .remove("CLAUDE_CODE_OAUTH_TOKEN");
        let project = tempfile::tempdir().unwrap();
        seed_profile(&state, project.path(), ProfileScope::Claude, None, None);
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.models.claude = Some("project-model".into());
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        let path = project.path().to_str().unwrap();
        crate::services::profile::ProfileService::add_project(&state, path).unwrap();
        let result =
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, path, None)
                .unwrap();
        #[cfg(not(target_os = "macos"))]
        assert!(
            matches!(result, ProjectDeployResult::NeedsConnection { connection }
            if connection.status == readiness::ConnectionStatus::Missing)
        );
        #[cfg(not(target_os = "macos"))]
        assert!(!project.path().join(CLAUDE_PROJECT_SETTINGS).exists());
        #[cfg(target_os = "macos")]
        let _ = result; // A keychain which was not inspected is unknown, not missing.
        let result = ProjectProfileService::deploy_with_connection_choice(
            &state,
            "profile",
            ProfileScope::Claude,
            path,
            None,
            true,
        )
        .unwrap();
        assert!(matches!(
            result,
            ProjectDeployResult::Applied {
                connection: Some(AppliedConnection {
                    mode: "inherit",
                    ..
                }),
                ..
            }
        ));
        let local: Value =
            crate::config::read_json_file(&project.path().join(CLAUDE_PROJECT_SETTINGS)).unwrap();
        assert_eq!(local["model"], "project-model");
        assert!(local["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
    }

    #[test]
    #[serial]
    fn connection_guidance_does_not_block_resource_only_or_existing_native_connections() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        seed_profile(
            &state,
            project.path(),
            ProfileScope::Claude,
            Some(vec![]),
            Some(vec![]),
        );
        let path = project.path().to_str().unwrap();
        crate::services::profile::ProfileService::add_project(&state, path).unwrap();
        assert!(matches!(
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, path, None)
                .unwrap(),
            ProjectDeployResult::Applied { .. }
        ));
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.models.claude = Some("native-project-model".into());
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        fs::create_dir_all(project.path().join(".claude")).unwrap();
        let shared = project.path().join(".claude/settings.json");
        fs::write(&shared, r#"{"env":{"ANTHROPIC_BASE_URL":"https://native.example","ANTHROPIC_AUTH_TOKEN":"native-test-token"}}"#).unwrap();
        let before = fs::read(&shared).unwrap();
        let result =
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, path, None)
                .unwrap();
        assert!(matches!(
            result,
            ProjectDeployResult::Applied {
                connection: Some(AppliedConnection {
                    mode: "inherit",
                    connection: readiness::InheritedConnection {
                        status: readiness::ConnectionStatus::Configured,
                        source: Some(readiness::ConnectionSource::Project),
                        ..
                    }
                }),
                ..
            }
        ));
        assert_eq!(fs::read(&shared).unwrap(), before);
    }

    #[test]
    fn explicitly_inherited_model_is_not_a_request_to_use_provider_defaults() {
        let mut payload = ProfilePayload::default();
        payload.providers.claude = Some("reference-only".into());
        payload.models.claude = Some(String::new());
        assert!(!profile_selects_model(&payload));
        payload.models.claude = None;
        assert!(profile_selects_model(&payload));
        payload.providers.claude = None;
        payload.model_settings.claude = Some(BTreeMap::from([(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
            "chosen-role".into(),
        )]));
        assert!(profile_selects_model(&payload));
    }

    #[test]
    #[serial]
    fn resource_names_survive_missing_references_and_do_not_change_deployment_identity() {
        use crate::services::profile::{ProfileConfig, ProfileService};

        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        seed_mcp_and_skill(&state);
        let mut server = state.db.get_all_mcp_servers().unwrap()["filesystem"].clone();
        server.apps.claude = true;
        state.db.save_mcp_server(&server).unwrap();
        let mut skill = state.db.get_installed_skill("skill-id").unwrap().unwrap();
        skill.apps.claude = true;
        state.db.save_skill(&skill).unwrap();
        state
            .db
            .save_prompt(&crate::prompt::Prompt {
                id: "prompt-id".into(),
                name: "Project instructions".into(),
                content: "Keep these instructions".into(),
                description: None,

                created_at: None,
                updated_at: None,
            })
            .unwrap();
        crate::services::PromptService::enable_prompt(&state, AppType::Claude, "prompt-id")
            .unwrap();
        let mut profile = ProfileService::create(
            &state,
            "Named resources",
            ProfileScope::Claude,
            crate::database::ProfileTarget::Project,
        )
        .unwrap();
        test_projects::use_project(project.path());
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        assert_eq!(
            payload.resource_names.claude.mcp["filesystem"],
            "Filesystem"
        );
        assert_eq!(
            payload.resource_names.claude.skills["skill-id"],
            "project-skill"
        );
        assert_eq!(
            payload.resource_names.claude.prompts["prompt-id"],
            "Project instructions"
        );
        payload
            .resource_names
            .codex
            .mcp
            .insert("other".into(), "Other Agent".into());
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        let config = ProfileConfig {
            claude_project_connection: Default::default(),
            follow_global_resources: false,
            native_config: None,
            provider_id: None,
            model: String::new(),
            model_settings: Some(BTreeMap::new()),
            model_catalog: None,
            mcp_ids: vec!["filesystem".into()],
            skill_ids: vec!["skill-id".into()],
            prompt_id: Some("prompt-id".into()),
        };
        profile = ProfileService::update_config(
            &state,
            &profile.id,
            ProfileScope::Claude,
            config.clone(),
            None,
        )
        .unwrap();
        payload = serde_json::from_str(&profile.payload).unwrap();
        test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap();
        let original_hash = profile_scope_hash(&payload, ProfileScope::Claude);
        let output_paths = [
            ".mcp.json",
            ".claude/skills/project-skill/SKILL.md",
            "CLAUDE.md",
        ];
        let deployed: Vec<Vec<u8>> = output_paths
            .iter()
            .map(|path| fs::read(project.path().join(path)).unwrap())
            .collect();
        server.name = "Renamed MCP".into();
        state.db.save_mcp_server(&server).unwrap();
        let edited = ProfileService::update_config(
            &state,
            &profile.id,
            ProfileScope::Claude,
            config.clone(),
            None,
        )
        .unwrap();
        let saved: ProfilePayload = serde_json::from_str(&edited.payload).unwrap();
        assert_eq!(saved.resource_names.claude.mcp["filesystem"], "Renamed MCP");
        assert_eq!(saved.resource_names.codex, payload.resource_names.codex);
        assert_eq!(
            profile_scope_hash(&saved, ProfileScope::Claude),
            original_hash
        );
        assert_eq!(
            test_projects::deployed_scopes(&edited),
            vec![ProfileScope::Claude]
        );

        state.db.conn.lock().unwrap().execute(
            "UPDATE mcp_servers SET name = 'Unreadable current name', server_config = '{broken' WHERE id = 'filesystem'", [],
        ).unwrap();
        let result =
            test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap();
        assert!(
            matches!(result, ProjectDeployResult::Applied { warnings, .. }
            if warnings.iter().any(|warning| warning.contains("Unreadable current name (filesystem)") && warning.contains("配置不可读")))
        );
        let edited = ProfileService::update_config(
            &state,
            &profile.id,
            ProfileScope::Claude,
            config.clone(),
            None,
        )
        .unwrap();
        let saved: ProfilePayload = serde_json::from_str(&edited.payload).unwrap();
        assert_eq!(
            saved.resource_names.claude.mcp["filesystem"],
            "Unreadable current name"
        );

        state.db.delete_mcp_server("filesystem").unwrap();
        state.db.delete_skill("skill-id").unwrap();
        state.db.delete_prompt("prompt-id").unwrap();
        let edited = ProfileService::update_config(
            &state,
            &profile.id,
            ProfileScope::Claude,
            config.clone(),
            Some("Renamed profile".into()),
        )
        .unwrap();
        let missing: ProfilePayload = serde_json::from_str(&edited.payload).unwrap();
        assert_eq!(missing.resource_names, saved.resource_names);
        let ProjectDeployResult::Applied { warnings, .. } =
            test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap()
        else {
            panic!("missing resources should preserve existing project files");
        };
        for label in [
            "Unreadable current name (filesystem)",
            "project-skill (skill-id)",
            "Project instructions (prompt-id)",
        ] {
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.contains(label) && warning.contains("已删除")),
                "{warnings:?}"
            );
        }
        for (path, original) in output_paths.iter().zip(deployed) {
            assert_eq!(fs::read(project.path().join(path)).unwrap(), original);
        }
        let global = Profile {
            id: "global-copy".into(),
            target: crate::database::ProfileTarget::Global,
            ..edited.clone()
        };
        state.db.save_profile(&global).unwrap();
        let warnings = ProfileService::apply(&state, &global.id, ProfileScope::Claude).unwrap();
        for label in [
            "Unreadable current name (filesystem)",
            "project-skill (skill-id)",
            "Project instructions (prompt-id)",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(label)),
                "{warnings:?}"
            );
        }
        let pruned = ProfileService::update_config(
            &state,
            &profile.id,
            ProfileScope::Claude,
            ProfileConfig {
                claude_project_connection: Default::default(),
                native_config: None,
                mcp_ids: vec![],
                skill_ids: vec![],
                prompt_id: None,
                ..config
            },
            None,
        )
        .unwrap();
        let pruned: ProfilePayload = serde_json::from_str(&pruned.payload).unwrap();
        assert_eq!(pruned.resource_names.claude, Default::default());
        assert_eq!(pruned.resource_names.codex, saved.resource_names.codex);
    }

    #[test]
    #[serial]
    fn editing_an_applied_profile_marks_only_that_agent_as_pending() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        seed_profile(
            &state,
            project.path(),
            ProfileScope::Claude,
            Some(vec![]),
            None,
        );
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.mcp.codex = Some(vec![]);
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap();
        test_projects::deploy(&state, &profile.id, ProfileScope::Codex, None).unwrap();
        assert_eq!(
            test_projects::deployed_scopes(&profile),
            vec![ProfileScope::Claude, ProfileScope::Codex]
        );
        let edited = crate::services::profile::ProfileService::update_config(
            &state,
            &profile.id,
            ProfileScope::Claude,
            crate::services::profile::ProfileConfig {
                claude_project_connection: Default::default(),
                follow_global_resources: false,
                native_config: None,
                provider_id: None,
                model: "claude-sonnet-4-6".into(),
                model_settings: Some(Default::default()),
                model_catalog: None,
                mcp_ids: vec![],
                skill_ids: vec![],
                prompt_id: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            test_projects::deployed_scopes(&edited),
            vec![ProfileScope::Codex]
        );
        test_projects::deploy(&state, &edited.id, ProfileScope::Claude, None).unwrap();
        assert_eq!(
            test_projects::deployed_scopes(&edited),
            vec![ProfileScope::Claude, ProfileScope::Codex]
        );
        let settings: Value = serde_json::from_slice(
            &fs::read(project.path().join(".claude/settings.local.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "claude-sonnet-4-6");
    }

    #[test]
    #[serial]
    fn deployed_profile_selection_is_shared_per_project_and_agent_and_tolerates_bad_paths() {
        let (_home, state) = test_state();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let root = first.path().canonicalize().unwrap();
        seed_mcp_and_skill(&state);
        seed_profile(
            &state,
            &root,
            ProfileScope::Claude,
            Some(vec!["filesystem".into()]),
            None,
        );
        let mut a = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&a.payload).unwrap();
        payload.mcp.codex = Some(vec!["filesystem".into()]);
        a.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&a).unwrap();
        let mut b = a.clone();
        b.id = "same-project-alternative".into();
        payload.mcp.claude = Some(vec![]);
        payload.mcp.codex = Some(vec![]);
        b.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&b).unwrap();
        let mut c = a.clone();
        c.id = "other-project".into();
        test_projects::bind(&c.id, second.path());
        state.db.save_profile(&c).unwrap();
        let error = ProjectProfileService::deploy(
            &state,
            &c.id,
            ProfileScope::Claude,
            second.path().to_str().unwrap(),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("请先登记项目目录"));
        assert!(second.path().read_dir().unwrap().next().is_none());
        // A registered directory has no deployment until the user applies a Profile.
        assert!(test_projects::deployed_scopes(&a).is_empty());
        test_projects::deploy(&state, &a.id, ProfileScope::Claude, None).unwrap();
        test_projects::deploy(&state, &b.id, ProfileScope::Codex, None).unwrap();
        assert_eq!(
            test_projects::deployed_scopes(&a),
            vec![ProfileScope::Claude]
        );
        assert_eq!(
            test_projects::deployed_scopes(&b),
            vec![ProfileScope::Codex]
        );
        test_projects::deploy(&state, &b.id, ProfileScope::Claude, None).unwrap();
        assert!(test_projects::deployed_scopes(&a).is_empty());
        assert_eq!(
            test_projects::deployed_scopes(&b),
            vec![ProfileScope::Claude, ProfileScope::Codex]
        );
        test_projects::deploy(&state, &c.id, ProfileScope::Claude, None).unwrap();
        test_projects::deploy(&state, &a.id, ProfileScope::Codex, None).unwrap();
        assert_eq!(
            test_projects::deployed_scopes(&a),
            vec![ProfileScope::Codex]
        );
        assert_eq!(
            test_projects::deployed_scopes(&b),
            vec![ProfileScope::Claude]
        );
        assert_eq!(
            test_projects::deployed_scopes(&c),
            vec![ProfileScope::Claude]
        );
        test_projects::deploy(
            &state,
            &a.id,
            ProfileScope::Claude,
            Some(ConflictResolution::Skip),
        )
        .unwrap();
        assert_eq!(
            test_projects::deployed_scopes(&b),
            vec![ProfileScope::Claude]
        );
        fs::write(root.join(".mcp.json"), "invalid JSON").unwrap();
        assert!(test_projects::deploy(&state, &a.id, ProfileScope::Claude, None).is_err());
        assert_eq!(
            test_projects::deployed_scopes(&b),
            vec![ProfileScope::Claude]
        );
        assert_eq!(
            test_projects::deployed_scopes(&a),
            vec![ProfileScope::Codex]
        );
        fs::write(
            manifest_path(&second.path().canonicalize().unwrap(), "claude").unwrap(),
            "invalid manifest",
        )
        .unwrap();
        assert!(test_projects::deployed_scopes(&c).is_empty());
        test_projects::bind(&c.id, &root.join("not-present"));
        assert!(test_projects::deployed_scopes(&c).is_empty());
        let missing = ProjectProfileService::project_status(
            &state,
            &root.join("not-present").to_string_lossy(),
        );
        assert!(!missing.available);
        assert!(missing.deployed.is_empty());
        // One directory reports each Agent's own Profile, and flags an edited or
        // deleted Profile instead of hiding what the project still contains.
        let status = ProjectProfileService::project_status(&state, &root.to_string_lossy());
        assert!(status.available);
        assert_eq!(
            status.deployed["claude"],
            DeployedProfile {
                profile_id: b.id.clone(),
                stale: false
            }
        );
        assert_eq!(status.deployed["codex"].profile_id, a.id);
        let mut edited: ProfilePayload = serde_json::from_str(&b.payload).unwrap();
        edited.models.claude = Some("edited-after-deploy".into());
        b.payload = serde_json::to_string(&edited).unwrap();
        state.db.save_profile(&b).unwrap();
        state.db.delete_profile(&a.id).unwrap();
        let status = ProjectProfileService::project_status(&state, &root.to_string_lossy());
        assert_eq!(
            status.deployed["claude"],
            DeployedProfile {
                profile_id: b.id.clone(),
                stale: true
            }
        );
        assert_eq!(
            status.deployed["codex"],
            DeployedProfile {
                profile_id: a.id.clone(),
                stale: true
            }
        );
        // The two project locations are shared by all of the Profiles.
        let projects = state.db.get_profile_projects().unwrap();
        assert_eq!(projects.len(), 2, "{projects:?}");
    }

    #[cfg(windows)]
    #[test]
    #[serial]
    fn failed_final_mcp_write_restores_the_previously_deployed_profile() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_mcp_and_skill(&state);
        seed_profile(
            &state,
            &root,
            ProfileScope::Claude,
            Some(vec!["filesystem".into()]),
            None,
        );
        let mut a = state.db.get_profile("profile").unwrap().unwrap();
        let mut initial_payload: ProfilePayload = serde_json::from_str(&a.payload).unwrap();
        initial_payload.model_settings.claude = Some(BTreeMap::from([(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
            "initial-sonnet".to_string(),
        )]));
        a.payload = serde_json::to_string(&initial_payload).unwrap();
        state.db.save_profile(&a).unwrap();
        test_projects::deploy(&state, &a.id, ProfileScope::Claude, None).unwrap();
        let mut b = a.clone();
        b.id = "alternative".into();
        let mut payload: ProfilePayload = serde_json::from_str(&b.payload).unwrap();
        payload.mcp.claude = Some(vec![]);
        payload.model_settings.claude = Some(BTreeMap::from([(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
            "rollback-sonnet".to_string(),
        )]));
        b.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&b).unwrap();
        let mcp_path = root.join(".mcp.json");
        let original_mcp = fs::read(&mcp_path).unwrap();
        let settings_path = root.join(CLAUDE_PROJECT_SETTINGS);
        let original_settings = fs::read(&settings_path).unwrap();
        let manifest_file = manifest_path(&root, "claude").unwrap();
        let original_manifest = fs::read(&manifest_file).unwrap();
        // FILE_SHARE_READ denies replacement while still allowing deployment preflight.
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&mcp_path)
            .unwrap();
        assert!(test_projects::deploy(&state, &b.id, ProfileScope::Claude, None).is_err());
        assert_eq!(fs::read(&manifest_file).unwrap(), original_manifest);
        assert_eq!(fs::read(&mcp_path).unwrap(), original_mcp);
        assert_eq!(fs::read(&settings_path).unwrap(), original_settings);
        assert_eq!(
            test_projects::deployed_scopes(&a),
            vec![ProfileScope::Claude]
        );
        assert!(test_projects::deployed_scopes(&b).is_empty());
        drop(locked);
        test_projects::deploy(&state, &b.id, ProfileScope::Claude, None).unwrap();
        assert!(test_projects::deployed_scopes(&a).is_empty());
        assert_eq!(
            test_projects::deployed_scopes(&b),
            vec![ProfileScope::Claude]
        );
    }

    fn seed_model_profile(state: &AppState, root: &Path, scope: ProfileScope) {
        let app = scope.apps()[0].clone();
        let settings = match app {
            AppType::Claude => serde_json::json!({"env": {"ANTHROPIC_MODEL": "test-model"}}),
            _ => serde_json::json!({"config": "model = \"test-model\"\n"}),
        };
        state
            .db
            .save_provider(
                &app,
                &crate::provider::Provider::with_id(
                    "model-provider".into(),
                    "Model provider".into(),
                    settings,
                    None,
                ),
            )
            .unwrap();
        crate::settings::set_current_provider(&app, Some("model-provider")).unwrap();
        state
            .db
            .save_prompt(&crate::prompt::Prompt {
                id: "project-prompt".into(),
                name: "Project prompt".into(),
                content: "project instructions".into(),
                description: None,

                created_at: None,
                updated_at: None,
            })
            .unwrap();
        seed_profile(state, root, scope, Some(vec![]), Some(vec![]));
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        *payload.providers.get_mut(&app).unwrap() = Some("model-provider".into());
        *payload.prompts.get_mut(&app).unwrap() = Some("project-prompt".into());
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
    }

    #[test]
    #[serial]
    fn switching_project_profiles_removes_only_the_previous_managed_model_and_prompt() {
        let (_home, state) = test_state();
        for scope in [ProfileScope::Claude, ProfileScope::Codex] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            let app = scope.apps()[0].clone();
            seed_model_profile(&state, &root, scope);
            let (model_path, original) = match scope {
                ProfileScope::Claude => (
                    root.join(".claude/settings.local.json"),
                    "{\"userField\":true}",
                ),
                _ => (
                    root.join(".codex/config.toml"),
                    "# user comment\nmodel_provider = \"user-provider\"\n",
                ),
            };
            fs::create_dir_all(model_path.parent().unwrap()).unwrap();
            fs::write(&model_path, original).unwrap();
            assert!(matches!(
                test_projects::deploy(&state, "profile", scope, None).unwrap(),
                ProjectDeployResult::Applied { .. }
            ));
            let instruction_path = root.join(instruction_relative_path(&app).unwrap());
            assert_eq!(
                fs::read_to_string(&instruction_path).unwrap(),
                "project instructions"
            );
            let model = match scope {
                ProfileScope::Claude => ClaudeSettingsDocument::open(&root).unwrap().model(),
                _ => ProjectMcpDocument::open(&root, &app).unwrap().model(),
            };
            assert_eq!(model.as_deref(), Some("test-model"));
            assert!(
                matches!(test_projects::deploy(&state, "profile", scope, None).unwrap(),
                ProjectDeployResult::Applied { changed, .. } if changed.is_empty())
            );

            let mut next = state.db.get_profile("profile").unwrap().unwrap();
            next.id = "empty-profile".into();
            let mut payload: ProfilePayload = serde_json::from_str(&next.payload).unwrap();
            *payload.providers.get_mut(&app).unwrap() = None;
            *payload.prompts.get_mut(&app).unwrap() = Some(NO_ACTIVE_PROMPT.into());
            next.payload = serde_json::to_string(&payload).unwrap();
            state.db.save_profile(&next).unwrap();
            assert!(matches!(
                test_projects::deploy(&state, "empty-profile", scope, None).unwrap(),
                ProjectDeployResult::Applied { .. }
            ));
            assert!(!instruction_path.exists());
            match scope {
                ProfileScope::Claude => {
                    let doc = ClaudeSettingsDocument::open(&root).unwrap();
                    assert_eq!(doc.model(), None);
                    assert_eq!(doc.root["userField"], true);
                }
                _ => {
                    let saved = fs::read_to_string(&model_path).unwrap();
                    assert!(saved.contains("# user comment"));
                    let doc: toml_edit::DocumentMut = saved.parse().unwrap();
                    assert!(doc.get("model").is_none());
                    assert_eq!(doc["model_provider"].as_str(), Some("user-provider"));
                }
            }
            let manifest = load_manifest(&root, app.as_str()).unwrap();
            assert_eq!(manifest.model_hash, None);
            assert_eq!(manifest.instruction_hash, None);
            assert_eq!(
                crate::mode::current::provider_for(
                    &state.db,
                    &app,
                    crate::mode::current::Purpose::InUse
                )
                .unwrap()
                .as_deref(),
                Some("model-provider")
            );
            assert!(state.db.get_prompt_application(&app).unwrap().is_none());
        }
    }

    #[test]
    #[serial]
    fn codex_profile_model_catalog_deploys_with_conflicts_and_manifest_ownership() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let catalog = serde_json::json!({
            "models": [{
                "model": "gpt-profile",
                "displayName": "Profile GPT",
                "contextWindow": 200000,
                "reasoningLevels": ["low", "high"]
            }],
            "autoReviewModel": "gpt-profile",
            "officialSpeedTiers": false
        });
        state
            .db
            .save_provider(
                &AppType::Codex,
                &crate::provider::Provider::with_id(
                    "catalog-provider".into(),
                    "Catalog provider".into(),
                    serde_json::json!({
                        "config": "model = \"provider-model\"\n",
                        "modelCatalog": catalog
                    }),
                    None,
                ),
            )
            .unwrap();
        crate::settings::set_current_provider(&AppType::Codex, Some("catalog-provider")).unwrap();
        seed_profile(
            &state,
            &root,
            ProfileScope::Codex,
            Some(vec![]),
            Some(vec![]),
        );
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.providers.codex = Some("catalog-provider".into());
        payload.models.codex = Some("gpt-profile".into());
        payload.model_settings.codex = Some(BTreeMap::new());
        payload.model_catalog.codex = Some(catalog.clone());
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();

        assert!(matches!(
            test_projects::deploy(&state, &profile.id, ProfileScope::Codex, None).unwrap(),
            ProjectDeployResult::Applied { .. }
        ));
        let project_catalog_path = root.join(".codex/cc-switch-model-catalog.json");
        let deployed: Value =
            serde_json::from_slice(&fs::read(&project_catalog_path).unwrap()).unwrap();
        assert_eq!(deployed["models"][0]["slug"], "gpt-profile");
        assert_eq!(deployed["models"][0]["display_name"], "Profile GPT");
        let reasoning_levels = deployed["models"][0]["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level["effort"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(reasoning_levels, ["low", "high"]);
        assert_eq!(
            deployed["models"][0]["auto_review_model_override"],
            "gpt-profile"
        );
        let config_text = fs::read_to_string(root.join(".codex/config.toml")).unwrap();
        let config: toml_edit::DocumentMut = config_text.parse().unwrap();
        assert_eq!(config["model"].as_str(), Some("gpt-profile"));
        assert!(config["model_catalog_json"]
            .as_str()
            .unwrap()
            .ends_with("cc-switch-model-catalog.json"));
        let manifest = load_manifest(&root, "codex").unwrap();
        assert!(manifest.codex_model_catalog_hash.is_some());
        assert!(manifest.codex_model_catalog_pointer_hash.is_some());
    }

    #[test]
    #[serial]
    fn codex_profile_catalog_without_provider_deploys_edits_and_preserves_connection() {
        use crate::services::profile::{ProfileConfig, ProfileService};

        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(
            root.join(".codex/config.toml"),
            "model_provider = \"project-endpoint\"\n[model_providers.project-endpoint]\nname = \"Keep connection\"\nbase_url = \"http://localhost:3000/v1\"\nwire_api = \"responses\"\n",
        ).unwrap();
        seed_profile(
            &state,
            &root,
            ProfileScope::Codex,
            Some(vec![]),
            Some(vec![]),
        );

        for (model, display_name) in [
            ("first-model", "First model"),
            ("second-model", "Edited model"),
        ] {
            let saved = ProfileService::update_config(
                &state,
                "profile",
                ProfileScope::Codex,
                ProfileConfig {
                    claude_project_connection: Default::default(),
                    follow_global_resources: false,
                    native_config: None,
                    provider_id: None,
                    model: model.into(),
                    model_settings: Some(BTreeMap::new()),
                    model_catalog: Some(serde_json::json!({
                        "models": [{ "model": model, "displayName": display_name, "contextWindow": 200000 }],
                        "autoReviewModel": model,
                        "officialSpeedTiers": false
                    })),
                    mcp_ids: vec![],
                    skill_ids: vec![],
                    prompt_id: None,
                },
                None,
            ).unwrap();
            assert!(matches!(
                test_projects::deploy(&state, &saved.id, ProfileScope::Codex, None).unwrap(),
                ProjectDeployResult::Applied { .. }
            ));
            let config: toml_edit::DocumentMut =
                fs::read_to_string(root.join(".codex/config.toml"))
                    .unwrap()
                    .parse()
                    .unwrap();
            assert_eq!(config["model"].as_str(), Some(model));
            assert_eq!(config["model_provider"].as_str(), Some("project-endpoint"));
            assert_eq!(
                config["model_providers"]["project-endpoint"]["base_url"].as_str(),
                Some("http://localhost:3000/v1")
            );
            let catalog: Value = serde_json::from_slice(
                &fs::read(root.join(".codex/cc-switch-model-catalog.json")).unwrap(),
            )
            .unwrap();
            let entry = catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["slug"] == model)
                .unwrap();
            assert_eq!(entry["display_name"], display_name);
            assert_eq!(entry["auto_review_model_override"], model);
            let payload: ProfilePayload = serde_json::from_str(&saved.payload).unwrap();
            assert_eq!(payload.providers.codex, None);
        }
        assert_eq!(state.db.get_current_profile_id("codex").unwrap(), None);
        assert!(!crate::codex_config::get_codex_config_path().exists());
    }

    #[test]
    fn project_skill_aliases_reject_duplicate_destinations_before_writes() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        for app in [AppType::Claude, AppType::Codex] {
            let mut desired = DesiredSkills::default();
            for id in ["original", "imported-copy"] {
                desired.entries.insert(
                    id.into(),
                    DesiredSkill {
                        id: id.into(),
                        directory: "shared-name".into(),
                        source: root.join(id),
                        hash: id.into(),
                    },
                );
            }
            let error =
                plan_skill_ops(&root, &app, &ProjectManifest::default(), &desired).unwrap_err();
            assert!(error.to_string().contains("多个 Skill 使用同一项目目录"));
            assert!(fs::read_dir(&root).unwrap().next().is_none());
        }
    }

    #[test]
    #[serial]
    fn profile_context_settings_roundtrip_import_edit_deploy_and_clear() {
        use crate::services::profile::{ProfileConfig, ProfileService};
        let (_home, state) = test_state();
        let source = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join(".codex")).unwrap();
        let original = "model='custom-model'\nmodel_context_window=1000000\nmodel_auto_compact_token_limit=900000\n[model_providers.keep]\nbase_url='https://example.test/v1'\n";
        fs::write(source.path().join(".codex/config.toml"), original).unwrap();
        let (imported, _) = ProjectProfileService::import_from_project(
            &state,
            "Context",
            ProfileScope::Codex,
            source.path().to_str().unwrap(),
        )
        .unwrap();
        let payload: ProfilePayload = serde_json::from_str(&imported.payload).unwrap();
        let settings = payload.model_settings.codex.unwrap();
        assert_eq!(settings["model_context_window"], "1000000");
        assert_eq!(settings["model_auto_compact_token_limit"], "900000");
        let target = tempfile::tempdir().unwrap();
        assert_eq!(state.db.get_profile_projects().unwrap().len(), 1);
        let copied = ProfileService::duplicate(
            &state,
            &imported.id,
            "Copy",
            ProfileScope::Codex,
            crate::database::ProfileTarget::Project,
        )
        .unwrap();
        test_projects::bind(&copied.id, target.path());
        let mut config = ProfileConfig {
            claude_project_connection: Default::default(),
            follow_global_resources: false,
            native_config: None,
            provider_id: None,
            model: "custom-model".into(),
            model_settings: Some(settings),
            model_catalog: Some(
                serde_json::json!({"models":[{"model":"custom-model", "base_instructions":"First line\nSecond line\tindent", "supports_parallel_tool_calls":false,"input_modalities":["text","image"],"reasoning_levels":["low","high"],"default_reasoning_level":"high"}]}),
            ),
            mcp_ids: vec![],
            skill_ids: vec![],
            prompt_id: None,
        };
        ProfileService::update_config(
            &state,
            &copied.id,
            ProfileScope::Codex,
            config.clone(),
            None,
        )
        .unwrap();
        assert!(matches!(
            test_projects::deploy(&state, &copied.id, ProfileScope::Codex, None).unwrap(),
            ProjectDeployResult::Applied { .. }
        ));
        let path = target.path().join(".codex/config.toml");
        let document: toml_edit::DocumentMut = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(document["model_context_window"].as_integer(), Some(1000000));
        assert_eq!(
            document["model_auto_compact_token_limit"].as_integer(),
            Some(900000)
        );
        let catalog: Value = serde_json::from_slice(
            &fs::read(target.path().join(".codex/cc-switch-model-catalog.json")).unwrap(),
        )
        .unwrap();
        let model = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["slug"] == "custom-model")
            .unwrap();
        assert_eq!(model["context_window"], 1000000);
        assert_eq!(
            model["base_instructions"],
            "First line\nSecond line\tindent"
        );
        assert_eq!(model["supports_parallel_tool_calls"], false);
        assert_eq!(
            fs::read_to_string(source.path().join(".codex/config.toml")).unwrap(),
            original
        );
        config
            .model_settings
            .as_mut()
            .unwrap()
            .insert("model_context_window".into(), "".into());
        config
            .model_settings
            .as_mut()
            .unwrap()
            .insert("model_auto_compact_token_limit".into(), "".into());
        ProfileService::update_config(
            &state,
            &copied.id,
            ProfileScope::Codex,
            config.clone(),
            None,
        )
        .unwrap();
        test_projects::deploy(&state, &copied.id, ProfileScope::Codex, None).unwrap();
        let document: toml_edit::DocumentMut = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert!(document.get("model_context_window").is_none());
        assert!(document.get("model_auto_compact_token_limit").is_none());
        let before = state.db.get_profile(&copied.id).unwrap().unwrap().payload;
        for invalid in ["-1", "0", "abc", "9223372036854775808"] {
            config
                .model_settings
                .as_mut()
                .unwrap()
                .insert("model_context_window".into(), invalid.into());
            assert!(ProfileService::update_config(
                &state,
                &copied.id,
                ProfileScope::Codex,
                config.clone(),
                None
            )
            .is_err());
            assert_eq!(
                state.db.get_profile(&copied.id).unwrap().unwrap().payload,
                before
            );
        }
        assert!(!crate::codex_config::get_codex_config_path().exists());
    }

    #[test]
    fn project_skill_aliases_follow_platform_case_sensitivity() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let mut desired = DesiredSkills::default();
        for (id, directory) in [("first", "Skill"), ("second", "skill")] {
            desired.entries.insert(
                id.into(),
                DesiredSkill {
                    id: id.into(),
                    directory: directory.into(),
                    source: root.join(id),
                    hash: id.into(),
                },
            );
        }
        let result = plan_skill_ops(
            &root,
            &AppType::Codex,
            &ProjectManifest::default(),
            &desired,
        );
        #[cfg(windows)]
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("多个 Skill 使用同一项目目录"));
        #[cfg(not(windows))]
        assert_eq!(result.unwrap().len(), 2);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
    }

    #[test]
    #[cfg(windows)]
    fn project_skill_aliases_reject_case_only_conflicts_with_previous_deployment() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let mut manifest = ProjectManifest::default();
        manifest.skills.insert(
            ".agents/skills/Skill".into(),
            ManagedSkill {
                id: "previous".into(),
                hash: "previous-hash".into(),
            },
        );
        let mut desired = DesiredSkills::default();
        desired.entries.insert(
            "next".into(),
            DesiredSkill {
                id: "next".into(),
                directory: "skill".into(),
                source: root.join("next"),
                hash: "next-hash".into(),
            },
        );
        let error = plan_skill_ops(&root, &AppType::Codex, &manifest, &desired).unwrap_err();
        assert!(error.to_string().contains("大小写冲突"));
        assert!(fs::read_dir(&root).unwrap().next().is_none());
    }

    #[test]
    #[serial]
    fn claude_profile_model_settings_detect_conflicts_and_preserve_user_edits() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_model_profile(&state, &root, ProfileScope::Claude);
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        let mut model_settings = BTreeMap::new();
        for key in crate::services::profile::CLAUDE_PROFILE_MODEL_KEYS {
            model_settings.insert((*key).to_string(), String::new());
        }
        model_settings.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
            "profile-sonnet".into(),
        );
        payload.model_settings.claude = Some(model_settings);
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();

        fs::create_dir_all(root.join(".claude")).unwrap();
        let settings_path = root.join(CLAUDE_PROJECT_SETTINGS);
        fs::write(
            &settings_path,
            r#"{"env":{"ANTHROPIC_DEFAULT_SONNET_MODEL":"manual-sonnet","USER_FLAG":"keep"}}"#,
        )
        .unwrap();
        let ProjectDeployResult::NeedsResolution { conflicts, .. } =
            test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap()
        else {
            panic!("existing model settings must require explicit resolution")
        };
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].path,
            ".claude/settings.local.json#env.ANTHROPIC_DEFAULT_SONNET_MODEL"
        );

        assert!(matches!(
            test_projects::deploy(
                &state,
                &profile.id,
                ProfileScope::Claude,
                Some(ConflictResolution::Overwrite),
            )
            .unwrap(),
            ProjectDeployResult::Applied { .. }
        ));
        let settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(
            settings["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
            "profile-sonnet"
        );
        assert_eq!(settings["env"]["USER_FLAG"], "keep");

        let sonnet_path = format!("{CLAUDE_PROJECT_SETTINGS}#env.ANTHROPIC_DEFAULT_SONNET_MODEL");
        let manifest = load_manifest(&root, "claude").unwrap();
        let expected_hash = text_hash("profile-sonnet");
        assert_eq!(
            manifest.model_settings_hashes.get(&sonnet_path),
            Some(&expected_hash)
        );
        let mut edited: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        edited.model_settings.claude.as_mut().unwrap().insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
            "next-sonnet".into(),
        );
        profile.payload = serde_json::to_string(&edited).unwrap();
        state.db.save_profile(&profile).unwrap();
        let mut settings: Value =
            serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        settings["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"] = "user-edited".into();
        fs::write(
            &settings_path,
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
        let ProjectDeployResult::NeedsResolution { conflicts, .. } =
            test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap()
        else {
            panic!("post-deploy edits must require resolution")
        };
        assert!(conflicts
            .iter()
            .any(|conflict| conflict.path == sonnet_path));
        test_projects::deploy(
            &state,
            &profile.id,
            ProfileScope::Claude,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        let settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(
            settings["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
            "user-edited"
        );
        assert!(!load_manifest(&root, "claude")
            .unwrap()
            .model_settings_hashes
            .contains_key(&sonnet_path));
    }

    #[test]
    #[serial]
    fn matching_user_model_and_instructions_are_not_claimed_or_later_removed() {
        let (_home, state) = test_state();
        for scope in [ProfileScope::Claude, ProfileScope::Codex] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            let app = scope.apps()[0].clone();
            seed_model_profile(&state, &root, scope);
            let (model_path, original) = match scope {
                ProfileScope::Claude => (
                    root.join(".claude/settings.local.json"),
                    "{\"model\":\"test-model\",\"env\":{\"ANTHROPIC_MODEL\":\"test-model\"},\"userField\":true}",
                ),
                _ => (
                    root.join(".codex/config.toml"),
                    "# user model\nmodel = \"test-model\"\n",
                ),
            };
            fs::create_dir_all(model_path.parent().unwrap()).unwrap();
            fs::write(&model_path, original).unwrap();
            let instruction_path = root.join(instruction_relative_path(&app).unwrap());
            fs::write(&instruction_path, "project instructions").unwrap();
            assert!(
                matches!(test_projects::deploy(&state, "profile", scope, None).unwrap(),
                ProjectDeployResult::Applied { changed, .. } if changed.is_empty())
            );
            let manifest = load_manifest(&root, app.as_str()).unwrap();
            assert_eq!(manifest.model_hash, None);
            assert_eq!(manifest.instruction_hash, None);
            let mut profile = state.db.get_profile("profile").unwrap().unwrap();
            let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
            *payload.providers.get_mut(&app).unwrap() = None;
            *payload.prompts.get_mut(&app).unwrap() = Some(NO_ACTIVE_PROMPT.into());
            profile.payload = serde_json::to_string(&payload).unwrap();
            state.db.save_profile(&profile).unwrap();
            test_projects::deploy(&state, "profile", scope, None).unwrap();
            assert_eq!(fs::read_to_string(model_path).unwrap(), original);
            assert_eq!(
                fs::read_to_string(instruction_path).unwrap(),
                "project instructions"
            );
        }
    }

    #[test]
    #[serial]
    fn project_models_use_registered_stack_members_and_keep_unavailable_snapshots() {
        let (_home, state) = test_state();
        let store = crate::live::engine::DeviceStore::for_device();
        for scope in [ProfileScope::Claude, ProfileScope::Codex] {
            let project = tempfile::tempdir().unwrap();
            let app = scope.apps()[0].clone();
            seed_model_profile(&state, project.path(), scope);
            let mut payload = ProfilePayload::default();
            let mut warnings = vec![];
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                None
            );
            *payload.mcp.get_mut(&app).unwrap() = Some(vec![]);
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                Some(None)
            );
            *payload.providers.get_mut(&app).unwrap() = Some("deleted-provider".into());
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                None
            );
            *payload.providers.get_mut(&app).unwrap() = Some("model-provider".into());
            state
                .db
                .save_provider(
                    &app,
                    &crate::provider::Provider::with_id(
                        "other-provider".into(),
                        "Other provider".into(),
                        serde_json::json!({}),
                        None,
                    ),
                )
                .unwrap();
            crate::settings::set_current_provider(&app, Some("other-provider")).unwrap();
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                None
            );
            crate::settings::set_current_provider(&app, Some("model-provider")).unwrap();
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                Some(Some("test-model".into()))
            );
            let original = state
                .db
                .get_provider_by_id("model-provider", &app)
                .unwrap()
                .unwrap();
            let mut empty = original.clone();
            empty.settings_config = serde_json::json!({});
            state.db.save_provider(&app, &empty).unwrap();
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                Some(None)
            );
            state.db.save_provider(&app, &original).unwrap();
            crate::mode::state::update(&store, |live| {
                let native = live.apps.entry(app.as_str().into()).or_default();
                native.mode = Some(crate::mode::state::Mode::Proxy);
                native.proxy_route = Some("model-provider".into());
                native.stack.members = vec!["model-provider".into()];
                native
                    .stack
                    .keys
                    .insert("registered".into(), "model-provider".into());
            })
            .unwrap();
            // Ordinary route mode must not emit reserved Stack model IDs.
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                Some(Some("test-model".into()))
            );
            crate::mode::state::update(&store, |live| {
                live.apps.get_mut(app.as_str()).unwrap().stack.enabled = true;
            })
            .unwrap();
            let route_model = desired_project_model(&state, &payload, &app, &mut warnings)
                .unwrap()
                .unwrap()
                .unwrap();
            if app == AppType::Codex {
                assert_eq!(route_model, "test-model");
            } else {
                assert_eq!(route_model, "ccs-claude-registered--test-model");
            }
            crate::mode::state::update(&store, |live| {
                live.apps.get_mut(app.as_str()).unwrap().proxy_route =
                    Some("other-provider".into());
            })
            .unwrap();
            let model = desired_project_model(&state, &payload, &app, &mut warnings)
                .unwrap()
                .unwrap()
                .unwrap();
            let crate::mode::stack::Resolved::Hit(target) =
                crate::mode::stack::resolve(state.db.as_ref(), &store, &app, &model).unwrap()
            else {
                panic!("project model must resolve to the selected provider")
            };
            assert_eq!(target.provider.id, "model-provider");
            assert_eq!(target.upstream_model, "test-model");
            crate::mode::state::update(&store, |live| {
                live.apps
                    .get_mut(app.as_str())
                    .unwrap()
                    .stack
                    .members
                    .clear();
            })
            .unwrap();
            assert_eq!(
                desired_project_model(&state, &payload, &app, &mut warnings).unwrap(),
                None
            );
            assert!(!warnings.is_empty());
        }
    }

    #[test]
    #[serial]
    fn claude_local_model_preserves_global_and_shared_settings_and_user_env() {
        let (_home, state) = test_state();
        let global_path = crate::config::get_claude_config_dir().join("settings.json");
        fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        let global = "{\"env\":{\"ANTHROPIC_MODEL\":\"global-model\"}}";
        fs::write(&global_path, global).unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_model_profile(&state, &root, ProfileScope::Claude);
        fs::create_dir_all(root.join(".claude")).unwrap();
        let project_path = root.join(".claude/settings.local.json");
        fs::write(
            &project_path,
            "{\"env\":{\"USER_FLAG\":\"keep\"},\"userField\":true}",
        )
        .unwrap();
        let shared_path = root.join(".claude/settings.json");
        let shared = "{\"model\":\"shared-model\",\"env\":{\"ANTHROPIC_MODEL\":\"shared-model\"}}";
        fs::write(&shared_path, shared).unwrap();
        let ProjectDeployResult::Applied { warnings, .. } =
            test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap()
        else {
            panic!("deploy should write personal project settings")
        };
        assert!(!warnings
            .iter()
            .any(|warning| warning.contains("优先级更高")));
        let mut doc = ClaudeSettingsDocument::open(&root).unwrap();
        assert_eq!(doc.model().as_deref(), Some("test-model"));
        assert_eq!(doc.env_model().as_deref(), Some("test-model"));
        assert_eq!(doc.root["env"]["USER_FLAG"], "keep");
        assert_eq!(doc.root["userField"], true);
        assert_eq!(fs::read_to_string(&global_path).unwrap(), global);
        assert_eq!(fs::read_to_string(&shared_path).unwrap(), shared);
        let manifest = load_manifest(&root, "claude").unwrap();
        assert_eq!(manifest.model_hash, Some(text_hash("test-model")));
        assert_eq!(
            manifest.claude_env_model_hash,
            Some(text_hash("test-model"))
        );
        doc.set_env_model(Some("user-edited-model")).unwrap();
        doc.save(&root).unwrap();
        let ProjectDeployResult::NeedsResolution { conflicts, .. } =
            test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap()
        else {
            panic!("user-edited env model must need resolution")
        };
        assert!(conflicts
            .iter()
            .any(|conflict| conflict.path.ends_with("#env.ANTHROPIC_MODEL")));
        test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        assert_eq!(
            ClaudeSettingsDocument::open(&root)
                .unwrap()
                .env_model()
                .as_deref(),
            Some("user-edited-model")
        );
        assert_eq!(
            load_manifest(&root, "claude")
                .unwrap()
                .claude_env_model_hash,
            None
        );
        test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        assert_eq!(
            ClaudeSettingsDocument::open(&root)
                .unwrap()
                .env_model()
                .as_deref(),
            Some("test-model")
        );
        assert_eq!(fs::read_to_string(&global_path).unwrap(), global);
    }

    #[test]
    #[serial]
    fn claude_keep_current_model_does_not_override_it_through_new_env() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_model_profile(&state, &root, ProfileScope::Claude);
        fs::create_dir_all(root.join(".claude")).unwrap();
        let path = root.join(".claude/settings.local.json");
        let original = "{\"model\":\"user-model\",\"env\":{\"USER_FLAG\":\"keep\"}}";
        fs::write(&path, original).unwrap();
        test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let manifest = load_manifest(&root, "claude").unwrap();
        assert_eq!(manifest.model_hash, None);
        assert_eq!(manifest.claude_env_model_hash, None);
    }

    #[test]
    fn manifest_rejects_a_different_claude_settings_target_without_writing() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let mut manifest = load_manifest(&root, "claude").unwrap();
        manifest.claude_settings_file = Some(".claude/settings.json".into());
        save_manifest(&root, "claude", &manifest).unwrap();
        let path = manifest_path(&root, "claude").unwrap();
        let before = fs::read(&path).unwrap();

        assert!(load_manifest(&root, "claude").is_err());
        assert_eq!(fs::read(path).unwrap(), before);
        assert!(!root.join(CLAUDE_PROJECT_SETTINGS).exists());
    }

    #[test]
    fn claude_settings_cas_tracks_sequential_writes_and_undo() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let path = root.join(".claude/settings.local.json");
        let original = b"{\"userField\":true}".to_vec();
        fs::write(&path, &original).unwrap();
        let mut doc = ClaudeSettingsDocument::open(&root).unwrap();
        doc.set_model(Some("new-model")).unwrap();
        doc.set_env_model(Some("new-model")).unwrap();
        let first_hash = doc.save(&root).unwrap().unwrap();
        assert_eq!(doc.initial_hash.as_deref(), Some(first_hash.as_str()));
        assert!(!doc.changed);
        assert_eq!(doc.save(&root).unwrap(), None);
        let first_bytes = fs::read(&path).unwrap();
        doc.root["skillOverrides"] = serde_json::json!({"fixture-skill":"off"});
        doc.changed = true;
        let second_hash = doc.save(&root).unwrap().unwrap();
        assert_eq!(doc.initial_hash.as_deref(), Some(second_hash.as_str()));
        let undo = vec![
            ProjectFileUndo {
                path: path.clone(),
                original: Some(original.clone()),
                applied_hash: Some(first_hash),
            },
            ProjectFileUndo {
                path: path.clone(),
                original: Some(first_bytes),
                applied_hash: Some(second_hash),
            },
        ];
        assert!(rollback_project_files(&undo).is_empty());
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    #[serial]
    fn claude_project_mcp_denials_are_scoped_to_unselected_library_resources() {
        let (_home, state) = test_state();
        for id in ["alpha", "beta"] {
            state
                .db
                .save_mcp_server(&McpServer {
                    id: id.into(),
                    name: id.into(),
                    server: serde_json::json!({"command":"echo","args":[id]}),
                    apps: McpApps {
                        claude: true,
                        ..McpApps::default()
                    },
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: vec![],
                })
                .unwrap();
        }
        let global_path = crate::config::get_claude_config_dir().join("settings.json");
        fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        let global = "{\"deniedMcpServers\":[{\"serverName\":\"policy-owned\"}]}";
        fs::write(&global_path, global).unwrap();
        let first = tempfile::tempdir().unwrap();
        let first_root = first.path().canonicalize().unwrap();
        fs::create_dir_all(first_root.join(".claude")).unwrap();
        fs::write(first_root.join(".claude/settings.local.json"), "{\"userField\":true,\"deniedMcpServers\":[{\"serverUrl\":\"https://manual.invalid/*\"}]}").unwrap();
        seed_profile(
            &state,
            &first_root,
            ProfileScope::Claude,
            Some(vec!["alpha".into()]),
            Some(vec![]),
        );
        test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        let settings = ClaudeSettingsDocument::open(&first_root).unwrap();
        let rules = claude_mcp_denial_rules(&settings.root).unwrap();
        assert!(claude_mcp_denial_group(&rules, "alpha").is_none());
        assert!(claude_mcp_denial_group(&rules, "beta").is_some());
        assert_eq!(settings.root["userField"], true);
        assert!(rules.contains(&serde_json::json!({"serverUrl":"https://manual.invalid/*"})));
        assert!(load_manifest(&first_root, "claude")
            .unwrap()
            .claude_mcp_denials
            .contains_key("beta"));
        let second = tempfile::tempdir().unwrap();
        let second_root = second.path().canonicalize().unwrap();
        let mut other = state.db.get_profile("profile").unwrap().unwrap();
        other.id = "other-project".into();
        test_projects::bind(&other.id, &second_root);
        let mut other_payload: ProfilePayload = serde_json::from_str(&other.payload).unwrap();
        other_payload.mcp.claude = Some(vec!["alpha".into(), "beta".into()]);
        other.payload = serde_json::to_string(&other_payload).unwrap();
        state.db.save_profile(&other).unwrap();
        test_projects::deploy(&state, &other.id, ProfileScope::Claude, None).unwrap();
        let second_mcp = fs::read(second_root.join(".mcp.json")).unwrap();
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.mcp.claude = Some(vec!["beta".into()]);
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        test_projects::deploy(&state, &profile.id, ProfileScope::Claude, None).unwrap();
        let rules =
            claude_mcp_denial_rules(&ClaudeSettingsDocument::open(&first_root).unwrap().root)
                .unwrap();
        assert!(claude_mcp_denial_group(&rules, "alpha").is_some());
        assert!(claude_mcp_denial_group(&rules, "beta").is_none());
        assert!(rules.contains(&serde_json::json!({"serverUrl":"https://manual.invalid/*"})));
        assert_eq!(fs::read(second_root.join(".mcp.json")).unwrap(), second_mcp);
        assert!(
            claude_mcp_denial_rules(&ClaudeSettingsDocument::open(&second_root).unwrap().root)
                .unwrap()
                .is_empty()
        );
        assert_eq!(fs::read_to_string(global_path).unwrap(), global);
        assert!(state
            .db
            .get_mcp_server_rows()
            .unwrap()
            .readable
            .values()
            .all(|server| server.apps.claude));
    }

    #[test]
    #[serial]
    fn claude_project_mcp_denials_preserve_user_rules_and_detect_duplicate_edits() {
        let (_home, state) = test_state();
        seed_mcp_and_skill(&state);
        let mut server = state.db.get_mcp_server_rows().unwrap().readable["filesystem"].clone();
        server.apps.claude = true;
        state.db.save_mcp_server(&server).unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_profile(
            &state,
            &root,
            ProfileScope::Claude,
            Some(vec!["filesystem".into()]),
            Some(vec![]),
        );
        fs::create_dir_all(root.join(".claude")).unwrap();
        let path = root.join(".claude/settings.local.json");
        let original = serde_json::json!({"deniedMcpServers":[
            {"serverName":"filesystem"}, {"serverName":"unmanaged"},
            {"serverUrl":"https://manual.invalid/*"}, {"serverCommand":["echo","unrelated"]}],
            "userField":true});
        fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let global_path = crate::config::get_claude_config_dir().join("settings.json");
        fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        let global = "{\"deniedMcpServers\":[{\"serverName\":\"filesystem\"}]}";
        let shared_path = root.join(".claude/settings.json");
        let shared = "{\"deniedMcpServers\":[{\"serverCommand\":[\"echo\",\"hello\"]}]}";
        fs::write(&global_path, global).unwrap();
        fs::write(&shared_path, shared).unwrap();
        let ProjectDeployResult::NeedsResolution {
            conflicts,
            warnings,
        } = test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap()
        else {
            panic!("user-authored MCP denial must require a resolution")
        };
        assert!(conflicts
            .iter()
            .any(|conflict| conflict.path.ends_with("#deniedMcpServers[filesystem]")));
        assert!(warnings.iter().any(|warning| warning.contains("用户设置")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains(".claude/settings.json")));
        test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        let kept: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(kept["deniedMcpServers"], original["deniedMcpServers"]);
        assert_eq!(kept["userField"], true);
        assert!(!load_manifest(&root, "claude")
            .unwrap()
            .claude_mcp_denials
            .contains_key("filesystem"));
        test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        let preserved =
            claude_mcp_denial_rules(&ClaudeSettingsDocument::open(&root).unwrap().root).unwrap();
        assert!(claude_mcp_denial_group(&preserved, "filesystem").is_none());
        for rule in original["deniedMcpServers"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
        {
            assert!(preserved.contains(rule));
        }
        assert_eq!(fs::read_to_string(&global_path).unwrap(), global);
        assert_eq!(fs::read_to_string(&shared_path).unwrap(), shared);
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.mcp.claude = Some(vec![]);
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        let mut edited = ClaudeSettingsDocument::open(&root).unwrap();
        edited.root["deniedMcpServers"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"serverName":"filesystem"}));
        fs::write(&path, serde_json::to_vec(&edited.root).unwrap()).unwrap();
        assert!(matches!(
            test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap(),
            ProjectDeployResult::NeedsResolution { .. }
        ));
        test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        let rules =
            claude_mcp_denial_rules(&ClaudeSettingsDocument::open(&root).unwrap().root).unwrap();
        assert_eq!(
            claude_mcp_denial_group(&rules, "filesystem")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // An identical hand-authored guard must stay user-owned.
        let unowned = tempfile::tempdir().unwrap();
        let unowned_root = unowned.path().canonicalize().unwrap();
        fs::create_dir_all(unowned_root.join(".claude")).unwrap();
        fs::write(
            unowned_root.join(".claude/settings.local.json"),
            "{\"deniedMcpServers\":[{\"serverName\":\"filesystem\"}]}",
        )
        .unwrap();
        seed_profile(
            &state,
            &unowned_root,
            ProfileScope::Claude,
            Some(vec![]),
            Some(vec![]),
        );
        test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        assert!(load_manifest(&unowned_root, "claude")
            .unwrap()
            .claude_mcp_denials
            .is_empty());
    }

    #[test]
    fn invalid_project_model_types_are_rejected_without_writing() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let claude = root.join(".claude/settings.local.json");
        fs::write(&claude, "{\"model\":42}").unwrap();
        assert!(ClaudeSettingsDocument::open(&root).is_err());
        assert_eq!(fs::read_to_string(&claude).unwrap(), "{\"model\":42}");
        for invalid in ["{\"env\":[]}", "{\"env\":{\"ANTHROPIC_MODEL\":42}}"] {
            fs::write(&claude, invalid).unwrap();
            assert!(ClaudeSettingsDocument::open(&root).is_err());
            assert_eq!(fs::read_to_string(&claude).unwrap(), invalid);
        }
        fs::create_dir_all(root.join(".codex")).unwrap();
        let codex = root.join(".codex/config.toml");
        fs::write(&codex, "[model]\nuser = true\n").unwrap();
        assert!(ProjectMcpDocument::open(&root, &AppType::Codex).is_err());
        assert_eq!(fs::read_to_string(codex).unwrap(), "[model]\nuser = true\n");
    }

    #[test]
    #[serial]
    fn failed_later_project_write_restores_model_and_instructions_and_keeps_concurrent_edits() {
        let (_home, _state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::create_dir_all(root.join(".claude")).unwrap();
        let settings_path = root.join(".claude/settings.local.json");
        let original_settings = "{\"model\":\"old-model\",\"userField\":true}";
        fs::write(&settings_path, original_settings).unwrap();
        let instruction_path = root.join("CLAUDE.md");
        fs::write(&instruction_path, "old instructions").unwrap();
        let mut manifest = load_manifest(&root, "claude").unwrap();
        manifest.model_hash = Some(text_hash("old-model"));
        manifest.instruction_hash = Some(text_hash("old instructions"));
        let mut settings = ClaudeSettingsDocument::open(&root).unwrap();
        let model = plan_model_op(
            ".claude/settings.local.json",
            Some(Some("new-model".into())),
            settings.model(),
            &manifest,
        )
        .unwrap();
        let instruction = plan_instruction_op(
            &root,
            &AppType::Claude,
            Some(Some("new instructions".into())),
            &manifest,
        )
        .unwrap()
        .unwrap();
        let mut undo = vec![];
        apply_model_op(
            &root,
            &AppType::Claude,
            "claude",
            Some(&model),
            None,
            &[],
            None,
            None,
            Some(&mut settings),
            None,
            &mut manifest,
            &mut vec![],
            &mut undo,
        )
        .unwrap();
        apply_instruction_op(
            &root,
            "claude",
            &instruction,
            None,
            &mut manifest,
            &mut vec![],
            &mut undo,
        )
        .unwrap();
        let mut mcp = ProjectMcpDocument::open(&root, &AppType::Claude).unwrap();
        mcp.set_entry("test", &serde_json::json!({"command":"echo"}))
            .unwrap();
        fs::write(root.join(".mcp.json"), "{\"userEdit\":true}").unwrap();
        assert!(mcp.save(&root).is_err());
        assert!(rollback_project_files(&undo).is_empty());
        assert_eq!(
            fs::read_to_string(&settings_path).unwrap(),
            original_settings
        );
        assert_eq!(
            fs::read_to_string(&instruction_path).unwrap(),
            "old instructions"
        );
        fs::write(&instruction_path, "user concurrent edit").unwrap();
        assert_eq!(rollback_project_files(&undo[1..]).len(), 1);
        assert_eq!(
            fs::read_to_string(instruction_path).unwrap(),
            "user concurrent edit"
        );
    }

    #[test]
    #[serial]
    fn failed_skill_batch_restores_previous_directories_and_manifest() {
        let (_home, _state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let old_destination = root.join(".claude/skills/managed");
        fs::create_dir_all(&old_destination).unwrap();
        fs::write(old_destination.join("SKILL.md"), "old skill").unwrap();
        let old_hash = path_state(&old_destination).unwrap().unwrap();

        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("SKILL.md"), "new managed skill").unwrap();
        let desired_hash = path_state(source.path()).unwrap().unwrap();
        let new_source = tempfile::tempdir().unwrap();
        fs::write(new_source.path().join("SKILL.md"), "new skill").unwrap();
        let new_hash = path_state(new_source.path()).unwrap().unwrap();

        let first_path = ".claude/skills/managed".to_string();
        let mut manifest = ProjectManifest {
            version: 1,
            project_root: root.to_string_lossy().into_owned(),
            app: "claude".to_string(),
            ..ProjectManifest::default()
        };
        manifest.skills.insert(
            first_path.clone(),
            ManagedSkill {
                id: "managed".to_string(),
                hash: old_hash.clone(),
            },
        );
        save_manifest(root, "claude", &manifest).unwrap();
        let manifest_file = manifest_path(root, "claude").unwrap();
        let original_manifest = read_optional(&manifest_file).unwrap();

        let operations = vec![
            SkillOperation {
                relative_path: first_path,
                id: "managed".to_string(),
                source: Some(source.path().to_path_buf()),
                desired_hash: Some(desired_hash),
                current_state: Some(old_hash),
                conflict: None,
                no_op: false,
            },
            SkillOperation {
                relative_path: ".claude/skills/new-skill".to_string(),
                id: "new-skill".to_string(),
                source: Some(new_source.path().to_path_buf()),
                desired_hash: Some(new_hash),
                current_state: None,
                conflict: None,
                no_op: false,
            },
            SkillOperation {
                relative_path: ".claude/skills/broken".to_string(),
                id: "broken".to_string(),
                source: Some(root.join("missing-source")),
                desired_hash: Some("missing-source-hash".to_string()),
                current_state: None,
                conflict: None,
                no_op: false,
            },
        ];
        let mut undo = Vec::new();
        assert!(apply_skill_ops(
            root,
            &mut manifest,
            &operations,
            None,
            "claude",
            &mut Vec::new(),
            &mut undo,
        )
        .is_err());

        assert!(rollback_skill_ops(&undo).is_empty());
        restore_manifest(&manifest_file, original_manifest.as_deref()).unwrap();
        assert_eq!(
            fs::read_to_string(old_destination.join("SKILL.md")).unwrap(),
            "old skill"
        );
        assert!(!root.join(".claude/skills/new-skill").exists());
        assert_eq!(read_optional(&manifest_file).unwrap(), original_manifest);
    }

    #[test]
    fn rollback_keeps_concurrent_skill_edits_and_the_backup() {
        let project = tempfile::tempdir().unwrap();
        let destination = project.path().join("skill");
        let backup = project.path().join("skill.bak");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("SKILL.md"), "user edit").unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("SKILL.md"), "old skill").unwrap();

        let errors = rollback_skill_ops(&[SkillUndo {
            destination: destination.clone(),
            backup: Some(backup.clone()),
            applied_state: Some("state-written-by-deploy".to_string()),
        }]);

        assert_eq!(errors.len(), 1);
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            "user edit"
        );
        assert_eq!(
            fs::read_to_string(backup.join("SKILL.md")).unwrap(),
            "old skill"
        );
    }

    #[test]
    fn project_skill_paths_are_agent_native_and_confined() {
        assert_eq!(
            skill_relative_path(&AppType::Claude, "cookbook").unwrap(),
            PathBuf::from(".claude/skills/cookbook")
        );
        assert_eq!(
            skill_relative_path(&AppType::Codex, "cookbook").unwrap(),
            PathBuf::from(".agents/skills/cookbook")
        );
        for invalid in ["../outside", "/absolute", "a/b", ".."] {
            assert!(skill_relative_path(&AppType::Claude, invalid).is_err());
        }
    }

    #[test]
    #[serial]
    fn project_manifest_rejects_skill_paths_outside_project() {
        let (_home, _state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let mut manifest = ProjectManifest {
            version: 1,
            project_root: root.to_string_lossy().into_owned(),
            app: "claude".to_string(),
            ..ProjectManifest::default()
        };
        manifest.skills.insert(
            "../../outside".to_string(),
            ManagedSkill {
                id: "skill-id".to_string(),
                hash: "hash".to_string(),
            },
        );
        save_manifest(&root, "claude", &manifest).unwrap();

        assert!(load_manifest(&root, "claude").is_err());
    }

    #[test]
    fn project_mcp_export_refuses_inline_env_and_header_credentials() {
        let inline = serde_json::json!({ "command": "node", "env": { "API_KEY": "secret-value" } });
        let headers = serde_json::json!({ "type": "http", "url": "https://example.test", "headers": { "Authorization": "Bearer secret" } });
        assert!(project_safe_mcp_spec(&inline, &AppType::Codex).is_err());
        assert!(project_safe_mcp_spec(&headers, &AppType::Claude).is_err());
        let safe_ref = serde_json::json!({ "command": "node", "env": { "API_KEY": "${API_KEY}" } });
        assert!(project_safe_mcp_spec(&safe_ref, &AppType::Claude).is_ok());
        assert!(project_safe_mcp_spec(&safe_ref, &AppType::Codex).is_err());
    }

    #[test]
    fn project_mcp_accepts_official_interpolation_and_benign_literals_but_not_secret_fallbacks() {
        let spec = serde_json::json!({
            "type": "http",
            "url": "${API_BASE_URL:-https://example.test}/mcp",
            "headers": {"Authorization": "Bearer ${API_KEY}", "Content-Type": "application/json"},
            "env": {"NODE_ENV": "production", "DEBUG": "1", "TOKEN": "${TOKEN}"}
        });
        assert!(project_safe_mcp_spec(&spec, &AppType::Claude).is_ok());
        assert!(project_safe_mcp_spec(&spec, &AppType::Codex).is_err());
        let safe_codex = serde_json::json!({
            "command": "node", "env": {"NODE_ENV": "production"}, "env_vars": ["TOKEN"],
            "env_http_headers": {"Authorization": "TOKEN"},
            "http_headers": {"Content-Type": "application/json"}
        });
        assert!(project_safe_mcp_spec(&safe_codex, &AppType::Codex).is_ok());
        let url_ref =
            serde_json::json!({"type":"http", "url":"https://example.test/mcp?api_key=${API_KEY}"});
        assert!(project_safe_mcp_spec(&url_ref, &AppType::Claude).is_ok());
        let url_query_ref =
            serde_json::json!({"url":"https://example.test/mcp?api_key=${API_KEY}&version=1"});
        assert!(project_safe_mcp_spec(&url_query_ref, &AppType::Claude).is_ok());
        let unsafe_base_ref =
            serde_json::json!({"url":"${BASE_URL:-https://example.test/mcp?token=literal-secret}"});
        assert!(project_safe_mcp_spec(&unsafe_base_ref, &AppType::Claude).is_err());
        assert!(project_safe_mcp_spec(&url_ref, &AppType::Codex).is_err());
        let headers = serde_json::json!({"url":"https://example.test/mcp", "headers":{"Content-Type":"application/json"}});
        let exported = project_safe_mcp_spec(&headers, &AppType::Codex).unwrap();
        assert_eq!(exported["http_headers"]["Content-Type"], "application/json");
        assert!(exported.get("headers").is_none());
        for secret in [
            "${TOKEN:-arbitrary-secret}",
            "Bearer ${TOKEN:-arbitrary-secret}",
            "Bearer secret ${TOKEN}",
        ] {
            let unsafe_spec = serde_json::json!({"url": "https://example.test/mcp", "headers": {"Authorization": secret}});
            assert!(project_safe_mcp_spec(&unsafe_spec, &AppType::Claude).is_err());
        }
        assert!(!is_env_reference("${TOKEN:-arbitrary-secret}"));
        let unsafe_env =
            serde_json::json!({"command": "node", "env": {"TOKEN": "${TOKEN:-arbitrary-secret}"}});
        assert!(project_safe_mcp_spec(&unsafe_env, &AppType::Claude).is_err());
    }

    #[test]
    fn project_mcp_export_validates_environment_reference_fields() {
        let safe = serde_json::json!({
            "url": "https://example.test/mcp",
            "bearer_token_env_var": "MCP_TOKEN",
            "env_http_headers": { "Authorization": "_AUTH_TOKEN2" }
        });
        assert!(project_safe_mcp_spec(&safe, &AppType::Codex).is_ok());
        for invalid in [
            serde_json::json!({ "bearer_token_env_var": "sk-secret" }),
            serde_json::json!({ "bearer_token_env_var": "Bearer secret" }),
            serde_json::json!({ "bearer_token_env_var": ["TOKEN"] }),
            serde_json::json!({ "bearer_token_env_var": "2TOKEN" }),
            serde_json::json!({ "env_http_headers": { "Authorization": "Bearer sk-secret" } }),
            serde_json::json!({ "env_http_headers": { "Authorization": { "token": "secret" } } }),
            serde_json::json!({ "env_http_headers": "TOKEN" }),
        ] {
            assert!(project_safe_mcp_spec(&invalid, &AppType::Codex).is_err());
        }
    }

    #[test]
    fn project_mcp_export_rejects_nested_secrets_and_malformed_credential_fields() {
        let nested_credentials = serde_json::json!({
            "command": "node",
            "credentials": { "authorization": { "token": "secret-value" } }
        });
        let api_key_argument = serde_json::json!({
            "command": "node",
            "args": ["--api-key=arbitrary-secret"]
        });
        let token_argument = serde_json::json!({
            "command": "node",
            "args": ["--token=arbitrary-secret"]
        });
        let token_env_reference = serde_json::json!({
            "command": "node",
            "args": ["--token=${TOKEN}"]
        });
        let invalid_env = serde_json::json!({ "command": "node", "env": "API_KEY=secret" });
        let invalid_headers = serde_json::json!({
            "type": "http",
            "url": "https://example.test",
            "headers": "Authorization: Bearer secret-value"
        });

        assert!(project_safe_mcp_spec(&nested_credentials, &AppType::Claude).is_err());
        assert!(project_safe_mcp_spec(&api_key_argument, &AppType::Claude).is_err());
        assert!(project_safe_mcp_spec(&token_argument, &AppType::Claude).is_err());
        assert!(project_safe_mcp_spec(&token_env_reference, &AppType::Claude).is_ok());
        assert!(project_safe_mcp_spec(&invalid_env, &AppType::Claude).is_err());
        assert!(project_safe_mcp_spec(&invalid_headers, &AppType::Claude).is_err());
    }

    #[test]
    #[serial]
    fn deploys_claude_skills_and_mcp_without_changing_global_enable_flags() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        seed_mcp_and_skill(&state);
        seed_profile(
            &state,
            project.path(),
            ProfileScope::Claude,
            Some(vec!["filesystem".to_string()]),
            Some(vec!["skill-id".to_string()]),
        );

        let result = test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        assert!(matches!(result, ProjectDeployResult::Applied { .. }));
        let config: Value =
            serde_json::from_slice(&fs::read(project.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(config["mcpServers"]["filesystem"]["command"], "echo");
        assert_eq!(
            fs::read_to_string(project.path().join(".claude/skills/project-skill/SKILL.md"))
                .unwrap(),
            "---\nname: project-skill\ndescription: test\n---\n"
        );
        let repeated =
            test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        assert!(matches!(
            repeated,
            ProjectDeployResult::Applied { changed, .. } if changed.is_empty()
        ));
        assert!(
            !state
                .db
                .get_all_mcp_servers()
                .unwrap()
                .get("filesystem")
                .unwrap()
                .apps
                .claude
        );
        assert!(
            !state
                .db
                .get_installed_skill("skill-id")
                .unwrap()
                .unwrap()
                .apps
                .claude
        );
    }

    #[test]
    #[serial]
    fn deploys_codex_mcp_and_skills_without_replacing_user_toml() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        seed_mcp_and_skill(&state);
        fs::create_dir_all(project.path().join(".codex")).unwrap();
        fs::write(
            project.path().join(".codex/config.toml"),
            "model = \"gpt-5\"\nmodel_provider = \"user-provider\"\n\n[mcp_servers.user]\ncommand = \"keep\"\n",
        )
        .unwrap();
        seed_profile(
            &state,
            project.path(),
            ProfileScope::Codex,
            Some(vec!["filesystem".to_string()]),
            Some(vec!["skill-id".to_string()]),
        );

        let result = test_projects::deploy(&state, "profile", ProfileScope::Codex, None).unwrap();
        assert!(matches!(result, ProjectDeployResult::Applied { .. }));
        let document: toml_edit::DocumentMut =
            fs::read_to_string(project.path().join(".codex/config.toml"))
                .unwrap()
                .parse()
                .unwrap();
        assert_eq!(document["model"].as_str(), Some("gpt-5"));
        assert_eq!(document["model_provider"].as_str(), Some("user-provider"));
        assert_eq!(
            document["mcp_servers"]["user"]["command"].as_str(),
            Some("keep")
        );
        assert_eq!(
            document["mcp_servers"]["filesystem"]["command"].as_str(),
            Some("echo")
        );
        assert!(project
            .path()
            .join(".agents/skills/project-skill/SKILL.md")
            .is_file());
        let repeated = test_projects::deploy(&state, "profile", ProfileScope::Codex, None).unwrap();
        assert!(matches!(
            repeated,
            ProjectDeployResult::Applied { changed, .. } if changed.is_empty()
        ));
    }

    #[test]
    #[serial]
    fn claude_project_skill_selection_controls_managed_global_skills_by_native_name() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_mcp_and_skill(&state);
        let alpha_source = SkillService::get_installed_skill_dir("project-skill").unwrap();
        fs::write(
            alpha_source.join("SKILL.md"),
            "---\nname: alpha-native\ndescription: Alpha\n---\nAlpha instructions",
        )
        .unwrap();
        let mut alpha = state.db.get_installed_skill("skill-id").unwrap().unwrap();
        alpha.apps.claude = true;
        state.db.save_skill(&alpha).unwrap();
        let beta = InstalledSkill {
            id: "beta-id".into(),
            name: "Beta display label".into(),
            directory: "beta-directory".into(),
            ..alpha.clone()
        };
        state.db.save_skill(&beta).unwrap();
        let beta_source = SkillService::get_installed_skill_dir(&beta.directory).unwrap();
        fs::create_dir_all(&beta_source).unwrap();
        fs::write(
            beta_source.join("SKILL.md"),
            "---\nname: beta-native\ndescription: Beta\n---\nBeta instructions",
        )
        .unwrap();
        let global = SkillService::get_app_skills_dir(&AppType::Claude).unwrap();
        copy_tree(&alpha_source, &global.join(&alpha.directory)).unwrap();
        copy_tree(&beta_source, &global.join(&beta.directory)).unwrap();
        let original_alpha = fs::read(global.join(&alpha.directory).join("SKILL.md")).unwrap();
        let original_beta = fs::read(global.join(&beta.directory).join("SKILL.md")).unwrap();
        let settings_path = root.join(".claude/settings.local.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(&settings_path, r#"{"userField":true,"skillOverrides":{"manual-user":"user-invocable-only","plugin:skill":"off"}}"#).unwrap();
        seed_profile(
            &state,
            &root,
            ProfileScope::Claude,
            Some(vec![]),
            Some(vec![alpha.id.clone()]),
        );
        let mut a = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&a.payload).unwrap();
        payload.providers.claude = Some("missing-provider".into());
        a.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&a).unwrap();
        let mut b = a.clone();
        b.id = "beta-profile".into();
        payload.skills.claude = Some(vec![beta.id.clone()]);
        b.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&b).unwrap();
        test_projects::deploy(&state, &a.id, ProfileScope::Claude, None).unwrap();
        let settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["skillOverrides"]["project-skill"], "on");
        assert_eq!(settings["skillOverrides"]["beta-directory"], "off");
        assert!(settings["skillOverrides"].get("alpha-native").is_none());
        test_projects::deploy(&state, &b.id, ProfileScope::Claude, None).unwrap();
        let settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["skillOverrides"]["project-skill"], "off");
        assert_eq!(settings["skillOverrides"]["beta-directory"], "on");
        let manifest = load_manifest(&root, "claude").unwrap();
        assert_eq!(
            manifest.claude_skill_overrides["project-skill"].id,
            alpha.id
        );
        assert!(manifest
            .claude_skill_overrides
            .contains_key("beta-directory"));
        assert_eq!(
            settings["skillOverrides"]["manual-user"],
            "user-invocable-only"
        );
        assert_eq!(settings["skillOverrides"]["plugin:skill"], "off");
        assert_eq!(settings["userField"], true);
        assert!(!root.join(".claude/skills/project-skill").exists());
        assert!(root
            .join(".claude/skills/beta-directory/SKILL.md")
            .is_file());
        assert_eq!(
            fs::read(global.join(&alpha.directory).join("SKILL.md")).unwrap(),
            original_alpha
        );
        assert_eq!(
            fs::read(global.join(&beta.directory).join("SKILL.md")).unwrap(),
            original_beta
        );
        assert!(
            state
                .db
                .get_installed_skill(&alpha.id)
                .unwrap()
                .unwrap()
                .apps
                .claude
        );
        assert!(
            state
                .db
                .get_installed_skill(&beta.id)
                .unwrap()
                .unwrap()
                .apps
                .claude
        );
        // A real global disable removes the client directory as well as its flag.
        // Flipping only the DB bit leaves a native-visible, unregistered directory,
        // which must be protected rather than treated as globally disabled.
        SkillService::toggle_app(&state.db, &alpha.id, &AppType::Claude, false).unwrap();
        assert!(!global.join(&alpha.directory).exists());
        assert!(
            !state
                .db
                .get_installed_skill(&alpha.id)
                .unwrap()
                .unwrap()
                .apps
                .claude
        );
        test_projects::deploy(&state, &b.id, ProfileScope::Claude, None).unwrap();
        let settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        assert!(settings["skillOverrides"].get("project-skill").is_none());
        let manifest = load_manifest(&root, "claude").unwrap();
        assert!(!manifest
            .claude_skill_overrides
            .contains_key("project-skill"));
        assert!(manifest
            .claude_skill_overrides
            .contains_key("beta-directory"));
        assert_eq!(settings["skillOverrides"]["beta-directory"], "on");
        SkillService::toggle_app(&state.db, &alpha.id, &AppType::Claude, true).unwrap();
        assert!(global.join(&alpha.directory).join("SKILL.md").is_file());
        let manual = root.join(".claude/skills/project-skill/SKILL.md");
        fs::create_dir_all(manual.parent().unwrap()).unwrap();
        let manual_content =
            "---\nname: alpha-native\ndescription: User skill\n---\nUser instructions";
        fs::write(&manual, manual_content).unwrap();
        let mut settings: Value =
            serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        settings["skillOverrides"]["project-skill"] = Value::String("on".into());
        fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let result = test_projects::deploy(&state, &b.id, ProfileScope::Claude, None).unwrap();
        assert!(
            matches!(result, ProjectDeployResult::Applied { warnings, .. } if warnings.iter().any(|warning| warning.contains("未登记手写技能同名")))
        );
        let settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["skillOverrides"]["project-skill"], "on");
        assert_eq!(fs::read_to_string(&manual).unwrap(), manual_content);
        assert!(!load_manifest(&root, "claude")
            .unwrap()
            .claude_skill_overrides
            .contains_key("project-skill"));
    }

    #[test]
    #[serial]
    fn claude_skill_overrides_preserve_user_fields_blocked_sources_and_failed_deployments() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_mcp_and_skill(&state);
        seed_profile(
            &state,
            &root,
            ProfileScope::Claude,
            Some(vec!["filesystem".into()]),
            Some(vec!["skill-id".into()]),
        );
        let settings_path = root.join(".claude/settings.local.json");
        let shared_path = root.join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        let user = r#"{"skillOverrides":{"project-skill":"user-invocable-only","manual":"on"},"userField":true}"#;
        let shared = r#"{"skillOverrides":{"project-skill":"off"}}"#;
        fs::write(&settings_path, user).unwrap();
        fs::write(&shared_path, shared).unwrap();
        assert!(
            matches!(test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap(), ProjectDeployResult::NeedsResolution { conflicts, .. } if conflicts.iter().any(|conflict| conflict.path.ends_with("#skillOverrides.project-skill")))
        );
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), user);
        assert!(!root.join(".claude/skills/project-skill").exists());
        test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), user);
        assert!(load_manifest(&root, "claude")
            .unwrap()
            .claude_skill_overrides
            .is_empty());
        let result = test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        assert!(
            matches!(result, ProjectDeployResult::Applied { warnings, .. }
            if !warnings.iter().any(|warning| warning.contains("优先级更高")))
        );
        let saved = fs::read(&settings_path).unwrap();
        let settings: Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(settings["skillOverrides"]["project-skill"], "on");
        assert_eq!(settings["skillOverrides"]["manual"], "on");
        assert_eq!(fs::read_to_string(&shared_path).unwrap(), shared);
        let source = SkillService::get_installed_skill_dir("project-skill").unwrap();
        fs::rename(source.join("SKILL.md"), source.join("SKILL.backup")).unwrap();
        let manifest_before = fs::read(manifest_path(&root, "claude").unwrap()).unwrap();
        test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        assert_eq!(fs::read(&settings_path).unwrap(), saved);
        assert_eq!(
            fs::read(manifest_path(&root, "claude").unwrap()).unwrap(),
            manifest_before
        );
        fs::rename(source.join("SKILL.backup"), source.join("SKILL.md")).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let selected = state.db.get_profile("profile").unwrap().unwrap();
            let mut empty = selected.clone();
            empty.id = "empty-profile".into();
            let mut payload: ProfilePayload = serde_json::from_str(&empty.payload).unwrap();
            payload.skills.claude = Some(vec![]);
            payload.mcp.claude = Some(vec![]);
            empty.payload = serde_json::to_string(&payload).unwrap();
            state.db.save_profile(&empty).unwrap();
            let mcp_path = root.join(".mcp.json");
            let mcp_before = fs::read(&mcp_path).unwrap();
            let locked = fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&mcp_path)
                .unwrap();
            assert!(test_projects::deploy(&state, &empty.id, ProfileScope::Claude, None).is_err());
            assert_eq!(fs::read(&settings_path).unwrap(), saved);
            assert_eq!(fs::read(&mcp_path).unwrap(), mcp_before);
            assert_eq!(
                fs::read(manifest_path(&root, "claude").unwrap()).unwrap(),
                manifest_before
            );
            assert!(root.join(".claude/skills/project-skill/SKILL.md").is_file());
            drop(locked);
        }
    }

    #[test]
    #[serial]
    fn codex_mcp_selections_enable_and_cancellation_owns_a_disabled_entry() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_mcp_and_skill(&state);
        let global_path = crate::config::get_home_dir().join(".codex/config.toml");
        fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        let global = "[mcp_servers.filesystem]\ncommand = \"echo\"\nargs = [\"global\"]\nenabled = false\n\n[mcp_servers.extra]\ncommand = \"echo\"\n";
        fs::write(&global_path, global).unwrap();
        seed_profile(
            &state,
            &root,
            ProfileScope::Codex,
            Some(vec!["filesystem".into()]),
            None,
        );
        let selected = state.db.get_profile("profile").unwrap().unwrap();
        let mut cancelled = selected.clone();
        cancelled.id = "cancelled".into();
        let mut payload: ProfilePayload = serde_json::from_str(&cancelled.payload).unwrap();
        payload.mcp.codex = Some(vec![]);
        cancelled.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&cancelled).unwrap();
        let path = root.join(".codex/config.toml");

        test_projects::deploy(&state, &selected.id, ProfileScope::Codex, None).unwrap();
        let document: toml_edit::DocumentMut = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            document["mcp_servers"]["filesystem"]["enabled"].as_bool(),
            Some(true)
        );
        assert!(document["mcp_servers"].get("extra").is_none());

        test_projects::deploy(&state, &cancelled.id, ProfileScope::Codex, None).unwrap();
        let document: toml_edit::DocumentMut = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            document["mcp_servers"]["filesystem"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(
            document["mcp_servers"]["filesystem"]["command"].as_str(),
            Some("echo")
        );
        assert_eq!(
            document["mcp_servers"]["filesystem"]["args"]
                .as_array()
                .unwrap()
                .get(0)
                .unwrap()
                .as_str(),
            Some("hello")
        );
        let manifest = load_manifest(&root, "codex").unwrap();
        assert_eq!(
            manifest.mcp_servers.get("filesystem"),
            codex_entry_hash(&document, "filesystem").unwrap().as_ref()
        );
        assert!(
            matches!(test_projects::deploy(&state, &cancelled.id, ProfileScope::Codex, None).unwrap(), ProjectDeployResult::Applied { changed, .. } if changed.is_empty())
        );

        test_projects::deploy(&state, &selected.id, ProfileScope::Codex, None).unwrap();
        // Recover enough transport configuration to disable a removed managed table.
        fs::write(&path, "").unwrap();
        test_projects::deploy(&state, &cancelled.id, ProfileScope::Codex, None).unwrap();
        let document: toml_edit::DocumentMut = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            document["mcp_servers"]["filesystem"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(
            document["mcp_servers"]["filesystem"]["command"].as_str(),
            Some("echo")
        );
        assert_eq!(fs::read_to_string(&global_path).unwrap(), global);
        assert!(
            !state.db.get_all_mcp_servers().unwrap()["filesystem"]
                .apps
                .codex
        );
    }

    #[test]
    #[serial]
    fn codex_mcp_cancellation_requires_resolution_for_user_edits() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_mcp_and_skill(&state);
        seed_profile(
            &state,
            &root,
            ProfileScope::Codex,
            Some(vec!["filesystem".into()]),
            None,
        );
        let selected = state.db.get_profile("profile").unwrap().unwrap();
        let mut cancelled = selected.clone();
        cancelled.id = "cancelled".into();
        let mut payload: ProfilePayload = serde_json::from_str(&cancelled.payload).unwrap();
        payload.mcp.codex = Some(vec![]);
        cancelled.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&cancelled).unwrap();
        test_projects::deploy(&state, &selected.id, ProfileScope::Codex, None).unwrap();
        let path = root.join(".codex/config.toml");
        let mut document: toml_edit::DocumentMut =
            fs::read_to_string(&path).unwrap().parse().unwrap();
        document["mcp_servers"]["filesystem"]["command"] = toml_edit::value("user-command");
        let edited = document.to_string();
        fs::write(&path, &edited).unwrap();
        assert!(matches!(
            test_projects::deploy(&state, &cancelled.id, ProfileScope::Codex, None).unwrap(),
            ProjectDeployResult::NeedsResolution { .. }
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        test_projects::deploy(
            &state,
            &cancelled.id,
            ProfileScope::Codex,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        assert!(!load_manifest(&root, "codex")
            .unwrap()
            .mcp_servers
            .contains_key("filesystem"));
        test_projects::deploy(&state, &cancelled.id, ProfileScope::Codex, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);

        test_projects::deploy(
            &state,
            &selected.id,
            ProfileScope::Codex,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        let mut document: toml_edit::DocumentMut =
            fs::read_to_string(&path).unwrap().parse().unwrap();
        document["mcp_servers"]["filesystem"]["command"] = toml_edit::value("user-command");
        fs::write(&path, document.to_string()).unwrap();
        test_projects::deploy(
            &state,
            &cancelled.id,
            ProfileScope::Codex,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        let document: toml_edit::DocumentMut = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            document["mcp_servers"]["filesystem"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(
            document["mcp_servers"]["filesystem"]["command"].as_str(),
            Some("user-command")
        );
    }

    #[test]
    #[serial]
    fn codex_instruction_deployment_warns_when_override_shadows_agents_md() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        seed_profile(&state, &root, ProfileScope::Codex, Some(vec![]), None);
        state
            .db
            .save_prompt(&crate::prompt::Prompt {
                id: "instructions".into(),
                name: "Instructions".into(),
                content: "profile instructions".into(),
                description: None,

                created_at: None,
                updated_at: None,
            })
            .unwrap();
        let mut profile = state.db.get_profile("profile").unwrap().unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload).unwrap();
        payload.prompts.codex = Some("instructions".into());
        profile.payload = serde_json::to_string(&payload).unwrap();
        state.db.save_profile(&profile).unwrap();
        let override_path = root.join("AGENTS.override.md");
        fs::write(&override_path, "user override").unwrap();
        let result = test_projects::deploy(&state, &profile.id, ProfileScope::Codex, None).unwrap();
        assert!(
            matches!(result, ProjectDeployResult::Applied { warnings, .. } if warnings.iter().any(|warning| warning.contains("AGENTS.override.md") && warning.contains("忽略")))
        );
        assert_eq!(fs::read_to_string(&override_path).unwrap(), "user override");
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "profile instructions"
        );
        fs::write(&override_path, "").unwrap();
        let result = test_projects::deploy(&state, &profile.id, ProfileScope::Codex, None).unwrap();
        assert!(
            matches!(result, ProjectDeployResult::Applied { warnings, .. } if !warnings.iter().any(|warning| warning.contains("AGENTS.override.md")))
        );
    }

    #[test]
    #[serial]
    fn project_conflicts_require_a_choice_and_keep_current_releases_ownership() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        seed_mcp_and_skill(&state);
        seed_profile(
            &state,
            project.path(),
            ProfileScope::Claude,
            Some(vec!["filesystem".to_string()]),
            Some(vec!["skill-id".to_string()]),
        );
        test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();

        let config_path = project.path().join(".mcp.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["mcpServers"]["filesystem"]["args"] = serde_json::json!(["user"]);
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let skill_path = project.path().join(".claude/skills/project-skill/SKILL.md");
        fs::write(&skill_path, "user-edited skill").unwrap();

        let preview = test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        let ProjectDeployResult::NeedsResolution { conflicts, .. } = preview else {
            panic!("expected file ownership conflicts");
        };
        assert_eq!(conflicts.len(), 2);

        let result = test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::KeepCurrent),
        )
        .unwrap();
        assert!(matches!(result, ProjectDeployResult::Applied { .. }));
        let saved: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(
            saved["mcpServers"]["filesystem"]["args"],
            serde_json::json!(["user"])
        );
        assert_eq!(fs::read_to_string(skill_path).unwrap(), "user-edited skill");
    }

    #[test]
    #[serial]
    fn overwrite_resolution_replaces_only_the_owned_conflicting_resources() {
        let (_home, state) = test_state();
        let project = tempfile::tempdir().unwrap();
        seed_mcp_and_skill(&state);
        seed_profile(
            &state,
            project.path(),
            ProfileScope::Claude,
            Some(vec!["filesystem".to_string()]),
            Some(vec!["skill-id".to_string()]),
        );
        test_projects::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();

        let config_path = project.path().join(".mcp.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["mcpServers"]["filesystem"]["args"] = serde_json::json!(["user"]);
        config["userField"] = serde_json::json!(true);
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let skill_path = project.path().join(".claude/skills/project-skill/SKILL.md");
        fs::write(&skill_path, "user-edited skill").unwrap();

        let result = test_projects::deploy(
            &state,
            "profile",
            ProfileScope::Claude,
            Some(ConflictResolution::Overwrite),
        )
        .unwrap();
        assert!(matches!(result, ProjectDeployResult::Applied { .. }));
        let saved: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(saved["userField"], true);
        assert_eq!(
            saved["mcpServers"]["filesystem"]["args"],
            serde_json::json!(["hello"])
        );
        assert_eq!(
            fs::read_to_string(skill_path).unwrap(),
            "---\nname: project-skill\ndescription: test\n---\n"
        );
    }
}
