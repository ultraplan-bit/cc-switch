//! Claude project connections are deployed together; credentials stay out of Profile payloads.

use super::*;
use crate::live::floor;
use crate::provider::{AuthBindingSource, Provider};
use crate::services::profile::{native_config, ClaudeProjectConnection};

const API_ENV: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_CUSTOM_HEADERS",
];

pub(super) fn is_direct(payload: &ProfilePayload, app: &AppType) -> bool {
    app == &AppType::Claude && payload.claude_project_connection == ClaudeProjectConnection::Direct
}

fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(format!("Claude Code 项目独立直连：{message}"))
}

fn provider_env(provider: &Provider) -> Result<&Map<String, Value>, AppError> {
    provider
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("供应商缺少 API 连接配置"))
}

fn env_text<'a>(env: &'a Map<String, Value>, key: &str) -> Result<&'a str, AppError> {
    match env.get(key) {
        None => Ok(""),
        Some(Value::String(value)) => Ok(value.trim()),
        Some(_) => Err(invalid(&format!("供应商的 {key} 必须是字符串"))),
    }
}

pub(crate) fn validate_provider(provider: &Provider) -> Result<(), AppError> {
    validate_env_keys(&provider.settings_config)?;
    if provider
        .meta
        .as_ref()
        .is_some_and(|meta| meta.is_full_url == Some(true))
    {
        return Err(invalid(
            "完整请求地址需要本地路由，请选择标准 Anthropic API 基础地址",
        ));
    }
    if provider.uses_managed_account_auth()
        || provider
            .meta
            .as_ref()
            .and_then(|meta| meta.auth_binding.as_ref())
            .is_some_and(|binding| binding.source == AuthBindingSource::ManagedAccount)
    {
        return Err(invalid(
            "托管账号需要本地路由，请选择使用 API Key 或 Token 的供应商",
        ));
    }
    for format in [
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_format.as_deref()),
        provider
            .settings_config
            .get("api_format")
            .and_then(Value::as_str),
        provider
            .settings_config
            .get("apiFormat")
            .and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    {
        if !format.is_empty() && format != "anthropic" {
            return Err(invalid(
                "仅支持 Anthropic Messages 协议；协议转换请使用本地路由",
            ));
        }
    }
    let env = provider_env(provider)?;
    for (key, value) in env {
        if native_config::claude_connection_env(key) && !API_ENV.contains(&key.as_str()) {
            let disabled = value.as_str().is_some_and(|value| {
                value.trim().is_empty()
                    || floor::CLAUDE_PROTOCOL_SELECTORS.contains(&key.as_str())
                        && matches!(value.trim(), "0" | "false")
            });
            if !disabled {
                return Err(invalid(&format!(
                    "暂不支持包含 {key} 的连接，请使用全局连接或路由"
                )));
            }
        }
    }
    for (key, value) in provider.settings_config.as_object().into_iter().flatten() {
        if native_config::claude_connection_top(key)
            && !value.as_str().is_some_and(|value| value.trim().is_empty())
        {
            return Err(invalid(&format!(
                "暂不支持 {key}，请选择静态 API Key 或 Token 连接"
            )));
        }
    }
    let url = url::Url::parse(env_text(env, "ANTHROPIC_BASE_URL")?)
        .map_err(|_| invalid("API 地址必须是有效的 HTTP 或 HTTPS 地址"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("API 地址必须是无嵌入凭据的 HTTP 或 HTTPS 地址"));
    }
    let token = env_text(env, "ANTHROPIC_AUTH_TOKEN")?;
    let key = env_text(env, "ANTHROPIC_API_KEY")?;
    if token.is_empty() == key.is_empty() {
        return Err(invalid(
            "必须且只能配置一种认证：ANTHROPIC_AUTH_TOKEN 或 ANTHROPIC_API_KEY",
        ));
    }
    for value in [token, key] {
        if value == crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER
            || value.contains("${")
            || value.chars().any(char::is_control)
        {
            return Err(invalid(
                "请填写真实的静态 API Key 或 Token，不能使用路由占位值或变量引用",
            ));
        }
    }
    Ok(())
}

fn validate_env_keys(config: &Value) -> Result<(), AppError> {
    if let Some(env) = config.get("env").and_then(Value::as_object) {
        for key in env.keys() {
            if API_ENV
                .iter()
                .chain(floor::CLAUDE_PROTOCOL_SELECTORS)
                .any(|canonical| key.eq_ignore_ascii_case(canonical) && key != canonical)
            {
                return Err(invalid(
                    "API 与协议环境变量必须使用标准大写名称，请先整理重复或大小写不同的连接字段",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn native_behavior(
    root: &Path,
    payload: &ProfilePayload,
    provider: &Provider,
) -> Result<String, AppError> {
    // Windows environment variable names are case-insensitive. Do not merge
    // differently-cased credential keys and leave their effective value ambiguous.
    for path in [
        crate::config::get_claude_settings_path(),
        root.join(".claude/settings.json"),
    ] {
        let Some(bytes) = read_optional(&path)? else {
            continue;
        };
        let config: Value = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("继承的 Claude 设置无法解析，尚未写入项目连接"))?;
        validate_env_keys(&config)?;
    }
    let text = native_config::normalize(
        &AppType::Claude,
        payload.native_config.claude.as_deref().unwrap_or("{}"),
        true,
    )?;
    let mut config: Value =
        serde_json::from_str(&text).map_err(|_| invalid("项目行为配置无法解析"))?;
    validate_env_keys(&config)?;
    let env = config
        .as_object_mut()
        .expect("normalized JSON")
        .entry("env")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("normalized env");
    // Compatibility flags belong to native behavior ownership. Profile overrides win.
    for key in floor::CLAUDE_EXCLUSIVE_ENV {
        if let Some(value) = provider_env(provider)?.get(*key) {
            if !value.is_string() {
                return Err(invalid(&format!("供应商的 {key} 必须是字符串")));
            }
            env.entry((*key).to_string())
                .or_insert_with(|| value.clone());
        }
    }
    Ok(config.to_string())
}

pub(super) fn provider(state: &AppState, payload: &ProfilePayload) -> Result<Provider, AppError> {
    let id = payload
        .providers
        .claude
        .as_deref()
        .ok_or_else(|| invalid("请选择供应商"))?;
    let provider = state
        .db
        .read_provider(&AppType::Claude, id)?
        .and_then(Result::ok)
        .ok_or_else(|| invalid("所选供应商已删除或不可读，请重新选择"))?;
    validate_provider(&provider)?;
    Ok(provider)
}

pub(super) fn model(payload: &ProfilePayload, provider: &Provider) -> Result<String, AppError> {
    let model = payload
        .models
        .claude
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            provider
                .settings_config
                .pointer("/env/ANTHROPIC_MODEL")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            provider
                .settings_config
                .get("model")
                .and_then(Value::as_str)
        })
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| invalid("请选择此供应商可用的项目模型"))?;
    validate_model(model)?;
    Ok(model.to_string())
}

pub(super) fn validate_model(model: &str) -> Result<(), AppError> {
    if !matches!(
        crate::mode::stack::decode(&AppType::Claude, model),
        crate::mode::stack::Decoded::Plain
    ) {
        return Err(invalid("聚合模型需要本地路由，请选择供应商的原始模型名称"));
    }
    Ok(())
}

fn allowed_field(key: &str) -> bool {
    key == "apiKeyHelper"
        || key.strip_prefix("env.").is_some_and(|key| {
            API_ENV.contains(&key) || floor::CLAUDE_PROTOCOL_SELECTORS.contains(&key)
        })
}

pub(super) struct ConnectionOperation {
    key: String,
    pub operation: ModelOperation,
}

pub(super) fn plan(
    provider: Option<&Provider>,
    settings: &ClaudeSettingsDocument,
    manifest: &ProjectManifest,
) -> Result<Vec<ConnectionOperation>, AppError> {
    if provider.is_some() {
        validate_env_keys(&settings.root)?;
    }
    let mut desired = BTreeMap::new();
    if let Some(provider) = provider {
        let env = provider_env(provider)?;
        for key in API_ENV {
            desired.insert(format!("env.{key}"), env_text(env, key)?.to_string());
        }
        // Empty values override inherited credentials and cloud protocol selectors.
        desired.insert("apiKeyHelper".into(), String::new());
        for key in floor::CLAUDE_PROTOCOL_SELECTORS {
            desired.insert(format!("env.{key}"), String::new());
        }
    }
    let keys = desired
        .keys()
        .chain(manifest.claude_connection_hashes.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    keys.into_iter()
        .map(|key| {
            if !allowed_field(&key) {
                return Err(invalid("项目连接部署清单包含不支持的字段"));
            }
            let current = if let Some(env) = key.strip_prefix("env.") {
                settings.root.get("env").and_then(|value| value.get(env))
            } else {
                settings.root.get(&key)
            };
            let text = current
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| invalid("项目连接字段无法读取"))?;
            let wanted = desired
                .get(&key)
                .map(|value| serde_json::to_string(value).expect("string"));
            let mut operation = plan_model_field_op(
                &format!("{CLAUDE_PROJECT_SETTINGS}#{key}"),
                Some(wanted),
                text,
                manifest.claude_connection_hashes.get(&key),
            )
            .expect("connection fields are managed");
            // Even a matching handwritten URL/key must be explicitly taken over as
            // part of the group. Otherwise restoring inheritance could retain its
            // URL while deleting our key, sending the global credential there.
            if provider.is_some()
                && current.is_some()
                && !manifest.claude_connection_hashes.contains_key(&key)
            {
                operation.conflict = Some(ProjectDeployConflict {
                    path: format!("{CLAUDE_PROJECT_SETTINGS}#{key}"),
                    reason: "项目已有手写连接字段；覆盖将把整组连接交由方案管理".into(),
                });
            }
            // A deleted credential is also an external change, not permission to restore it.
            if current.is_none()
                && manifest.claude_connection_hashes.contains_key(&key)
                && operation.desired.is_some()
            {
                operation.conflict = Some(ProjectDeployConflict {
                    path: format!("{CLAUDE_PROJECT_SETTINGS}#{key}"),
                    reason: "项目连接字段在上次部署后被删除".into(),
                });
            }
            Ok(ConnectionOperation { key, operation })
        })
        .collect()
}

/// Stage the entire group before the model writer atomically saves settings.local.json.
pub(super) fn stage(
    operations: &[ConnectionOperation],
    settings: &mut ClaudeSettingsDocument,
    manifest: &mut ProjectManifest,
    changed: &mut Vec<String>,
) -> Result<(), AppError> {
    for item in operations {
        let operation = &item.operation;
        if operation.no_op
            && !manifest.claude_connection_hashes.contains_key(&item.key)
            && operation.conflict.is_none()
        {
            continue; // Matching hand-written values keep their ownership.
        }
        if !operation.no_op {
            let desired: Option<String> = operation
                .desired
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|_| invalid("项目连接字段无法写入"))?;
            if let Some(key) = item.key.strip_prefix("env.") {
                settings.set_env_setting(key, desired.as_deref())?;
            } else {
                let root = settings.root.as_object_mut().expect("validated settings");
                match desired {
                    Some(value) => {
                        root.insert(item.key.clone(), value.into());
                    }
                    None => {
                        root.remove(&item.key);
                    }
                }
                settings.changed = true;
            }
            changed.push(format!("{CLAUDE_PROJECT_SETTINGS}#{}", item.key));
        }
        match &operation.desired_hash {
            Some(hash) => {
                manifest
                    .claude_connection_hashes
                    .insert(item.key.clone(), hash.clone());
            }
            None => {
                manifest.claude_connection_hashes.remove(&item.key);
            }
        }
    }
    Ok(())
}

pub(super) fn ensure_untracked(root: &Path) -> Result<(), AppError> {
    if !root.ancestors().any(|path| path.join(".git").exists()) {
        return Ok(());
    }
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(root).args([
        "-c",
        "core.fsmonitor=false",
        "ls-files",
        "--error-unmatch",
        "--",
        CLAUDE_PROJECT_SETTINGS,
    ]);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .output()
        .map_err(|_| invalid("无法检查项目 Git 跟踪状态，尚未写入凭据"))?;
    match output.status.code() {
        Some(0) => Err(invalid(
            "settings.local.json 已被 Git 跟踪，请先停止跟踪该文件，再应用独立连接",
        )),
        Some(1) => Ok(()),
        _ => Err(invalid("无法检查项目 Git 跟踪状态，尚未写入凭据")),
    }
}

pub(super) fn ensure_ignored(
    root: &Path,
    undo: &mut Vec<ProjectFileUndo>,
    changed: &mut Vec<String>,
) -> Result<(), AppError> {
    ensure_untracked(root)?;
    ensure_project_parent(root, Path::new(".claude"), true)?;
    let path = root.join(".claude/.gitignore");
    ensure_regular_target(&path)?;
    let original = read_optional(&path)?;
    let mut text = std::str::from_utf8(original.as_deref().unwrap_or_default())
        .map_err(|_| invalid(".claude/.gitignore 不是 UTF-8 文本，尚未写入凭据"))?
        .to_string();
    // Append last so an earlier negation cannot expose the credential file.
    if text.lines().last().map(str::trim) == Some("/settings.local.json") {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("/settings.local.json\n");
    let previous = original.as_deref().map(crate::live::engine::sha256_hex);
    write_if_unchanged(&path, previous.as_deref(), text.as_bytes())?;
    undo.push(ProjectFileUndo {
        path,
        original,
        applied_hash: Some(text_hash(&text)),
    });
    changed.push(".claude/.gitignore".into());
    Ok(())
}

#[cfg(test)]
mod tests;
