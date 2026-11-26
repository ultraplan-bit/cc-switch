//! 项目 Profile 编排服务
//!
//! Profile 是**全应用共享的项目实体**（用户拥有的项目就那几个），payload
//! 按 app 分槽存配置快照（供应商 / 模型设置 / MCP / Skills / Prompt）。快照与应用
//! 均**按分组（scope）操作**：Claude Code 与 Codex 的工作目录往往不同
//! （各在各的项目里），因此各组独立指向自己的当前项目、只拍/只应用组内
//! 槽位，互不牵连；重命名/删除作用于共享实体本身。
//! 应用（apply）时复用现有切换原语批量落地：
//! - 供应商：`ProviderService::switch`（内建代理接管热切换与接管下禁切官方）
//! - MCP：`McpService::toggle_app`（改标志 + 单 server 物化）
//! - Skills：`SkillService::toggle_app`（改标志 + 单 skill 物化）
//! - Prompt：`PromptService::enable_prompt`（互斥激活 + 原子写 live）
//!
//! apply 为 best-effort：单项失败收集为 warning 继续，不整体回滚。

use std::collections::{BTreeMap, HashSet};
#[cfg(windows)]
use std::path::PathBuf;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::app_config::AppType;
use crate::database::Profile;
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::services::{McpService, PromptService, ProviderService, SkillService};
use crate::store::AppState;

mod migration;
pub(crate) mod native_config;

/// Profile 操作的应用分组：项目实体全应用共享，但快照/应用/当前指针按组进行。
///
/// Claude Code 与 Claude Desktop 的供应商在 cc-switch 中是独立切换的，
/// 因此各自拥有独立的项目分组。两者 live 文件零交集
///（`~/.claude` / `Application Support/Claude-3p`），分组切换互不干扰。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileScope {
    Claude,
    #[serde(rename = "claude-desktop")]
    ClaudeDesktop,
    Codex,
}

impl ProfileScope {
    /// 全部分组（扩展新分组时同步扩展 apps/for_app 与前端 scope.ts 镜像）
    pub const ALL: [ProfileScope; 3] = [
        ProfileScope::Claude,
        ProfileScope::ClaudeDesktop,
        ProfileScope::Codex,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileScope::Claude => "claude",
            ProfileScope::ClaudeDesktop => "claude-desktop",
            ProfileScope::Codex => "codex",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "claude" => Ok(ProfileScope::Claude),
            "claude-desktop" => Ok(ProfileScope::ClaudeDesktop),
            "codex" => Ok(ProfileScope::Codex),
            other => Err(AppError::InvalidInput(format!(
                "Unknown profile scope: {other}"
            ))),
        }
    }

    /// 组内受管应用（快照与 apply 只作用于这些 app 的槽位）
    pub fn apps(&self) -> &'static [AppType] {
        match self {
            ProfileScope::Claude => &[AppType::Claude],
            ProfileScope::ClaudeDesktop => &[AppType::ClaudeDesktop],
            ProfileScope::Codex => &[AppType::Codex],
        }
    }

    /// 应用页 → 所属分组（Profile 不支持的应用返回 None）
    pub fn for_app(app: &AppType) -> Option<Self> {
        match app {
            AppType::Claude => Some(ProfileScope::Claude),
            AppType::ClaudeDesktop => Some(ProfileScope::ClaudeDesktop),
            AppType::Codex => Some(ProfileScope::Codex),
            _ => None,
        }
    }
}

/// 提示词槽位的「拍到时没有启用中的提示词」标记（应用时停用当前启用项）。
///
/// 不能用 None 表示：None 已约定为「该侧从未拍过、不动」。提示词 id 不会是空串；
/// 旧版读到它会当作已删除的提示词跳过并给提示，不会误改配置。
pub const NO_ACTIVE_PROMPT: &str = "";

pub(crate) const CLAUDE_PROFILE_MODEL_KEYS: &[&str] = &[
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
    "CLAUDE_CODE_SUBAGENT_MODEL",
];

pub(crate) const CODEX_PROFILE_MODEL_KEYS: &[&str] = &[
    "model_reasoning_effort",
    "plan_mode_reasoning_effort",
    "model_context_window",
    "model_auto_compact_token_limit",
    "agents.default_subagent_model",
    "agents.default_subagent_reasoning_effort",
    "agents.max_threads",
];

pub(crate) fn is_codex_integer_model_setting(key: &str) -> bool {
    matches!(
        key,
        "model_context_window" | "model_auto_compact_token_limit" | "agents.max_threads"
    )
}

pub(crate) fn profile_model_keys(app: &AppType) -> &'static [&'static str] {
    match app {
        AppType::Claude => CLAUDE_PROFILE_MODEL_KEYS,
        AppType::Codex => CODEX_PROFILE_MODEL_KEYS,
        _ => &[],
    }
}

pub(crate) fn normalize_profile_model_catalog(
    value: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("Codex 模型目录必须是对象".to_string()))?;
    let rows = object
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| AppError::InvalidInput("Codex 模型目录必须包含 models 数组".to_string()))?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for row in rows {
        let mut row = row
            .as_object()
            .ok_or_else(|| AppError::InvalidInput("Codex 模型目录条目必须是对象".to_string()))?
            .clone();
        // Match the provider form's native-catalog compatibility mapping.
        for (canonical, native) in [
            ("displayName", "display_name"),
            ("contextWindow", "context_window"),
            ("supportsParallelToolCalls", "supports_parallel_tool_calls"),
            ("inputModalities", "input_modalities"),
            ("baseInstructions", "base_instructions"),
            ("reasoningLevels", "reasoning_levels"),
            ("defaultReasoningLevel", "default_reasoning_level"),
        ] {
            if !row.contains_key(canonical) {
                if let Some(value) = row.get(native).cloned() {
                    row.insert(canonical.into(), value);
                }
            }
        }
        let model = row
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|model| {
                !model.is_empty() && model.len() <= 1024 && !model.chars().any(char::is_control)
            })
            .ok_or_else(|| AppError::InvalidInput("Codex 模型名称无效".to_string()))?;
        if !seen.insert(model.to_string()) {
            return Err(AppError::InvalidInput(format!("Codex 模型重复: {model}")));
        }
        let mut safe = serde_json::Map::new();
        safe.insert("model".into(), model.into());
        for key in ["displayName", "baseInstructions", "defaultReasoningLevel"] {
            if let Some(value) = row.get(key) {
                let text = value.as_str().ok_or_else(|| {
                    AppError::InvalidInput(format!("Codex 模型目录字段 {key} 必须是字符串"))
                })?;
                let instructions = key == "baseInstructions";
                if text.len() > if instructions { 1024 * 1024 } else { 8192 }
                    || text.chars().any(|ch| {
                        ch.is_control() && !(instructions && matches!(ch, '\n' | '\r' | '\t'))
                    })
                {
                    return Err(AppError::InvalidInput(format!(
                        "Codex 模型目录字段 {key} 格式无效"
                    )));
                }
                safe.insert(key.into(), text.into());
            }
        }
        if let Some(value) = row.get("contextWindow") {
            let number = value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()));
            if value.as_str() == Some("") {
                // The provider form uses an empty input for the native default.
            } else if number.is_none_or(|number| number == 0) {
                return Err(AppError::InvalidInput(
                    "Codex contextWindow 必须是字符串或正整数".to_string(),
                ));
            } else {
                safe.insert("contextWindow".into(), number.unwrap().into());
            }
        }
        if let Some(value) = row.get("supportsParallelToolCalls") {
            if !value.is_boolean() {
                return Err(AppError::InvalidInput(
                    "Codex supportsParallelToolCalls 必须是布尔值".to_string(),
                ));
            }
            safe.insert("supportsParallelToolCalls".into(), value.clone());
        }
        for key in ["inputModalities", "reasoningLevels"] {
            if let Some(value) = row.get(key) {
                if !value
                    .as_array()
                    .is_some_and(|items| items.iter().all(serde_json::Value::is_string))
                {
                    return Err(AppError::InvalidInput(format!(
                        "Codex 模型目录字段 {key} 必须是字符串数组"
                    )));
                }
                safe.insert(key.into(), value.clone());
            }
        }
        models.push(serde_json::Value::Object(safe));
    }
    let mut catalog = serde_json::Map::new();
    catalog.insert("models".into(), models.into());
    if let Some(value) = object.get("autoReviewModel") {
        let model = value.as_str().ok_or_else(|| {
            AppError::InvalidInput("Codex autoReviewModel 必须是字符串".to_string())
        })?;
        if !model.is_empty() && !seen.contains(model) {
            return Err(AppError::InvalidInput(
                "Codex 自动 Review 模型必须存在于模型目录".to_string(),
            ));
        }
        catalog.insert("autoReviewModel".into(), model.into());
    }
    if let Some(value) = object.get("officialSpeedTiers") {
        if !value.is_boolean() {
            return Err(AppError::InvalidInput(
                "Codex officialSpeedTiers 必须是布尔值".to_string(),
            ));
        }
        catalog.insert("officialSpeedTiers".into(), value.clone());
    }
    Ok(serde_json::Value::Object(catalog))
}

fn capture_current_model_settings(app: &AppType) -> Result<BTreeMap<String, String>, AppError> {
    let mut settings = BTreeMap::new();
    match app {
        AppType::Claude => {
            let path = crate::config::get_claude_settings_path();
            if path.exists() {
                let document: serde_json::Value = crate::config::read_json_file(&path)?;
                if let Some(env) = document.get("env").and_then(serde_json::Value::as_object) {
                    for key in CLAUDE_PROFILE_MODEL_KEYS {
                        if let Some(value) = env.get(*key).and_then(serde_json::Value::as_str) {
                            settings.insert((*key).to_string(), value.to_string());
                        }
                    }
                }
            }
        }
        AppType::Codex => {
            let path = crate::codex_config::get_codex_config_path();
            if path.exists() {
                let text = crate::codex_config::read_codex_config_text()?;
                let document = text.parse::<toml_edit::DocumentMut>().map_err(|error| {
                    AppError::Config(format!("解析 Codex 模型设置失败: {error}"))
                })?;
                for key in CODEX_PROFILE_MODEL_KEYS {
                    let value = if let Some((section, field)) = key.split_once('.') {
                        document
                            .get(section)
                            .and_then(toml_edit::Item::as_table_like)
                            .and_then(|table| table.get(field))
                    } else {
                        document.get(key)
                    };
                    if let Some(value) = value {
                        let text = value
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| value.as_integer().map(|number| number.to_string()));
                        if let Some(text) = text {
                            settings.insert((*key).to_string(), text);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    for key in profile_model_keys(app) {
        settings.entry((*key).to_string()).or_default();
    }
    Ok(settings)
}

pub(crate) fn provider_model_settings(
    state: &AppState,
    app: &AppType,
    provider_id: &str,
) -> Result<Option<BTreeMap<String, String>>, AppError> {
    let Some(Ok(provider)) = state.db.read_provider(app, provider_id)? else {
        return Ok(None);
    };
    let mut settings = BTreeMap::new();
    match app {
        AppType::Claude => {
            if let Some(env) = provider
                .settings_config
                .get("env")
                .and_then(serde_json::Value::as_object)
            {
                for key in CLAUDE_PROFILE_MODEL_KEYS {
                    if let Some(value) = env.get(*key).and_then(serde_json::Value::as_str) {
                        settings.insert((*key).to_string(), value.to_string());
                    }
                }
            }
        }
        AppType::Codex => {
            if let Some(config) = provider
                .settings_config
                .get("config")
                .and_then(serde_json::Value::as_str)
            {
                let document = config.parse::<toml_edit::DocumentMut>().map_err(|error| {
                    AppError::Config(format!("解析 Codex 供应商模型设置失败: {error}"))
                })?;
                for key in CODEX_PROFILE_MODEL_KEYS {
                    let value = if let Some((section, field)) = key.split_once('.') {
                        document
                            .get(section)
                            .and_then(toml_edit::Item::as_table_like)
                            .and_then(|table| table.get(field))
                    } else {
                        document.get(key)
                    };
                    if let Some(value) = value {
                        let text = value
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| value.as_integer().map(|number| number.to_string()));
                        if let Some(text) = text {
                            settings.insert((*key).to_string(), text);
                        }
                    }
                }
            }
        }
        _ => return Ok(None),
    }
    Ok(Some(settings))
}

pub(crate) fn provider_model_catalog(
    state: &AppState,
    app: &AppType,
    provider_id: &str,
) -> Result<Option<serde_json::Value>, AppError> {
    let Some(Ok(provider)) = state.db.read_provider(app, provider_id)? else {
        return Ok(None);
    };
    Ok(Some(
        provider
            .settings_config
            .get("modelCatalog")
            .filter(|catalog| catalog.is_object())
            .cloned()
            .unwrap_or_else(|| {
                serde_json::json!({
                    "models": [],
                    "autoReviewModel": "",
                    "officialSpeedTiers": false
                })
            }),
    ))
}

pub(crate) fn set_codex_model_setting(
    document: &mut toml_edit::DocumentMut,
    path: &str,
    value: Option<&String>,
) -> Result<(), AppError> {
    if !CODEX_PROFILE_MODEL_KEYS.contains(&path) {
        return Err(AppError::InvalidInput(format!(
            "不支持的 Codex 项目模型字段: {path}"
        )));
    }
    let (table_name, key) = path
        .split_once('.')
        .map_or((None, path), |(table, key)| (Some(table), key));
    let value = match value {
        Some(value) if is_codex_integer_model_setting(path) => {
            let value = value
                .parse::<i64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| AppError::InvalidInput(format!("{path} 必须是正整数")))?;
            Some(toml_edit::value(value))
        }
        Some(value) => Some(toml_edit::value(value.as_str())),
        None => None,
    };
    let Some(table_name) = table_name else {
        let table = document.as_table_mut();
        match value {
            Some(value) => {
                table.insert(key, value);
            }
            None => {
                table.remove(key);
            }
        }
        return Ok(());
    };
    let root = document.as_table_mut();
    if !root.contains_key(table_name) && value.is_some() {
        root.insert(table_name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let Some(table) = root
        .get_mut(table_name)
        .and_then(toml_edit::Item::as_table_like_mut)
    else {
        return if value.is_none() {
            Ok(())
        } else {
            Err(AppError::Config(format!(
                "Codex config 的 {table_name} 必须是表"
            )))
        };
    };
    match value {
        Some(value) => {
            table.insert(key, value);
        }
        None => {
            table.remove(key);
        }
    }
    Ok(())
}

/// 按 app 分槽的载荷容器；字段名与 AppType 的 serde 形式一致
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerApp<T> {
    pub claude: T,
    #[serde(rename = "claude-desktop")]
    pub claude_desktop: T,
    pub codex: T,
}

impl<T> PerApp<T> {
    pub fn get(&self, app: &AppType) -> Option<&T> {
        match app {
            AppType::Claude => Some(&self.claude),
            AppType::ClaudeDesktop => Some(&self.claude_desktop),
            AppType::Codex => Some(&self.codex),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, app: &AppType) -> Option<&mut T> {
        match app {
            AppType::Claude => Some(&mut self.claude),
            AppType::ClaudeDesktop => Some(&mut self.claude_desktop),
            AppType::Codex => Some(&mut self.codex),
            _ => None,
        }
    }
}

/// Display-only names retained when a referenced resource is no longer readable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileResourceNames {
    pub mcp: BTreeMap<String, String>,
    pub skills: BTreeMap<String, String>,
    pub prompts: BTreeMap<String, String>,
}

/// Native project names retained when imported resources need distinct library IDs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileProjectAliases {
    pub mcp: BTreeMap<String, String>,
    pub skills: BTreeMap<String, String>,
}

pub(crate) fn resource_label(id: &str, name: Option<&String>) -> String {
    match name.filter(|name| !name.trim().is_empty() && name.as_str() != id) {
        Some(name) => format!("{name} ({id})"),
        None => id.to_string(),
    }
}

fn remember_resource_names<'a>(
    names: &mut BTreeMap<String, String>,
    selected: &[String],
    available: impl Iterator<Item = (&'a String, &'a String)>,
) {
    names.retain(|id, _| selected.contains(id));
    for (id, name) in available {
        if selected.contains(id) {
            names.insert(id.clone(), name.clone());
        }
    }
}

/// Profile 的 JSON 快照结构（与前端 TS 类型严格对应）
///
/// 配置槽位都是 Option：None = 该侧从未拍过快照（应用时不动），
/// 与"拍到的就是空集/无激活项"（Some(空)，应用时清空启用）严格区分——
/// 在 Codex 页选中一个只在 Claude 页建过的项目不能误清 Codex 的启用状态。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProfilePayload {
    /// Agent-native behavior excluding connection, typed model and library resource fields.
    pub native_config: PerApp<Option<String>>,
    /// 每 app 的当前供应商 id
    pub providers: PerApp<Option<String>>,
    /// 每 app 独立保存的模型；None = 旧快照未保存，空串 = 继承默认模型。
    pub models: PerApp<Option<String>>,
    /// 每 app 的 Agent 专属模型覆盖；空 map 表示拍到但无额外覆盖，None 表示旧快照未保存。
    #[serde(alias = "model_settings")]
    pub model_settings: PerApp<Option<BTreeMap<String, String>>>,
    /// Codex 的 Profile 专属模型目录（含 auto-review 与 speed-tier 设置）。
    #[serde(alias = "model_catalog")]
    pub model_catalog: PerApp<Option<serde_json::Value>>,
    /// 每 app 启用的 MCP server id 集合
    pub mcp: PerApp<Option<Vec<String>>>,
    /// 每 app 启用的 Skill id 集合
    pub skills: PerApp<Option<Vec<String>>>,
    /// 每 app 激活的 prompt id；Some([`NO_ACTIVE_PROMPT`]) = 拍到时没有启用项
    pub prompts: PerApp<Option<String>>,
    /// Resource names are display metadata, not copied definitions or deployment state.
    pub resource_names: PerApp<ProfileResourceNames>,
    pub project_aliases: PerApp<ProfileProjectAliases>,
}

/// 编辑一个 Agent 的供应商引用、模型设置与资源引用；不修改资源库定义或 Agent 当前配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileConfig {
    #[serde(default)]
    pub native_config: Option<String>,
    pub provider_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub model_settings: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub model_catalog: Option<serde_json::Value>,
    pub mcp_ids: Vec<String>,
    pub skill_ids: Vec<String>,
    pub prompt_id: Option<String>,
}

impl ProfilePayload {
    /// 用另一份快照覆盖本载荷中某分组的槽位，其余分组原样保留
    /// （"以当前状态更新"只更新发起页所属分组，避免把别的应用
    /// 正处于其他项目的状态串进来）
    pub fn merge_scope_from(&mut self, other: &ProfilePayload, scope: ProfileScope) {
        for app in scope.apps() {
            if let (Some(dst), Some(src)) = (
                self.native_config.get_mut(app),
                other.native_config.get(app),
            ) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (
                self.project_aliases.get_mut(app),
                other.project_aliases.get(app),
            ) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (self.providers.get_mut(app), other.providers.get(app))
            {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (self.models.get_mut(app), other.models.get(app)) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (
                self.model_settings.get_mut(app),
                other.model_settings.get(app),
            ) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (
                self.model_catalog.get_mut(app),
                other.model_catalog.get(app),
            ) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (self.mcp.get_mut(app), other.mcp.get(app)) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (self.skills.get_mut(app), other.skills.get(app)) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (self.prompts.get_mut(app), other.prompts.get(app)) {
                *dst = src.clone();
            }
            if let (Some(dst), Some(src)) = (
                self.resource_names.get_mut(app),
                other.resource_names.get(app),
            ) {
                *dst = src.clone();
            }
        }
    }

    /// 某分组是否拍过快照（任一槽位非 None 即视为拍过）
    pub fn scope_captured(&self, scope: ProfileScope) -> bool {
        scope.apps().iter().any(|app| {
            self.native_config.get(app).is_some_and(|s| s.is_some())
                || self.providers.get(app).is_some_and(|s| s.is_some())
                || self.models.get(app).is_some_and(|s| s.is_some())
                || self.model_settings.get(app).is_some_and(|s| s.is_some())
                || self.model_catalog.get(app).is_some_and(|s| s.is_some())
                || self.mcp.get(app).is_some_and(|s| s.is_some())
                || self.skills.get(app).is_some_and(|s| s.is_some())
                || self.prompts.get(app).is_some_and(|s| s.is_some())
        })
    }
}

/// 计算从当前启用状态到目标集合的最小 toggle 集
///
/// 返回 (需要执行的 (id, enabled) 列表, payload 中已不存在于 DB 的悬空 id 列表)
fn plan_toggles(
    current: &[(String, bool)],
    target_ids: &[String],
) -> (Vec<(String, bool)>, Vec<String>) {
    let existing: HashSet<&str> = current.iter().map(|(id, _)| id.as_str()).collect();
    let target: HashSet<&str> = target_ids.iter().map(|s| s.as_str()).collect();

    let toggles = current
        .iter()
        .filter(|(id, enabled)| target.contains(id.as_str()) != *enabled)
        .map(|(id, enabled)| (id.clone(), !enabled))
        .collect();

    let dangling = target_ids
        .iter()
        .filter(|id| !existing.contains(id.as_str()))
        .cloned()
        .collect();

    (toggles, dangling)
}

/// 归一化项目目录路径；空串表示解绑，不访问文件系统。
pub fn normalize_project_path(raw: &str) -> Result<Option<String>, AppError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    #[cfg(windows)]
    let normalized = raw.replace('/', "\\");
    #[cfg(not(windows))]
    let normalized = raw.to_string();

    let path = Path::new(&normalized);
    if !path.is_absolute() {
        return Err(AppError::InvalidInput(
            "项目目录路径必须是绝对路径".to_string(),
        ));
    }

    // 根路径保留一个尾分隔符，其余尾分隔符去掉；内部的 `..` 保持原样。
    let is_root = path
        .components()
        .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir));
    if is_root {
        #[cfg(windows)]
        {
            let mut root = PathBuf::new();
            for component in path.components() {
                root.push(component.as_os_str());
            }
            return Ok(Some(root.to_string_lossy().into_owned()));
        }
        #[cfg(not(windows))]
        return Ok(Some("/".to_string()));
    }

    #[cfg(windows)]
    let trimmed = normalized.trim_end_matches('\\');
    #[cfg(not(windows))]
    let trimmed = normalized.trim_end_matches('/');
    Ok(Some(trimmed.to_string()))
}

pub struct ProfileService;

impl ProfileService {
    /// Rehydrate the active Profile for proxy restart/reattach projection without
    /// changing the provider library or saving runtime state back into a Profile.
    pub fn effective_provider_for_applied_profile(
        state: &AppState,
        app: &AppType,
        provider: &crate::provider::Provider,
    ) -> Result<crate::provider::Provider, AppError> {
        let Some(scope) = ProfileScope::for_app(app) else {
            return Ok(provider.clone());
        };
        let Some(id) = state.db.get_current_profile_id(scope.as_str())? else {
            return Ok(provider.clone());
        };
        let Some(profile) = state.db.get_profile(&id)? else {
            return Ok(provider.clone());
        };
        if profile.project_path.is_some() {
            return Ok(provider.clone());
        }
        let payload: ProfilePayload = serde_json::from_str(&profile.payload)
            .map_err(|error| AppError::Config(error.to_string()))?;
        if payload
            .providers
            .get(app)
            .and_then(Option::as_deref)
            .is_some_and(|id| id != provider.id)
        {
            return Ok(provider.clone());
        }
        let mut result = provider.clone();
        if app == &AppType::Claude {
            let mut document = native_config::NativeDocument::Json(result.settings_config.clone());
            if let Some(native) = payload.native_config.get(app).and_then(Option::as_deref) {
                native_config::overlay_provider_behavior(
                    app,
                    &result.settings_config,
                    native,
                    &mut document,
                )?;
            }
            let native_config::NativeDocument::Json(mut config) = document else {
                unreachable!()
            };
            let root = config
                .as_object_mut()
                .ok_or_else(|| AppError::Config("供应商配置必须是对象".into()))?;
            let env = root
                .entry("env")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or_else(|| AppError::Config("供应商 env 必须是对象".into()))?;
            if let Some(model) = payload
                .models
                .get(app)
                .and_then(Option::as_deref)
                .filter(|model| !model.is_empty())
            {
                env.insert("ANTHROPIC_MODEL".into(), model.into());
            }
            if let Some(settings) = payload.model_settings.get(app).and_then(Option::as_ref) {
                for (key, value) in settings {
                    if value.is_empty() {
                        env.remove(key);
                    } else {
                        env.insert(key.clone(), value.clone().into());
                    }
                }
            }
            if root.contains_key("model") {
                if let Some(model) = payload
                    .models
                    .claude
                    .as_deref()
                    .filter(|model| !model.is_empty())
                {
                    root.insert("model".into(), model.into());
                }
            }
            result.settings_config = config;
        } else if app == &AppType::Codex {
            let text = result
                .settings_config
                .get("config")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let mut document = native_config::NativeDocument::parse(app, text)?;
            if let Some(native) = payload.native_config.codex.as_deref() {
                native_config::overlay_provider_behavior(
                    app,
                    &result.settings_config,
                    native,
                    &mut document,
                )?;
            }
            let native_config::NativeDocument::Toml(mut config) = document else {
                unreachable!()
            };
            if let Some(model) = payload
                .models
                .codex
                .as_deref()
                .filter(|model| !model.is_empty())
            {
                config["model"] = toml_edit::value(model);
            }
            if let Some(settings) = payload.model_settings.codex.as_ref() {
                for (key, value) in settings {
                    set_codex_model_setting(
                        &mut config,
                        key,
                        (!value.is_empty()).then_some(value),
                    )?;
                }
            }
            result.settings_config["config"] = config.to_string().into();
            if let Some(catalog) = payload.model_catalog.codex {
                result.settings_config["modelCatalog"] = catalog;
            }
        }
        Ok(result)
    }

    /// 抓取分组内应用的当前配置状态生成快照（组外槽位保持默认值）
    pub fn snapshot_current(
        state: &AppState,
        scope: ProfileScope,
    ) -> Result<ProfilePayload, AppError> {
        let mut payload = ProfilePayload::default();
        let mcp_servers = state
            .db
            .get_mcp_server_rows()?
            .into_readable("抓取配置快照");
        let skills = state.db.get_all_installed_skills()?;

        for app in scope.apps().iter() {
            *payload.native_config.get_mut(app).expect("validated app") =
                Some(native_config::capture(app)?);
            if let Some(slot) = payload.providers.get_mut(app) {
                // 和应用快照时一致：代理模式下记的是代理路由到的那家。
                *slot = crate::mode::current::provider_for(
                    &state.db,
                    app,
                    crate::mode::current::Purpose::InUse,
                )?;
            }
            if app == &AppType::Codex {
                let catalog = payload
                    .providers
                    .get(app)
                    .and_then(Option::as_deref)
                    .map(|provider_id| provider_model_catalog(state, app, provider_id))
                    .transpose()?
                    .flatten()
                    .unwrap_or_else(|| {
                        serde_json::json!({
                            "models": [],
                            "autoReviewModel": "",
                            "officialSpeedTiers": false
                        })
                    });
                *payload.model_catalog.get_mut(app).expect("validated app") = Some(catalog);
            }
            if let Some(slot) = payload.mcp.get_mut(app) {
                *slot = Some(
                    mcp_servers
                        .values()
                        .filter(|s| s.apps.is_enabled_for(app))
                        .map(|s| s.id.clone())
                        .collect(),
                );
            }
            if let Some(slot) = payload.skills.get_mut(app) {
                *slot = Some(
                    skills
                        .values()
                        .filter(|s| s.apps.is_enabled_for(app))
                        .map(|s| s.id.clone())
                        .collect(),
                );
            }
            let prompts = state.db.get_prompts(app)?;
            if let Some(slot) = payload.prompts.get_mut(app) {
                *slot = Some(
                    prompts
                        .values()
                        .find(|p| p.enabled)
                        .map_or_else(|| NO_ACTIVE_PROMPT.to_string(), |p| p.id.clone()),
                );
            }
            let names = payload.resource_names.get_mut(app).expect("validated app");
            remember_resource_names(
                &mut names.mcp,
                payload
                    .mcp
                    .get(app)
                    .and_then(Option::as_deref)
                    .unwrap_or_default(),
                mcp_servers
                    .values()
                    .map(|server| (&server.id, &server.name)),
            );
            remember_resource_names(
                &mut names.skills,
                payload
                    .skills
                    .get(app)
                    .and_then(Option::as_deref)
                    .unwrap_or_default(),
                skills.values().map(|skill| (&skill.id, &skill.name)),
            );
            remember_resource_names(
                &mut names.prompts,
                payload.prompts.get(app).expect("validated app").as_slice(),
                prompts.values().map(|prompt| (&prompt.id, &prompt.name)),
            );
        }
        // Keep the selected model independent from future provider edits.
        for app in scope.apps() {
            if app == &AppType::ClaudeDesktop {
                continue;
            }
            let model = match Self::current_model(app)? {
                Some(model) => Some(model),
                None => crate::services::project_profile::desired_project_model(
                    state,
                    &payload,
                    app,
                    &mut Vec::new(),
                )?
                .flatten(),
            };
            *payload.models.get_mut(app).expect("validated profile app") =
                Some(model.unwrap_or_default());
            *payload
                .model_settings
                .get_mut(app)
                .expect("validated profile app") = Some(capture_current_model_settings(app)?);
        }
        Ok(payload)
    }

    fn current_model(app: &AppType) -> Result<Option<String>, AppError> {
        match app {
            AppType::Claude => {
                let path = crate::config::get_claude_settings_path();
                if !path.exists() {
                    return Ok(None);
                }
                let settings: serde_json::Value = crate::config::read_json_file(&path)?;
                Ok(settings
                    .get("env")
                    .and_then(|env| env.get("ANTHROPIC_MODEL"))
                    .or_else(|| settings.get("model"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string))
            }
            AppType::Codex => {
                let path = crate::codex_config::get_codex_config_path();
                if !path.exists() {
                    return Ok(None);
                }
                let config = crate::codex_config::read_codex_config_text()?;
                let config = config
                    .parse::<toml_edit::DocumentMut>()
                    .map_err(|error| AppError::Config(format!("解析 Codex 模型失败: {error}")))?;
                Ok(config
                    .get("model")
                    .and_then(toml_edit::Item::as_str)
                    .map(str::to_string))
            }
            _ => Ok(None),
        }
    }

    /// 列出所有项目（项目实体全应用共享，current 标记按分组单独读取）
    pub fn list(state: &AppState) -> Result<Vec<Profile>, AppError> {
        state.db.get_all_profiles()
    }

    /// 创建新项目：只拍发起页所属分组的当前状态，其余分组槽位留 None
    /// （其他应用可能正处于别的项目，不能替用户拍进来）
    pub fn create(state: &AppState, name: &str, scope: ProfileScope) -> Result<Profile, AppError> {
        Self::create_with_project_path(state, name, scope, None)
    }

    /// 创建项目并可选地原子绑定目录，避免保存 Profile 后再绑定失败留下半成品。
    pub fn create_with_project_path(
        state: &AppState,
        name: &str,
        scope: ProfileScope,
        project_path: Option<String>,
    ) -> Result<Profile, AppError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput("Profile name is empty".to_string()));
        }
        let project_path = project_path
            .map(|path| normalize_project_path(&path))
            .transpose()?
            .flatten();
        if scope == ProfileScope::ClaudeDesktop && project_path.is_some() {
            return Err(AppError::InvalidInput(
                "项目 Profile 仅支持 Claude Code 和 Codex".to_string(),
            ));
        }
        let payload = Self::snapshot_current(state, scope)?;
        let now = chrono::Utc::now().timestamp();
        let profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            payload: serde_json::to_string(&payload)
                .map_err(|e| AppError::Config(format!("序列化 profile payload 失败: {e}")))?,
            sort_order: None,
            project_path,
            created_at: Some(now),
            updated_at: Some(now),
        };
        state.db.save_profile(&profile)?;
        Ok(profile)
    }

    /// Clone a profile snapshot into an independent profile record.
    pub fn duplicate(
        state: &AppState,
        source_id: &str,
        name: &str,
        project_path: Option<String>,
    ) -> Result<Profile, AppError> {
        let source = state
            .db
            .get_profile(source_id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Profile not found: {source_id}")))?;
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput("Profile name is empty".to_string()));
        }
        let project_path = project_path
            .map(|path| normalize_project_path(&path))
            .transpose()?
            .flatten();
        let now = chrono::Utc::now().timestamp();
        let duplicate = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            payload: source.payload,
            sort_order: None,
            project_path,
            created_at: Some(now),
            updated_at: Some(now),
        };
        state.db.save_profile(&duplicate)?;
        Ok(duplicate)
    }

    /// 更新项目：重命名（作用于共享实体）和/或以当前状态重拍快照
    /// （resnapshot 只覆盖 scope 分组的槽位，其余分组原样保留；
    /// 全局切换前自动保存；项目 Profile 也可通过显式更新入口重拍）
    pub fn update(
        state: &AppState,
        id: &str,
        name: Option<String>,
        resnapshot: bool,
        scope: Option<ProfileScope>,
    ) -> Result<Profile, AppError> {
        let mut profile = state
            .db
            .get_profile(id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Profile not found: {id}")))?;

        if let Some(name) = name {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(AppError::InvalidInput("Profile name is empty".to_string()));
            }
            profile.name = name;
        }
        if resnapshot {
            let scope = scope.ok_or_else(|| {
                AppError::InvalidInput("Resnapshot requires a profile scope".to_string())
            })?;
            let mut payload: ProfilePayload = serde_json::from_str(&profile.payload)
                .map_err(|e| AppError::Config(format!("解析 profile payload 失败: {e}")))?;
            payload.merge_scope_from(&Self::snapshot_current(state, scope)?, scope);
            profile.payload = serde_json::to_string(&payload)
                .map_err(|e| AppError::Config(format!("序列化 profile payload 失败: {e}")))?;
        }
        profile.updated_at = Some(chrono::Utc::now().timestamp());
        state.db.save_profile(&profile)?;
        Ok(profile)
    }

    pub fn update_config(
        state: &AppState,
        id: &str,
        scope: ProfileScope,
        config: ProfileConfig,
        name: Option<String>,
    ) -> Result<Profile, AppError> {
        let mut profile = state
            .db
            .get_profile(id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Profile not found: {id}")))?;
        let mut payload: ProfilePayload = serde_json::from_str(&profile.payload)
            .map_err(|error| AppError::Config(format!("解析 Profile payload 失败: {error}")))?;
        let app = &scope.apps()[0];
        if let Some(native) = config.native_config.as_deref() {
            *payload.native_config.get_mut(app).expect("validated app") =
                Some(native_config::normalize(app, native, true)?);
        }
        let model = config.model.trim().to_string();
        if model.len() > 1024 || model.chars().any(char::is_control) {
            return Err(AppError::InvalidInput("模型名称格式无效".to_string()));
        }
        let mut model_settings = config.model_settings.unwrap_or_else(|| {
            payload
                .model_settings
                .get(app)
                .and_then(Option::as_ref)
                .cloned()
                .unwrap_or_default()
        });
        let model_catalog = match config.model_catalog {
            Some(catalog) if scope == ProfileScope::Codex => {
                Some(normalize_profile_model_catalog(catalog)?)
            }
            Some(_) => {
                return Err(AppError::InvalidInput(
                    "模型目录仅支持 Codex Profile".to_string(),
                ))
            }
            None => payload
                .model_catalog
                .get(app)
                .and_then(Option::as_ref)
                .cloned(),
        };
        for (key, value) in &mut model_settings {
            if !profile_model_keys(app).contains(&key.as_str()) {
                return Err(AppError::InvalidInput(format!(
                    "不支持的 Agent 模型设置: {key}"
                )));
            }
            *value = value.trim().to_string();
            if value.len() > 1024 || value.chars().any(char::is_control) {
                return Err(AppError::InvalidInput(format!("模型设置格式无效: {key}")));
            }
        }
        if let Some(max_threads) = model_settings.get("agents.max_threads") {
            if !max_threads.is_empty() && !matches!(max_threads.parse::<u16>(), Ok(1..=9999)) {
                return Err(AppError::InvalidInput(
                    "子代理并发上限必须是 1 到 9999 的整数".to_string(),
                ));
            }
        }
        for key in ["model_context_window", "model_auto_compact_token_limit"] {
            if let Some(value) = model_settings.get(key).filter(|value| !value.is_empty()) {
                if !value.parse::<i64>().is_ok_and(|value| value > 0) {
                    return Err(AppError::InvalidInput(format!("{key} 必须是正整数")));
                }
            }
        }
        if scope == ProfileScope::ClaudeDesktop
            && (profile.project_path.is_some()
                || !model.is_empty()
                || !model_settings.is_empty()
                || model_catalog.is_some()
                || !config.mcp_ids.is_empty()
                || !config.skill_ids.is_empty()
                || config.prompt_id.is_some())
        {
            return Err(AppError::InvalidInput(
                "Claude Desktop Profile 仅支持全局供应商配置".to_string(),
            ));
        }
        if let Some(provider_id) = &config.provider_id {
            if !state.db.provider_exists(provider_id, app)?
                && payload.providers.get(app).and_then(Option::as_ref) != Some(provider_id)
            {
                return Err(AppError::InvalidInput(format!(
                    "供应商已不存在: {provider_id}"
                )));
            }
        }
        let servers = state.db.get_mcp_server_rows()?;
        let skills = state.db.get_all_installed_skills()?;
        let prompts = state.db.get_prompts(app)?;
        for (ids, old_ids, known) in [
            (
                &config.mcp_ids,
                payload.mcp.get(app).and_then(Option::as_ref),
                servers.readable.keys().collect::<HashSet<_>>(),
            ),
            (
                &config.skill_ids,
                payload.skills.get(app).and_then(Option::as_ref),
                skills.keys().collect::<HashSet<_>>(),
            ),
        ] {
            for resource_id in ids {
                if !known.contains(resource_id)
                    && !old_ids.is_some_and(|old| old.contains(resource_id))
                {
                    return Err(AppError::InvalidInput(format!(
                        "资源已不存在: {resource_id}"
                    )));
                }
            }
        }
        if let Some(prompt_id) = &config.prompt_id {
            if !prompts.contains_key(prompt_id)
                && payload.prompts.get(app).and_then(Option::as_ref) != Some(prompt_id)
            {
                return Err(AppError::InvalidInput(format!(
                    "提示词已不存在: {prompt_id}"
                )));
            }
        }
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                return Err(AppError::InvalidInput("Profile name is empty".to_string()));
            }
            profile.name = name.to_string();
        }
        *payload
            .providers
            .get_mut(app)
            .expect("validated profile app") = config.provider_id;
        if scope != ProfileScope::ClaudeDesktop {
            let aliases = payload
                .project_aliases
                .get_mut(app)
                .expect("validated profile app");
            aliases.mcp.retain(|id, _| config.mcp_ids.contains(id));
            aliases.skills.retain(|id, _| config.skill_ids.contains(id));
            let names = payload
                .resource_names
                .get_mut(app)
                .expect("validated profile app");
            remember_resource_names(
                &mut names.mcp,
                &config.mcp_ids,
                servers
                    .readable
                    .values()
                    .map(|server| (&server.id, &server.name))
                    .chain(
                        servers
                            .unreadable
                            .iter()
                            .map(|server| (&server.id, &server.name)),
                    ),
            );
            remember_resource_names(
                &mut names.skills,
                &config.skill_ids,
                skills.values().map(|skill| (&skill.id, &skill.name)),
            );
            remember_resource_names(
                &mut names.prompts,
                config.prompt_id.as_slice(),
                prompts.values().map(|prompt| (&prompt.id, &prompt.name)),
            );
            *payload.models.get_mut(app).expect("validated profile app") = Some(model);
            *payload
                .model_settings
                .get_mut(app)
                .expect("validated profile app") = Some(model_settings);
            if scope == ProfileScope::Codex {
                *payload
                    .model_catalog
                    .get_mut(app)
                    .expect("validated profile app") = Some(model_catalog.unwrap_or_else(|| {
                    serde_json::json!({
                        "models": [],
                        "autoReviewModel": "",
                        "officialSpeedTiers": false
                    })
                }));
            }
            let deduplicate = |ids: Vec<String>| {
                let mut seen = HashSet::new();
                ids.into_iter()
                    .filter(|id| seen.insert(id.clone()))
                    .collect()
            };
            *payload.mcp.get_mut(app).expect("validated profile app") =
                Some(deduplicate(config.mcp_ids));
            *payload.skills.get_mut(app).expect("validated profile app") =
                Some(deduplicate(config.skill_ids));
            *payload.prompts.get_mut(app).expect("validated profile app") = Some(
                config
                    .prompt_id
                    .unwrap_or_else(|| NO_ACTIVE_PROMPT.to_string()),
            );
        }
        profile.payload = serde_json::to_string(&payload)
            .map_err(|error| AppError::Config(format!("序列化 Profile payload 失败: {error}")))?;
        profile.updated_at = Some(chrono::Utc::now().timestamp());
        state
            .db
            .update_profile_config_record(&profile, scope.as_str())?;
        Ok(profile)
    }

    /// 绑定或解绑项目目录。
    pub fn set_project_path(
        state: &AppState,
        id: &str,
        path: Option<String>,
    ) -> Result<Profile, AppError> {
        let mut profile = state
            .db
            .get_profile(id)?
            .ok_or_else(|| AppError::InvalidInput(format!("未找到 Profile: {id}")))?;
        let project_path = path
            .map(|path| normalize_project_path(&path))
            .transpose()?
            .flatten();
        profile.project_path = project_path;
        profile.updated_at = Some(chrono::Utc::now().timestamp());
        state.db.save_profile(&profile)?;
        if profile.project_path.is_some() {
            for scope in ProfileScope::ALL {
                if state.db.get_current_profile_id(scope.as_str())?.as_deref() == Some(id) {
                    state.db.set_current_profile_id(scope.as_str(), None)?;
                }
            }
        }
        Ok(profile)
    }

    /// 删除项目；若删除的是某分组当前激活项目，一并清除该分组的激活标记
    pub fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
        state.db.delete_profile(id)?;
        for scope in ProfileScope::ALL {
            if state.db.get_current_profile_id(scope.as_str())?.as_deref() == Some(id) {
                state.db.set_current_profile_id(scope.as_str(), None)?;
            }
        }
        Ok(())
    }

    /// 应用项目快照（best-effort，返回 warnings）
    ///
    /// 只作用于发起页所属分组内的应用，不碰其他分组的配置与 current 标记。
    /// 该分组从未拍过快照时不改动配置、也不标记完整应用，返回提示。
    ///
    /// Explicit application never rewrites a previously saved Profile. Refreshing
    /// a snapshot remains a separate user action.
    ///
    /// 应用指定项目的快照到当前分组内的所有应用。
    ///
    /// 供应商按应用当前的模式切换：直连模式改直连指针，代理模式改代理路由；不再为了
    /// 避开代理先退出代理模式。
    pub fn apply(
        state: &AppState,
        profile_id: &str,
        scope: ProfileScope,
    ) -> Result<Vec<String>, AppError> {
        let mut warnings = Vec::new();

        let profile = state
            .db
            .get_profile(profile_id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Profile not found: {profile_id}")))?;
        if profile.project_path.is_some() {
            return Err(AppError::InvalidInput(
                "项目 Profile 不能应用到全局配置，请从项目 Profile 管理中部署".to_string(),
            ));
        }

        let payload: ProfilePayload = serde_json::from_str(&profile.payload)
            .map_err(|e| AppError::Config(format!("解析 profile payload 失败: {e}")))?;
        if scope == ProfileScope::Codex
            && crate::mode::current::mode_state(&AppType::Codex).is_proxy()
            && crate::mode::state::stack(&crate::live::engine::DeviceStore::for_device(), "codex")?
                .enabled
        {
            let has_catalog = payload.model_catalog.codex.as_ref().is_some_and(|catalog| {
                catalog
                    .get("models")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|models| !models.is_empty())
                    || catalog
                        .get("autoReviewModel")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|model| !model.is_empty())
                    || catalog
                        .get("officialSpeedTiers")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
            });
            let has_settings = payload
                .model_settings
                .codex
                .as_ref()
                .is_some_and(|settings| settings.values().any(|value| !value.is_empty()));
            if has_catalog || has_settings {
                return Err(AppError::InvalidInput("此 Profile 含独立模型目录或模型参数，与当前聚合配置冲突；请在聚合设置中配置模型，或退出聚合后再应用。当前配置未修改".into()));
            }
        }

        if !payload.scope_captured(scope) {
            warnings.push(format!(
                "no {} configuration captured in this profile; no configuration changes were applied",
                scope.as_str()
            ));
        }

        for app in scope.apps().iter() {
            let app_str = app.as_str();
            let names = payload
                .resource_names
                .get(app)
                .expect("validated profile app");

            // 1. 供应商
            if let Some(Some(target_pid)) = payload.providers.get(app) {
                if !state.db.provider_exists(target_pid, app)? {
                    warnings.push(format!(
                        "[{app_str}] provider '{target_pid}' no longer exists, skipped"
                    ));
                } else {
                    // Explicit application also refreshes an edited connection for the same provider.
                    match ProviderService::switch(state, app.clone(), target_pid) {
                        Ok(result) => warnings.extend(result.warnings),
                        Err(e) => warnings.push(format!(
                            "[{app_str}] switch provider '{target_pid}' failed: {e}"
                        )),
                    }
                }
            }

            if payload.models.get(app).is_some_and(Option::is_some)
                || payload.model_settings.get(app).is_some_and(Option::is_some)
            {
                if let Err(error) = Self::apply_saved_model(state, &payload, app, &mut warnings) {
                    warnings.push(format!("[{app_str}] apply model failed: {error}"));
                }
            }
            if app == &AppType::Codex && payload.model_catalog.codex.is_some() {
                if let Err(error) = Self::apply_saved_model_catalog(state, &payload, &mut warnings)
                {
                    warnings.push(format!("[{app_str}] apply model catalog failed: {error}"));
                }
            }

            if let Err(error) = native_config::apply_global(state, &payload, app) {
                warnings.push(format!(
                    "[{app_str}] apply native configuration failed: {error}"
                ));
            }

            // 2. MCP diff（最小 toggle：仅动目标态≠当前态的条目；None = 该侧未拍过，不动）
            if let Some(Some(target_ids)) = payload.mcp.get(app) {
                let servers = state.db.get_mcp_server_rows()?;
                let current: Vec<(String, bool)> = servers
                    .readable
                    .values()
                    .map(|s| (s.id.clone(), s.apps.is_enabled_for(app)))
                    .collect();
                let (toggles, dangling) = plan_toggles(&current, target_ids);
                for id in dangling {
                    let unreadable = servers.unreadable.iter().find(|entry| entry.id == id);
                    let label = resource_label(
                        &id,
                        unreadable
                            .map(|entry| &entry.name)
                            .or_else(|| names.mcp.get(&id)),
                    );
                    let reason = if unreadable.is_some() {
                        "is unreadable"
                    } else {
                        "no longer exists"
                    };
                    warnings.push(format!("[{app_str}] MCP '{label}' {reason}, skipped"));
                }
                for (id, enabled) in toggles {
                    if let Err(e) = McpService::toggle_app(state, &id, app.clone(), enabled) {
                        warnings.push(format!(
                            "[{app_str}] toggle MCP '{id}' -> {enabled} failed: {e}"
                        ));
                    }
                }
            }

            // 3. Skills diff（SkillService 返回 anyhow::Result，收进 warning）
            if let Some(Some(target_ids)) = payload.skills.get(app) {
                let skills = state.db.get_all_installed_skills()?;
                let current: Vec<(String, bool)> = skills
                    .values()
                    .map(|s| (s.id.clone(), s.apps.is_enabled_for(app)))
                    .collect();
                let (toggles, dangling) = plan_toggles(&current, target_ids);
                for id in dangling {
                    let label = resource_label(&id, names.skills.get(&id));
                    warnings.push(format!(
                        "[{app_str}] skill '{label}' no longer exists, skipped"
                    ));
                }
                for (id, enabled) in toggles {
                    if let Err(e) = SkillService::toggle_app(&state.db, &id, app, enabled) {
                        warnings.push(format!(
                            "[{app_str}] toggle skill '{id}' -> {enabled} failed: {e}"
                        ));
                    }
                }
            }

            // 4. Prompt（None = 不动；NO_ACTIVE_PROMPT = 停用当前启用项；
            //    已激活则幂等跳过，避免无谓的文件写与备份）
            if let Some(Some(target_prompt)) = payload.prompts.get(app) {
                let prompts = state.db.get_prompts(app)?;
                match prompts.get(target_prompt) {
                    None if target_prompt == NO_ACTIVE_PROMPT => {
                        if prompts.values().any(|p| p.enabled) {
                            Self::disable_active_prompts(state, app, &mut warnings);
                        }
                    }
                    None => warnings.push(format!(
                        "[{app_str}] prompt '{}' no longer exists, skipped",
                        resource_label(target_prompt, names.prompts.get(target_prompt))
                    )),
                    Some(p) if p.enabled => {}
                    Some(_) => {
                        if let Err(e) =
                            PromptService::enable_prompt(state, app.clone(), target_prompt)
                        {
                            warnings.push(format!(
                                "[{app_str}] enable prompt '{target_prompt}' failed: {e}"
                            ));
                        }
                    }
                }
            }
        }

        state
            .db
            .set_current_profile_id(scope.as_str(), warnings.is_empty().then_some(profile_id))?;

        Ok(warnings)
    }

    fn apply_saved_model(
        state: &AppState,
        payload: &ProfilePayload,
        app: &AppType,
        warnings: &mut Vec<String>,
    ) -> Result<(), AppError> {
        if app == &AppType::ClaudeDesktop {
            return Ok(());
        }
        if payload.models.get(app).is_some_and(Option::is_some) {
            let mut desired_payload = payload.clone();
            // An empty global model inherits the selected provider's default.
            let inherits_default =
                desired_payload.models.get(app).and_then(Option::as_deref) == Some("");
            if inherits_default {
                *desired_payload.models.get_mut(app).expect("validated app") = None;
            }
            let mut model_warnings = Vec::new();
            let resolved = crate::services::project_profile::desired_project_model(
                state,
                &desired_payload,
                app,
                &mut model_warnings,
            )?;
            if !(inherits_default && matches!(resolved, Some(None))) {
                warnings.extend(model_warnings);
            }
            if let Some(model) = resolved {
                match app {
                    AppType::Claude => {
                        let path = crate::config::get_claude_settings_path();
                        let mut settings = if path.exists() {
                            crate::config::read_json_file::<serde_json::Value>(&path)?
                        } else {
                            serde_json::json!({})
                        };
                        let root = settings.as_object_mut().ok_or_else(|| {
                            AppError::Config("Claude settings 必须是对象".to_string())
                        })?;
                        let env = root
                            .entry("env")
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                            .ok_or_else(|| {
                                AppError::Config("Claude settings.env 必须是对象".to_string())
                            })?;
                        match model {
                            Some(model) => {
                                env.insert("ANTHROPIC_MODEL".to_string(), model.into());
                            }
                            None => {
                                env.remove("ANTHROPIC_MODEL");
                                root.remove("model");
                            }
                        }
                        crate::config::write_json_file_private(&path, &settings)?;
                    }
                    AppType::Codex => {
                        let path = crate::codex_config::get_codex_config_path();
                        let mut config = if path.exists() {
                            crate::codex_config::read_codex_config_text()?
                                .parse::<toml_edit::DocumentMut>()
                                .map_err(|error| {
                                    AppError::Config(format!("解析 Codex config 失败: {error}"))
                                })?
                        } else {
                            toml_edit::DocumentMut::new()
                        };
                        match model {
                            Some(model) => {
                                config["model"] = toml_edit::value(model);
                            }
                            None => {
                                config.remove("model");
                            }
                        }
                        crate::config::atomic_write_private(&path, config.to_string().as_bytes())?;
                    }
                    _ => {}
                }
            }
        }
        Self::apply_saved_model_settings(state, payload, app, warnings)
    }

    fn apply_saved_model_settings(
        state: &AppState,
        payload: &ProfilePayload,
        app: &AppType,
        warnings: &mut Vec<String>,
    ) -> Result<(), AppError> {
        let Some(overrides) = payload.model_settings.get(app).and_then(Option::as_ref) else {
            return Ok(());
        };
        if app == &AppType::Codex
            && crate::mode::current::mode_state(app).is_proxy()
            && crate::mode::state::stack(&crate::live::engine::DeviceStore::for_device(), "codex")?
                .enabled
        {
            return Ok(());
        }
        let mut desired =
            if let Some(provider_id) = payload.providers.get(app).and_then(Option::as_deref) {
                let Some(settings) = provider_model_settings(state, app, provider_id)? else {
                    warnings.push(format!(
                        "所选供应商的模型设置已删除或不可读，保留当前全局模型设置 ({provider_id})"
                    ));
                    return Ok(());
                };
                settings
            } else {
                BTreeMap::new()
            };
        desired.extend(overrides.clone());
        match app {
            AppType::Claude => {
                let path = crate::config::get_claude_settings_path();
                let mut settings = if path.exists() {
                    crate::config::read_json_file::<serde_json::Value>(&path)?
                } else {
                    serde_json::json!({})
                };
                let root = settings
                    .as_object_mut()
                    .ok_or_else(|| AppError::Config("Claude settings 必须是对象".to_string()))?;
                let env = root
                    .entry("env")
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
                    .ok_or_else(|| {
                        AppError::Config("Claude settings.env 必须是对象".to_string())
                    })?;
                for key in CLAUDE_PROFILE_MODEL_KEYS {
                    match desired.get(*key).filter(|value| !value.is_empty()) {
                        Some(value) => {
                            env.insert((*key).to_string(), value.clone().into());
                        }
                        None => {
                            env.remove(*key);
                        }
                    }
                }
                crate::config::write_json_file_private(&path, &settings)?;
            }
            AppType::Codex => {
                let path = crate::codex_config::get_codex_config_path();
                let mut config = if path.exists() {
                    crate::codex_config::read_codex_config_text()?
                        .parse::<toml_edit::DocumentMut>()
                        .map_err(|error| {
                            AppError::Config(format!("解析 Codex config 失败: {error}"))
                        })?
                } else {
                    toml_edit::DocumentMut::new()
                };
                for key in CODEX_PROFILE_MODEL_KEYS {
                    set_codex_model_setting(
                        &mut config,
                        key,
                        desired.get(*key).filter(|value| !value.is_empty()),
                    )?;
                }
                crate::config::atomic_write_private(&path, config.to_string().as_bytes())?;
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_saved_model_catalog(
        state: &AppState,
        payload: &ProfilePayload,
        warnings: &mut Vec<String>,
    ) -> Result<(), AppError> {
        let Some(catalog_settings) = payload.model_catalog.codex.as_ref() else {
            return Ok(());
        };
        let mode = crate::mode::current::mode_state(&AppType::Codex);
        let stack =
            crate::mode::state::stack(&crate::live::engine::DeviceStore::for_device(), "codex")?;
        if mode.is_proxy() && stack.enabled {
            return Ok(());
        }
        let provider_id = match payload.providers.codex.clone() {
            Some(id) => Some(id),
            None => crate::mode::current::provider_for(
                &state.db,
                &AppType::Codex,
                crate::mode::current::Purpose::InUse,
            )?,
        };
        let provider = match provider_id {
            Some(ref id) => match state.db.read_provider(&AppType::Codex, id)? {
                Some(Ok(provider)) => Some(provider),
                _ => {
                    warnings.push(format!(
                        "Codex 供应商已删除或不可读，保留当前模型目录 ({id})"
                    ));
                    return Ok(());
                }
            },
            None => None,
        };
        let mut settings = provider
            .as_ref()
            .map(|provider| provider.settings_config.clone())
            .unwrap_or_else(|| serde_json::json!({}));
        settings["modelCatalog"] = catalog_settings.clone();
        let config_path = crate::codex_config::get_codex_config_path();
        let mut config = if config_path.exists() {
            crate::codex_config::read_codex_config_text()?
                .parse::<toml_edit::DocumentMut>()
                .map_err(|error| AppError::Config(format!("解析 Codex config 失败: {error}")))?
        } else {
            toml_edit::DocumentMut::new()
        };
        if crate::live::project::codex::foreign_catalog(&config).is_some() {
            warnings.push("Codex config 指向用户维护的外部模型目录，Profile 未覆盖它".to_string());
            return Ok(());
        }
        if let Some(model) = payload.models.codex.as_ref() {
            if model.is_empty() {
                config.remove("model");
            } else {
                config["model"] = toml_edit::value(model.as_str());
            }
        }
        let profile = provider
            .as_ref()
            .map(crate::proxy::providers::resolve_codex_catalog_tool_profile)
            .unwrap_or(crate::codex_config::CodexCatalogToolProfile::NativeResponses);
        let catalog =
            crate::codex_config::plan_codex_model_catalog(&settings, &config.to_string(), profile)?
                .catalog;
        let catalog_path = crate::codex_config::get_codex_model_catalog_path();
        let original = read_optional_file(&catalog_path)?;
        match catalog {
            Some(catalog) => {
                let bytes = serde_json::to_vec_pretty(&catalog).map_err(|error| {
                    AppError::Config(format!("序列化 Codex 模型目录失败: {error}"))
                })?;
                crate::config::atomic_write_private(&catalog_path, &bytes)?;
                let config_dir = config_path
                    .parent()
                    .ok_or_else(|| AppError::InvalidInput("Codex config 路径无效".into()))?;
                let pointer = crate::live::project::codex::catalog_pointer_for(config_dir, &config);
                config["model_catalog_json"] = toml_edit::value(pointer);
                if let Err(error) =
                    crate::config::atomic_write_private(&config_path, config.to_string().as_bytes())
                {
                    if let Err(restore_error) =
                        restore_optional_file(&catalog_path, original.as_deref())
                    {
                        return Err(AppError::Config(format!(
                            "写入 Codex config 失败: {error}; 恢复模型目录失败: {restore_error}"
                        )));
                    }
                    return Err(error);
                }
            }
            None => {
                if !crate::live::project::codex::live_catalog_is_ours(&config.to_string()) {
                    return Ok(());
                }
                config.remove("model_catalog_json");
                crate::config::atomic_write_private(&config_path, config.to_string().as_bytes())?;
                if let Err(error) = std::fs::remove_file(&catalog_path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        warnings.push(format!("旧 Codex 模型目录无法清理: {error}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// 停用应用当前启用的提示词（项目快照里该应用没有启用项时）。
    ///
    /// 先经 `PromptService::get_prompts` 把 live 文件里的外部编辑回填进启用项，
    /// 再走与界面「停用」相同的 `upsert_prompt(enabled=false)`：停掉最后一条启用项时清空文件。
    fn disable_active_prompts(state: &AppState, app: &AppType, warnings: &mut Vec<String>) {
        let app_str = app.as_str();
        let prompts = match PromptService::get_prompts(state, app.clone()) {
            Ok(prompts) => prompts,
            Err(e) => {
                warnings.push(format!("[{app_str}] read prompts failed: {e}"));
                return;
            }
        };
        for (id, prompt) in prompts.into_iter().filter(|(_, p)| p.enabled) {
            let disabled = Prompt {
                enabled: false,
                ..prompt
            };
            if let Err(e) = PromptService::upsert_prompt(state, app.clone(), &id, disabled) {
                warnings.push(format!("[{app_str}] disable prompt '{id}' failed: {e}"));
            }
        }
    }
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn restore_optional_file(path: &Path, original: Option<&[u8]>) -> Result<(), AppError> {
    match original {
        Some(bytes) => crate::config::atomic_write_private(path, bytes),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::io(path, error)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn manual_profile_edits_are_scoped_and_not_overwritten_by_global_autosave() {
        let _home = crate::test_support::TempUserHome::new();
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().unwrap(),
        ));
        let path = crate::config::get_claude_settings_path();
        crate::config::write_json_file(
            &path,
            &serde_json::json!({
                "env": {"ANTHROPIC_MODEL": "global-model", "KEEP": "value"},
                "permissions": {"allow": ["Read"]}
            }),
        )
        .unwrap();
        let initial = ProfileService::create(&state, "Shared", ProfileScope::Claude).unwrap();
        let mut payload: ProfilePayload = serde_json::from_str(&initial.payload).unwrap();
        assert_eq!(payload.models.claude.as_deref(), Some("global-model"));
        payload.models.codex = Some("codex-unchanged".to_string());
        payload.mcp.codex = Some(vec!["old-codex-reference".to_string()]);
        state
            .db
            .save_profile(&Profile {
                payload: serde_json::to_string(&payload).unwrap(),
                ..initial.clone()
            })
            .unwrap();
        state
            .db
            .set_current_profile_id("claude", Some(&initial.id))
            .unwrap();
        state
            .db
            .set_current_profile_id("codex", Some(&initial.id))
            .unwrap();
        let config = ProfileConfig {
            native_config: None,
            provider_id: None,
            model: "project-independent-model".to_string(),
            model_settings: Some(BTreeMap::from([(
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                "profile-sonnet".to_string(),
            )])),
            model_catalog: None,
            mcp_ids: vec![],
            skill_ids: vec![],
            prompt_id: None,
        };
        let edited = ProfileService::update_config(
            &state,
            &initial.id,
            ProfileScope::Claude,
            config.clone(),
            Some("Edited".to_string()),
        )
        .unwrap();
        let saved: ProfilePayload = serde_json::from_str(&edited.payload).unwrap();
        assert_eq!(
            saved.models.claude.as_deref(),
            Some("project-independent-model")
        );
        assert_eq!(saved.models.codex, payload.models.codex);
        assert_eq!(
            saved.model_settings.claude.as_ref().unwrap()["ANTHROPIC_DEFAULT_SONNET_MODEL"],
            "profile-sonnet"
        );
        assert_eq!(saved.model_settings.codex, payload.model_settings.codex);
        let mut invalid = config.clone();
        invalid
            .model_settings
            .get_or_insert_with(BTreeMap::new)
            .insert(
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "must-not-save".to_string(),
            );
        assert!(matches!(
            ProfileService::update_config(&state, &initial.id, ProfileScope::Claude, invalid, None),
            Err(AppError::InvalidInput(_))
        ));
        assert_eq!(
            state.db.get_profile(&initial.id).unwrap().unwrap().payload,
            edited.payload,
            "rejected non-model fields must not alter the Profile"
        );
        assert_eq!(saved.mcp.codex, payload.mcp.codex);
        assert_eq!(state.db.get_current_profile_id("claude").unwrap(), None);
        assert_eq!(
            state.db.get_current_profile_id("codex").unwrap().as_deref(),
            Some(initial.id.as_str())
        );
        let live: serde_json::Value = crate::config::read_json_file(&path).unwrap();
        assert_eq!(
            live["env"]["ANTHROPIC_MODEL"], "global-model",
            "saving only changes profile data"
        );
        let other = ProfileService::create(&state, "Other", ProfileScope::Claude).unwrap();
        ProfileService::apply(&state, &other.id, ProfileScope::Claude).unwrap();
        assert_eq!(
            state.db.get_profile(&initial.id).unwrap().unwrap().payload,
            edited.payload
        );
        ProfileService::apply(&state, &initial.id, ProfileScope::Claude).unwrap();
        let live: serde_json::Value = crate::config::read_json_file(&path).unwrap();
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "project-independent-model");
        assert_eq!(
            live["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
            "profile-sonnet"
        );
        assert_eq!(live["env"]["KEEP"], "value");
        assert_eq!(live["permissions"]["allow"], serde_json::json!(["Read"]));
        let mut invalid = config;
        invalid.mcp_ids = vec!["new-missing-resource".to_string()];
        let before = state.db.get_profile(&initial.id).unwrap().unwrap().payload;
        assert!(ProfileService::update_config(
            &state,
            &initial.id,
            ProfileScope::Claude,
            invalid,
            None
        )
        .is_err());
        assert_eq!(
            state.db.get_profile(&initial.id).unwrap().unwrap().payload,
            before
        );
        assert_eq!(
            state
                .db
                .get_current_profile_id("claude")
                .unwrap()
                .as_deref(),
            Some(initial.id.as_str())
        );
    }

    #[test]
    fn test_profiles_normalize_project_path() {
        assert_eq!(normalize_project_path(" \t\n ").unwrap(), None);
        assert!(matches!(
            normalize_project_path("relative/project"),
            Err(AppError::InvalidInput(_))
        ));

        #[cfg(windows)]
        let (path, root) = (r"C:\Projects\Mixed Case\..\项目", r"C:\");
        #[cfg(not(windows))]
        let (path, root) = ("/Projects/Mixed Case/../项目", "/");

        assert_eq!(
            normalize_project_path(path).unwrap(),
            Some(path.to_string())
        );
        assert_eq!(
            normalize_project_path(&format!("{path}/")).unwrap(),
            Some(path.to_string())
        );
        assert_eq!(
            normalize_project_path(root).unwrap(),
            Some(root.to_string())
        );
        assert_eq!(
            normalize_project_path(&format!("{root}./")).unwrap(),
            Some(root.to_string())
        );

        #[cfg(windows)]
        assert_eq!(
            normalize_project_path(r"C:/Projects/Team/").unwrap(),
            Some(r"C:\Projects\Team".to_string())
        );
        #[cfg(not(windows))]
        {
            assert_eq!(
                normalize_project_path("/Projects/Team ").unwrap(),
                Some("/Projects/Team ".to_string())
            );
            assert_eq!(
                normalize_project_path("/Projects/Team\\").unwrap(),
                Some("/Projects/Team\\".to_string())
            );
        }

        #[cfg(windows)]
        {
            for invalid in [r"C:relative", r"\Projects", "/"] {
                assert!(matches!(
                    normalize_project_path(invalid),
                    Err(AppError::InvalidInput(_))
                ));
            }
            assert_eq!(
                normalize_project_path(r"\\server\share").unwrap(),
                normalize_project_path(r"\\server\share\").unwrap()
            );
            assert_eq!(
                normalize_project_path(r"\\?\C:\").unwrap(),
                Some(r"\\?\C:\".to_string())
            );
        }
    }

    #[test]
    fn test_profiles_set_project_path() -> Result<(), AppError> {
        let test_home = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(crate::database::Database::memory()?);
        let state = AppState {
            db: db.clone(),
            proxy_service: crate::services::ProxyService::new(db),
            usage_cache: std::sync::Arc::new(crate::services::UsageCache::new()),
            codex_oauth_manager: std::sync::Arc::new(
                crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
                    test_home.path().to_path_buf(),
                ),
            ),
        };
        let original = Profile {
            id: "a".to_string(),
            name: "项目 A".to_string(),
            payload: "{}".to_string(),
            sort_order: Some(3),
            project_path: None,
            created_at: Some(1),
            updated_at: Some(1),
        };
        state.db.save_profile(&original)?;
        state.db.save_profile(&Profile {
            id: "b".to_string(),
            name: "项目 B".to_string(),
            ..original.clone()
        })?;
        #[cfg(windows)]
        let path = r"C:\Projects\Équipe";
        #[cfg(not(windows))]
        let path = "/Projects/Équipe";

        let bound = ProfileService::set_project_path(&state, "a", Some(format!("{path}/")))?;
        assert_eq!(bound.project_path.as_deref(), Some(path));
        assert!(bound.updated_at > original.updated_at);
        assert_eq!(bound.name, original.name);
        assert_eq!(bound.payload, original.payload);
        assert_eq!(bound.sort_order, original.sort_order);
        assert_eq!(bound.created_at, original.created_at);
        assert_eq!(
            state.db.get_profile("a")?.unwrap().project_path,
            bound.project_path
        );
        // Multiple profiles can refer to the same project directory.
        ProfileService::set_project_path(&state, "a", Some(path.to_string()))?;
        let second_bound = ProfileService::set_project_path(&state, "b", Some(path.to_string()))?;
        assert_eq!(second_bound.project_path.as_deref(), Some(path));

        assert!(matches!(
            ProfileService::set_project_path(&state, "a", Some("relative".to_string())),
            Err(AppError::InvalidInput(_))
        ));
        assert_eq!(
            state.db.get_profile("a")?.unwrap().project_path,
            bound.project_path
        );
        assert!(matches!(
            ProfileService::set_project_path(&state, "missing", None),
            Err(AppError::InvalidInput(_))
        ));
        assert_eq!(
            ProfileService::set_project_path(&state, "a", None)?.project_path,
            None
        );
        ProfileService::set_project_path(&state, "a", Some(path.to_string()))?;
        assert_eq!(
            ProfileService::set_project_path(&state, "a", Some(" \t ".to_string()))?.project_path,
            None
        );

        #[cfg(windows)]
        let create_path = r"C:\Projects\New";
        #[cfg(not(windows))]
        let create_path = "/Projects/New";
        let created = ProfileService::create_with_project_path(
            &state,
            "新建并绑定",
            ProfileScope::Claude,
            Some(create_path.to_string()),
        )?;
        assert_eq!(created.project_path.as_deref(), Some(create_path));
        let same_path_profile = ProfileService::create_with_project_path(
            &state,
            "重复目录",
            ProfileScope::Claude,
            Some(create_path.to_string()),
        )?;
        assert_eq!(same_path_profile.project_path, created.project_path);
        Ok(())
    }

    #[test]
    fn test_duplicate_profile_copies_payload_independently() -> Result<(), AppError> {
        let test_home = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(crate::database::Database::memory()?);
        let state = AppState {
            db: db.clone(),
            proxy_service: crate::services::ProxyService::new(db),
            usage_cache: std::sync::Arc::new(crate::services::UsageCache::new()),
            codex_oauth_manager: std::sync::Arc::new(
                crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
                    test_home.path().to_path_buf(),
                ),
            ),
        };
        #[cfg(windows)]
        let path = r"C:\Projects\Shared";
        #[cfg(not(windows))]
        let path = "/Projects/Shared";
        let original = Profile {
            id: "source".to_string(),
            name: "Source".to_string(),
            payload: r#"{"providers":{"claude":"provider-a"},"mcp":{"claude":["mcp-a"]}}"#
                .to_string(),
            sort_order: Some(2),
            project_path: Some(path.to_string()),
            created_at: Some(1),
            updated_at: Some(2),
        };
        state.db.save_profile(&original)?;

        let duplicate = ProfileService::duplicate(
            &state,
            &original.id,
            "  Source copy  ",
            Some(format!("{path}/")),
        )?;
        assert_ne!(duplicate.id, original.id);
        assert_eq!(duplicate.name, "Source copy");
        assert_eq!(duplicate.payload, original.payload);
        assert_eq!(duplicate.project_path.as_deref(), Some(path));
        assert_ne!(duplicate.created_at, original.created_at);
        assert_ne!(duplicate.updated_at, original.updated_at);

        let mut edited_duplicate = state.db.get_profile(&duplicate.id)?.unwrap();
        edited_duplicate.payload = r#"{"providers":{"claude":"provider-b"}}"#.to_string();
        state.db.save_profile(&edited_duplicate)?;
        assert_eq!(
            state.db.get_profile(&original.id)?.unwrap().payload,
            original.payload
        );
        assert_eq!(
            state.db.get_profile(&duplicate.id)?.unwrap().payload,
            edited_duplicate.payload
        );

        let unbound_duplicate =
            ProfileService::duplicate(&state, &original.id, "Unbound copy", None)?;
        assert_eq!(unbound_duplicate.project_path, None);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn project_profiles_stay_out_of_global_apply_and_autosave() -> Result<(), AppError> {
        let home = crate::test_support::TestHome::new();
        let state = AppState::new(std::sync::Arc::new(crate::database::Database::memory()?));
        let global = Profile {
            id: "global".to_string(),
            name: "Global".to_string(),
            payload: "{}".to_string(),
            sort_order: None,
            project_path: None,
            created_at: None,
            updated_at: None,
        };
        let project = Profile {
            id: "project".to_string(),
            name: "Project".to_string(),
            ..global.clone()
        };
        state.db.save_profile(&global)?;
        state.db.save_profile(&project)?;
        for scope in ProfileScope::ALL {
            state
                .db
                .set_current_profile_id(scope.as_str(), Some(&project.id))?;
        }
        ProfileService::set_project_path(
            &state,
            &project.id,
            Some(home.path().to_string_lossy().into_owned()),
        )?;
        for scope in ProfileScope::ALL {
            assert_eq!(state.db.get_current_profile_id(scope.as_str())?, None);
        }

        // An older database can still point its global current ID at a project.
        state
            .db
            .set_current_profile_id("claude", Some(&project.id))?;
        assert!(ProfileService::apply(&state, &project.id, ProfileScope::Claude).is_err());
        assert_eq!(
            state.db.get_current_profile_id("claude")?.as_deref(),
            Some(project.id.as_str())
        );
        ProfileService::apply(&state, &global.id, ProfileScope::Claude)?;
        assert_eq!(
            state.db.get_profile(&project.id)?.unwrap().payload,
            project.payload
        );
        assert_eq!(
            state.db.get_current_profile_id("claude")?.as_deref(),
            None,
            "an uncaptured global scope is not marked fully applied"
        );

        assert!(ProfileService::create_with_project_path(
            &state,
            "Desktop project",
            ProfileScope::ClaudeDesktop,
            Some(home.path().to_string_lossy().into_owned()),
        )
        .is_err());
        assert_eq!(state.db.get_all_profiles()?.len(), 2);
        Ok(())
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_payload_serde_roundtrip() {
        let payload = ProfilePayload {
            native_config: PerApp::default(),
            models: PerApp::default(),
            model_settings: PerApp {
                claude: Some(BTreeMap::from([(
                    "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
                    "sonnet-profile".into(),
                )])),
                claude_desktop: None,
                codex: None,
            },
            model_catalog: PerApp {
                claude: None,
                claude_desktop: None,
                codex: Some(serde_json::json!({
                    "models": [{"model":"gpt-6-sol","reasoningLevels":["low","high"]}],
                    "autoReviewModel":"gpt-6-sol",
                    "officialSpeedTiers":false
                })),
            },
            providers: PerApp {
                claude: Some("p1".into()),
                claude_desktop: Some("d1".into()),
                codex: None,
            },
            mcp: PerApp {
                claude: Some(ids(&["m1", "m2"])),
                claude_desktop: Some(vec![]),
                codex: None,
            },
            skills: PerApp {
                claude: Some(vec![]),
                claude_desktop: Some(vec![]),
                codex: Some(ids(&["s1"])),
            },
            prompts: PerApp {
                claude: None,
                claude_desktop: None,
                codex: Some("pr1".into()),
            },
            resource_names: PerApp::default(),
            project_aliases: PerApp::default(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        // per-app key 必须与 AppType 的 serde 形式一致（claude-desktop 是连字符）
        assert!(json.contains("\"claude\""));
        assert!(json.contains("\"claude-desktop\""));
        assert!(json.contains("\"codex\""));
        assert!(json.contains("modelSettings"));
        assert!(json.contains("modelCatalog"));
        assert!(json.contains("resourceNames"));
        let back: ProfilePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn test_payload_tolerates_missing_fields() {
        // 前向兼容：旧版/部分字段缺失时应落到 None（"该侧未拍过"）而不是报错，
        // 应用时对缺失槽位不做任何改动
        let back: ProfilePayload =
            serde_json::from_str(r#"{"providers":{"claude":"p1"},"mcp":{"claude":["m1"]}}"#)
                .unwrap();
        assert_eq!(back.providers.claude, Some("p1".to_string()));
        assert_eq!(back.providers.claude_desktop, None);
        assert_eq!(back.providers.codex, None);
        assert_eq!(back.mcp.claude, Some(ids(&["m1"])));
        assert_eq!(back.mcp.claude_desktop, None);
        assert_eq!(back.mcp.codex, None, "missing slot means untouched");
        assert_eq!(back.model_settings.claude, None);
        assert_eq!(back.model_settings.codex, None);
        assert_eq!(back.prompts.codex, None);
        assert_eq!(back.resource_names, PerApp::default());

        let empty: ProfilePayload = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, ProfilePayload::default());
    }

    #[test]
    fn test_merge_scope_from_only_touches_scope_slots() {
        // 项目 A：两侧都已拍过快照
        let mut payload = ProfilePayload {
            providers: PerApp {
                claude: Some("p1".into()),
                claude_desktop: Some("d1".into()),
                codex: Some("c1".into()),
            },
            mcp: PerApp {
                claude: Some(ids(&["m1"])),
                claude_desktop: Some(vec![]),
                codex: Some(ids(&["m9"])),
            },
            model_settings: PerApp {
                claude: Some(BTreeMap::from([("model".into(), "a".into())])),
                claude_desktop: None,
                codex: Some(BTreeMap::from([("model".into(), "b".into())])),
            },
            ..Default::default()
        };
        // 在 Claude 页"以当前状态更新"：只覆盖 claude 组槽位
        payload
            .resource_names
            .codex
            .skills
            .insert("s1".into(), "Codex skill".into());
        let mut fresh = ProfilePayload {
            providers: PerApp {
                claude: Some("p2".into()),
                claude_desktop: None,
                codex: Some("SHOULD-NOT-LEAK".into()),
            },
            mcp: PerApp {
                claude: Some(ids(&["m2"])),
                claude_desktop: Some(vec![]),
                codex: None,
            },
            model_settings: PerApp {
                claude: Some(BTreeMap::from([("model".into(), "c".into())])),
                claude_desktop: None,
                codex: Some(BTreeMap::from([("model".into(), "LEAK".into())])),
            },
            ..Default::default()
        };
        fresh
            .resource_names
            .claude
            .mcp
            .insert("m2".into(), "Claude MCP".into());
        fresh
            .resource_names
            .codex
            .skills
            .insert("s1".into(), "SHOULD-NOT-LEAK".into());
        payload.merge_scope_from(&fresh, ProfileScope::Claude);

        assert_eq!(payload.providers.claude, Some("p2".to_string()));
        assert_eq!(
            payload.providers.claude_desktop,
            Some("d1".to_string()),
            "claude-desktop slot is in its own scope, untouched by claude merge"
        );
        assert_eq!(payload.mcp.claude, Some(ids(&["m2"])));
        assert_eq!(
            payload.model_settings.claude.as_ref().unwrap()["model"],
            "c"
        );
        // codex 侧完好：既没被覆盖也没被 fresh 的值污染
        assert_eq!(payload.providers.codex, Some("c1".to_string()));
        assert_eq!(payload.mcp.codex, Some(ids(&["m9"])));
        assert_eq!(payload.model_settings.codex.as_ref().unwrap()["model"], "b");
        assert_eq!(payload.resource_names.claude.mcp["m2"], "Claude MCP");
        assert_eq!(payload.resource_names.codex.skills["s1"], "Codex skill");
    }

    #[test]
    fn test_scope_captured_detects_per_scope_snapshot() {
        let mut payload = ProfilePayload::default();
        assert!(!payload.scope_captured(ProfileScope::Claude));
        assert!(!payload.scope_captured(ProfileScope::ClaudeDesktop));
        assert!(!payload.scope_captured(ProfileScope::Codex));
        payload
            .resource_names
            .claude
            .mcp
            .insert("m1".into(), "Display only".into());
        assert!(!payload.scope_captured(ProfileScope::Claude));

        // 只拍过 claude 组（哪怕拍到的是空集）
        payload.mcp.claude = Some(vec![]);
        assert!(payload.scope_captured(ProfileScope::Claude));
        assert!(!payload.scope_captured(ProfileScope::ClaudeDesktop));
        assert!(!payload.scope_captured(ProfileScope::Codex));

        let mut model_settings_only = ProfilePayload::default();
        model_settings_only.model_settings.claude = Some(BTreeMap::new());
        assert!(model_settings_only.scope_captured(ProfileScope::Claude));
        assert!(!model_settings_only.scope_captured(ProfileScope::Codex));

        // Desktop 槽位属于独立的 claude-desktop 组
        let mut desktop_only = ProfilePayload::default();
        desktop_only.providers.claude_desktop = Some("d1".into());
        assert!(desktop_only.scope_captured(ProfileScope::ClaudeDesktop));
        assert!(!desktop_only.scope_captured(ProfileScope::Claude));
    }

    #[test]
    fn test_per_app_get_only_supports_profile_apps() {
        let per: PerApp<Option<String>> = PerApp::default();
        assert!(per.get(&AppType::Claude).is_some());
        assert!(per.get(&AppType::ClaudeDesktop).is_some());
        assert!(per.get(&AppType::Codex).is_some());
        assert!(per.get(&AppType::Gemini).is_none());
    }

    #[test]
    fn test_scope_serde_and_parse_roundtrip() {
        for scope in ProfileScope::ALL {
            // DB 存储字符串（as_str/parse）与 JSON 序列化必须是同一形式
            assert_eq!(
                serde_json::to_string(&scope).unwrap(),
                format!("\"{}\"", scope.as_str())
            );
            assert_eq!(ProfileScope::parse(scope.as_str()).unwrap(), scope);
        }
        assert!(ProfileScope::parse("gemini").is_err());
        assert!(ProfileScope::parse("").is_err());
    }

    #[test]
    fn test_scope_app_grouping() {
        // Claude Code 与 Claude Desktop 各自独立成组；
        // 组内应用与 for_app 反向映射必须一致
        assert_eq!(ProfileScope::Claude.apps(), &[AppType::Claude]);
        assert_eq!(
            ProfileScope::ClaudeDesktop.apps(),
            &[AppType::ClaudeDesktop]
        );
        assert_eq!(ProfileScope::Codex.apps(), &[AppType::Codex]);
        for scope in ProfileScope::ALL {
            for app in scope.apps() {
                assert_eq!(ProfileScope::for_app(app), Some(scope));
            }
        }
        assert_eq!(ProfileScope::for_app(&AppType::Gemini), None);
    }

    #[test]
    fn test_plan_toggles_minimal_diff() {
        let current = vec![
            ("a".to_string(), true),  // 目标含 a：不动
            ("b".to_string(), false), // 目标含 b：开
            ("c".to_string(), true),  // 目标不含 c：关
            ("d".to_string(), false), // 目标不含 d：不动
        ];
        let (toggles, dangling) = plan_toggles(&current, &ids(&["a", "b", "ghost"]));
        assert_eq!(
            toggles,
            vec![("b".to_string(), true), ("c".to_string(), false)]
        );
        assert_eq!(dangling, ids(&["ghost"]));
    }

    #[test]
    fn test_plan_toggles_empty_target_disables_all_enabled() {
        let current = vec![("a".to_string(), true), ("b".to_string(), false)];
        let (toggles, dangling) = plan_toggles(&current, &[]);
        assert_eq!(toggles, vec![("a".to_string(), false)]);
        assert!(dangling.is_empty());
    }
}
