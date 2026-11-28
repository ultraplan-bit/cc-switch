use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub id: String,
    pub name: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

/// The last explicit application to an Agent's local instruction file.
/// Kept separately from the shared template, including after that template is deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptApplication {
    pub prompt_id: String,
    pub path: String,
    pub content_hash: String,
}

#[cfg(test)]
mod tests {
    use super::Prompt;

    #[test]
    fn prompt_payload_rejects_removed_agent_and_activation_fields() {
        for (field, value) in [
            ("enabled", serde_json::json!(true)),
            ("app", serde_json::json!("claude")),
            ("app_type", serde_json::json!("claude")),
        ] {
            let mut payload = serde_json::json!({
                "id": "shared", "name": "Rules", "content": "Keep tests passing"
            });
            payload[field] = value;
            assert!(serde_json::from_value::<Prompt>(payload).is_err());
        }
    }
}
