//! Gemini authentication type detection
//!
//! Detects whether a Gemini provider uses PackyCode API Key, Google OAuth, or generic API Key.

use crate::provider::Provider;

/// Gemini authentication type enumeration
///
/// Used to optimize performance by avoiding repeated provider type detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeminiAuthType {
    /// PackyCode provider (uses API Key)
    Packycode,
    /// Google Official (uses OAuth)
    GoogleOfficial,
    /// Generic Gemini provider (uses API Key)
    Generic,
}

// Partner Promotion Key constants
const PACKYCODE_PARTNER_KEY: &str = "packycode";
const GOOGLE_OFFICIAL_PARTNER_KEY: &str = "google-official";

// PackyCode keyword constants
const PACKYCODE_KEYWORDS: [&str; 3] = ["packycode", "packyapi", "packy"];

/// Detect Gemini provider authentication type
///
/// One-time detection to avoid repeated calls to `is_packycode_gemini` and `is_google_official_gemini`.
///
/// # Returns
///
/// - `GeminiAuthType::GoogleOfficial`: Google official, uses OAuth
/// - `GeminiAuthType::Packycode`: PackyCode provider, uses API Key
/// - `GeminiAuthType::Generic`: Other generic providers, uses API Key
pub(crate) fn detect_gemini_auth_type(provider: &Provider) -> GeminiAuthType {
    // Priority 1: Check partner_promotion_key (most reliable)
    if let Some(key) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.partner_promotion_key.as_deref())
    {
        if key.eq_ignore_ascii_case(GOOGLE_OFFICIAL_PARTNER_KEY) {
            return GeminiAuthType::GoogleOfficial;
        }
        if key.eq_ignore_ascii_case(PACKYCODE_PARTNER_KEY) {
            return GeminiAuthType::Packycode;
        }
    }

    // Check PackyCode service keywords
    if contains_packycode_keyword(&provider.name) {
        return GeminiAuthType::Packycode;
    }

    if let Some(site) = provider.website_url.as_deref() {
        if contains_packycode_keyword(site) {
            return GeminiAuthType::Packycode;
        }
    }

    if let Some(base_url) = provider
        .settings_config
        .pointer("/env/GOOGLE_GEMINI_BASE_URL")
        .and_then(|v| v.as_str())
    {
        if contains_packycode_keyword(base_url) {
            return GeminiAuthType::Packycode;
        }
    }

    GeminiAuthType::Generic
}

/// Check if string contains PackyCode related keywords (case-insensitive)
///
/// Keyword list: ["packycode", "packyapi", "packy"]
fn contains_packycode_keyword(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    PACKYCODE_KEYWORDS
        .iter()
        .any(|keyword| lower.contains(keyword))
}

/// Detect if provider is Google Official Gemini (uses OAuth authentication)
///
/// Google Official Gemini uses OAuth personal authentication, no API Key needed.
///
/// This is a convenience wrapper around `detect_gemini_auth_type`.
pub(crate) fn is_google_official_gemini(provider: &Provider) -> bool {
    detect_gemini_auth_type(provider) == GeminiAuthType::GoogleOfficial
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderMeta;
    use serde_json::json;

    #[test]
    fn google_name_does_not_select_official_authentication() {
        let mut provider = Provider::with_id(
            "custom".into(),
            "Google API key".into(),
            json!({"env":{"GEMINI_API_KEY":"custom-key"}}),
            None,
        );
        provider.category = Some("custom".into());
        assert!(!super::super::gemini_direct::is_official(&provider));
        provider.name = "Google".into();
        assert!(!super::super::gemini_direct::is_official(&provider));

        provider.category = Some("official".into());
        assert!(super::super::gemini_direct::is_official(&provider));
        provider.category = None;
        provider.meta = Some(ProviderMeta {
            partner_promotion_key: Some("google-official".into()),
            ..Default::default()
        });
        assert!(super::super::gemini_direct::is_official(&provider));
    }
}
