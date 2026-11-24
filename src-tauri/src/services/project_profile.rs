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
use crate::services::profile::{ProfilePayload, ProfileScope};
use crate::services::skill::SkillService;
use crate::store::AppState;

static PROJECT_DEPLOY_LOCK: Mutex<()> = Mutex::new(());

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
    NeedsResolution {
        conflicts: Vec<ProjectDeployConflict>,
        warnings: Vec<String>,
    },
    Applied {
        changed: Vec<String>,
        warnings: Vec<String>,
    },
    Skipped,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectManifest {
    version: u8,
    project_root: String,
    app: String,
    mcp_servers: BTreeMap<String, String>,
    skills: BTreeMap<String, ManagedSkill>,
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
    pub fn deploy(
        state: &AppState,
        profile_id: &str,
        scope: ProfileScope,
        resolution: Option<ConflictResolution>,
    ) -> Result<ProjectDeployResult, AppError> {
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
        let project_path = profile
            .project_path
            .as_deref()
            .ok_or_else(|| AppError::InvalidInput("Profile 未绑定项目目录".to_string()))?;
        let project_root = Path::new(project_path)
            .canonicalize()
            .map_err(|err| AppError::io(Path::new(project_path), err))?;
        if !project_root.is_dir() {
            return Err(AppError::InvalidInput("项目目录不可用".to_string()));
        }

        let payload: ProfilePayload = serde_json::from_str(&profile.payload)
            .map_err(|err| AppError::Config(format!("解析 Profile payload 失败: {err}")))?;
        let mut manifest = load_manifest(&project_root, app_name)?;

        let mut warnings = Vec::new();
        let mut changed = Vec::new();
        let mcp_ids = payload.mcp.get(&app).and_then(Option::as_ref);
        let skill_ids = payload.skills.get(&app).and_then(Option::as_ref);

        let mut mcp_doc = if mcp_ids.is_some() {
            Some(ProjectMcpDocument::open(&project_root, &app)?)
        } else {
            None
        };

        let desired_mcp = if let Some(ids) = mcp_ids {
            load_desired_mcp(state.db.as_ref(), ids, &app, &mut warnings)?
        } else {
            DesiredMcp::default()
        };
        let blocked_mcp = desired_mcp.blocked_ids.clone();
        let mcp_ops = if let Some(doc) = mcp_doc.as_ref() {
            plan_mcp_ops(doc, &manifest, &desired_mcp.entries, &blocked_mcp)?
        } else {
            Vec::new()
        };

        let desired_skills = if let Some(ids) = skill_ids {
            load_desired_skills(state.db.as_ref(), ids, &mut warnings)?
        } else {
            DesiredSkills::default()
        };
        let skill_ops = if skill_ids.is_some() {
            plan_skill_ops(&project_root, &app, &manifest, &desired_skills)?
        } else {
            Vec::new()
        };

        let conflicts = mcp_ops
            .iter()
            .filter_map(|op| op.conflict.clone())
            .chain(skill_ops.iter().filter_map(|op| op.conflict.clone()))
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

        if let Some(mut doc) = mcp_doc.take() {
            let previous_mcp_manifest = manifest.mcp_servers.clone();
            if let Err(error) = apply_mcp_ops(
                &mut doc,
                &mut manifest,
                &mcp_ops,
                resolution,
                &project_root,
                app_name,
                &mut changed,
            ) {
                manifest.mcp_servers = previous_mcp_manifest;
                if let Err(restore_error) = save_manifest(&project_root, app_name, &manifest) {
                    return Err(AppError::Config(format!(
                        "规划项目 MCP 失败: {error}; 恢复部署清单也失败: {restore_error}"
                    )));
                }
                return Err(error);
            }
            match doc.save(&project_root) {
                Ok(true) => changed.push(match app {
                    AppType::Claude => ".mcp.json".to_string(),
                    _ => ".codex/config.toml".to_string(),
                }),
                Ok(false) => {}
                Err(error) => {
                    manifest.mcp_servers = previous_mcp_manifest;
                    if let Err(restore_error) = save_manifest(&project_root, app_name, &manifest) {
                        return Err(AppError::Config(format!(
                            "写入项目 MCP 失败: {error}; 恢复部署清单也失败: {restore_error}"
                        )));
                    }
                    return Err(error);
                }
            }
        }
        apply_skill_ops(
            &project_root,
            &mut manifest,
            &skill_ops,
            resolution,
            app_name,
            &mut changed,
        )?;
        Ok(ProjectDeployResult::Applied { changed, warnings })
    }
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
    warnings: &mut Vec<String>,
) -> Result<DesiredMcp, AppError> {
    let rows = db.get_mcp_server_rows()?;
    let mut desired = DesiredMcp::default();
    for id in ids {
        let Some(server) = rows.readable.get(id) else {
            desired.blocked_ids.insert(id.clone());
            warnings.push(format!("MCP '{id}' 已删除或不可读，跳过并保留项目现状"));
            continue;
        };
        let spec = match project_safe_mcp_spec(&server.server, app) {
            Ok(spec) => spec,
            Err(reason) => {
                desired.blocked_ids.insert(id.clone());
                let _ = reason;
                warnings.push(format!("sensitiveMcpSkipped:{id}"));
                continue;
            }
        };
        desired.entries.insert(id.clone(), spec);
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

    if let Some(env) = object.get("env") {
        let Some(env) = env.as_object() else {
            return Err("env 必须是对象；为避免凭据泄露，已跳过".to_string());
        };
        let has_private_values = env.values().any(|value| match value.as_str() {
            Some(value) if matches!(app, AppType::Claude) => !is_env_reference(value),
            Some(value) => !value.is_empty(),
            None => !value.is_null(),
        });
        if has_private_values {
            return Err("包含内联环境变量值；请先改用环境变量引用".to_string());
        }
    }

    for key in ["headers", "http_headers"] {
        if let Some(headers) = object.get(key) {
            let Some(headers) = headers.as_object() else {
                return Err(format!(
                    "{key} 必须是对象；为避免把凭据写入项目文件，已跳过"
                ));
            };
            if headers.values().any(|value| !value.is_null()) {
                return Err("包含内联 HTTP 请求头；为避免把凭据写入项目文件，已跳过".to_string());
            }
        }
    }

    if contains_inline_secret(&spec) {
        return Err("包含疑似凭据；为避免写入项目文件，已跳过".to_string());
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
    !inner.trim().is_empty()
        && inner
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '-' | '.'))
}

fn contains_inline_secret(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let key_lower = key.to_ascii_lowercase();
            if matches!(
                key_lower.as_str(),
                "bearer_token_env_var" | "env_http_headers"
            ) {
                return false;
            }
            if key_lower == "url" && url_contains_credentials(value.as_str().unwrap_or_default()) {
                return true;
            }
            let secret_key = [
                "api_key",
                "apikey",
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
            .any(|needle| key_lower.contains(needle));
            if secret_key {
                return match value {
                    Value::String(value) => !value.is_empty() && !is_env_reference(value),
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
            let lower = text.to_ascii_lowercase();
            (text.contains("://") && url_contains_credentials(text))
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

fn url_contains_credentials(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    url.query_pairs().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        ["token", "key", "secret", "password", "auth", "credential"]
            .iter()
            .any(|marker| key.contains(marker))
            && !value.is_empty()
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
    warnings: &mut Vec<String>,
) -> Result<DesiredSkills, AppError> {
    let installed = db.get_all_installed_skills()?;
    let mut desired = DesiredSkills::default();
    for id in ids {
        let Some(skill) = installed.get(id) else {
            desired.blocked_ids.insert(id.clone());
            warnings.push(format!("Skill '{id}' 已删除，跳过并保留项目现状"));
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
                directory: skill.directory.clone(),
                source,
                hash,
            },
        );
    }
    Ok(desired)
}

fn plan_mcp_ops(
    document: &ProjectMcpDocument,
    manifest: &ProjectManifest,
    desired: &HashMap<String, Value>,
    blocked_ids: &HashSet<String>,
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
            let desired_value = desired.get(&id).cloned();
            let desired_hash = desired_value
                .as_ref()
                .map(|value| document.hash_after_set(&id, value))
                .transpose()?;
            let current_hash = document.entry_hash(&id)?;
            let prior_hash = manifest.mcp_servers.get(&id);
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
    for skill in desired.entries.values() {
        let relative_path = skill_relative_path(app, &skill.directory)?
            .to_string_lossy()
            .replace('\\', "/");
        desired_paths.insert(relative_path, skill);
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
                    replace_skill_directory(
                        source,
                        &destination,
                        Some(desired_hash),
                        operation.current_state.as_deref(),
                    )?;
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
                        remove_project_path(&destination)?;
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
    let staged = crate::config::stage_write(path, bytes, None, false)?;
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

fn hash_tree(path: &Path, reject_symlinks: bool) -> Result<String, AppError> {
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

fn validate_copy_source(source: &Path) -> Result<(), AppError> {
    let _ = hash_tree(source, true)?;
    if !source.join("SKILL.md").is_file() {
        return Err(AppError::InvalidInput(format!(
            "Skill 源目录缺少 SKILL.md: {}",
            source.display()
        )));
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), AppError> {
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
) -> Result<(), AppError> {
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

    verify_path_state(destination, expected_current_state)?;
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
            let _ = fs::rename(&backup, destination);
        }
        let _ = fs::remove_dir_all(&staged);
        return Err(AppError::io(destination, err));
    }
    if had_destination {
        if let Err(err) = remove_project_path(&backup) {
            log::warn!("清理项目 Skill 旧副本失败: {}: {err}", backup.display());
        }
    }
    Ok(())
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
            name: "Project profile".to_string(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            created_at: Some(1),
            updated_at: Some(1),
            project_path: Some(root.to_string_lossy().into_owned()),
        };
        state.db.save_profile(&profile).unwrap();
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

    fn test_state() -> (TempUserHome, AppState) {
        let home = TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        (home, state)
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
    fn project_mcp_export_rejects_nested_secrets_and_malformed_credential_fields() {
        let nested_credentials = serde_json::json!({
            "command": "node",
            "credentials": { "authorization": { "token": "secret-value" } }
        });
        let invalid_env = serde_json::json!({ "command": "node", "env": "API_KEY=secret" });
        let invalid_headers = serde_json::json!({
            "type": "http",
            "url": "https://example.test",
            "headers": "Authorization: Bearer secret-value"
        });

        assert!(project_safe_mcp_spec(&nested_credentials, &AppType::Claude).is_err());
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

        let result =
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
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
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
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

        let result =
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Codex, None).unwrap();
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
        let repeated =
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Codex, None).unwrap();
        assert!(matches!(
            repeated,
            ProjectDeployResult::Applied { changed, .. } if changed.is_empty()
        ));
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
        ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();

        let config_path = project.path().join(".mcp.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["mcpServers"]["filesystem"]["args"] = serde_json::json!(["user"]);
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let skill_path = project.path().join(".claude/skills/project-skill/SKILL.md");
        fs::write(&skill_path, "user-edited skill").unwrap();

        let preview =
            ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();
        let ProjectDeployResult::NeedsResolution { conflicts, .. } = preview else {
            panic!("expected file ownership conflicts");
        };
        assert_eq!(conflicts.len(), 2);

        let result = ProjectProfileService::deploy(
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
        ProjectProfileService::deploy(&state, "profile", ProfileScope::Claude, None).unwrap();

        let config_path = project.path().join(".mcp.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["mcpServers"]["filesystem"]["args"] = serde_json::json!(["user"]);
        config["userField"] = serde_json::json!(true);
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let skill_path = project.path().join(".claude/skills/project-skill/SKILL.md");
        fs::write(&skill_path, "user-edited skill").unwrap();

        let result = ProjectProfileService::deploy(
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
