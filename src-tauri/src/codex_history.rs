//! Read-only Codex session group hints after switching connections.

use crate::codex_config::{
    get_codex_config_dir, read_codex_config_text, CC_SWITCH_CODEX_MODEL_PROVIDER_ID,
};
use toml_edit::DocumentMut;

const OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID: &str = "openai";

/// 一次 Codex 切换让新会话换了组（openai ↔ custom），而统一会话开关关着。
/// Codex 桌面版 26.1103（CLI 0.167）起只允许续聊 provider id 与当前相同的线程，
/// 原来那组的会话从此只读，前端据此提示当前分组变化。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexHistorySplitHint {
    /// 切换前新会话记的 provider id
    pub previous_bucket: String,
    /// 切换后新会话记的 provider id
    pub current_bucket: String,
    /// state DB 里记在切换前那组的线程数（现在会被锁成只读）
    pub locked_threads: usize,
}

/// live config.toml 下新会话会记的 provider id：顶层 `model_provider`，缺省是内建 "openai"。
/// 配置读不到或解析失败时为 None。
pub fn codex_live_history_bucket() -> Option<String> {
    let text = read_codex_config_text().ok()?;
    codex_config_text_history_bucket(&text)
}

fn codex_config_text_history_bucket(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID);
    Some(id.to_string())
}

/// 切换完成后判断要不要提示：开关关着、新会话从 openai 换到 custom（或反过来），并且
/// 原来那组确实有线程。只看 CC Switch 自己会写的这两组。
pub fn codex_history_split_hint(previous_bucket: Option<&str>) -> Option<CodexHistorySplitHint> {
    if crate::settings::unify_codex_session_history() {
        return None;
    }
    let config_text = read_codex_config_text().ok()?;
    codex_history_split_hint_for(previous_bucket?, &config_text, |bucket| {
        crate::codex_state_db::count_threads_with_model_provider(
            &get_codex_config_dir(),
            &config_text,
            bucket,
        )
    })
}

fn codex_history_split_hint_for(
    previous_bucket: &str,
    config_text: &str,
    count_threads: impl FnOnce(&str) -> Option<usize>,
) -> Option<CodexHistorySplitHint> {
    let is_managed = |bucket: &str| {
        bucket == OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID
            || bucket == CC_SWITCH_CODEX_MODEL_PROVIDER_ID
    };
    let current_bucket = codex_config_text_history_bucket(config_text)?;
    if previous_bucket == current_bucket
        || !is_managed(previous_bucket)
        || !is_managed(&current_bucket)
    {
        return None;
    }
    let locked_threads = count_threads(previous_bucket)?;
    (locked_threads > 0).then(|| CodexHistorySplitHint {
        previous_bucket: previous_bucket.to_string(),
        current_bucket,
        locked_threads,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_groups_reports_existing_threads_without_rewriting_them() {
        assert_eq!(
            codex_config_text_history_bucket("").as_deref(),
            Some("openai")
        );
        let hint =
            codex_history_split_hint_for("openai", r#"model_provider = "custom""#, |_| Some(3));
        assert_eq!(hint.unwrap().locked_threads, 3);
        assert!(
            codex_history_split_hint_for("custom", r#"model_provider = "custom""#, |_| Some(3))
                .is_none()
        );
        assert!(
            codex_history_split_hint_for("openai", r#"model_provider = "custom""#, |_| Some(0))
                .is_none()
        );
    }
}
