//! Copilot models — what the account may use, and enabling the ones that are off.

use serde_json::Value;

use super::super::super::credential::OAuthCredential;
use super::{API_VERSION, DEFAULT_API_BASE, EDITOR_HEADERS};

/// List the models this account may use, enabling any that are merely unconfigured.
///
/// Copilot gates some models (Claude, Gemini) behind a per-account policy acceptance that a user
/// would otherwise have to click through in VS Code. Enabling only the ones the API reports as
/// *not yet configured* keeps this honest: nothing already disabled on purpose is re-enabled.
/// Best-effort throughout — a failure here means a shorter model list, not a failed login.
pub(super) async fn enable_and_list_models(credential: &OAuthCredential) -> Vec<String> {
    let base = credential.endpoint.clone().unwrap_or_else(|| DEFAULT_API_BASE.to_string());
    let Some(models) = fetch_models(&base, credential.access.expose()).await else {
        return Vec::new();
    };

    let pending: Vec<String> =
        models.iter().filter(|model| model.needs_enabling).map(|model| model.id.clone()).collect();
    if !pending.is_empty() {
        for id in &pending {
            let enabled = enable_model(&base, credential.access.expose(), id).await;
            tracing::debug!(model = %id, enabled, "copilot model policy");
        }
        if let Some(refreshed) = fetch_models(&base, credential.access.expose()).await {
            return refreshed.into_iter().filter(|model| model.selectable).map(|m| m.id).collect();
        }
    }
    models.into_iter().filter(|model| model.selectable).map(|model| model.id).collect()
}

/// One row of Copilot's `/models`.
#[derive(Debug, Clone)]
pub(super) struct CopilotModel {
    pub(super) id: String,
    /// Usable from a chat client right now.
    pub(super) selectable: bool,
    /// Gated behind a policy the user hasn't accepted or rejected yet.
    pub(super) needs_enabling: bool,
}

pub(super) async fn fetch_models(base: &str, token: &str) -> Option<Vec<CopilotModel>> {
    let url = format!("{}/models", base.trim_end_matches('/'));
    let mut request = crate::http::shared_client()
        .get(&url)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header("X-GitHub-Api-Version", API_VERSION);
    for (name, value) in EDITOR_HEADERS {
        request = request.header(*name, *value);
    }
    let response = request.send().await.ok()?;
    let body: Value = super::super::read_json(&url, response).await.ok()?;
    Some(parse_models(&body))
}

pub(super) fn parse_models(body: &Value) -> Vec<CopilotModel> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(parse_model).collect())
        .unwrap_or_default()
}

fn parse_model(row: &Value) -> Option<CopilotModel> {
    let id = row.get("id").and_then(Value::as_str)?.to_string();
    let picker = row.get("model_picker_enabled").and_then(Value::as_bool).unwrap_or(false);
    let policy_state =
        row.get("policy").and_then(|policy| policy.get("state")).and_then(Value::as_str);
    let tool_calls = row
        .get("capabilities")
        .and_then(|caps| caps.get("supports"))
        .and_then(|supports| supports.get("tool_calls"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let enabled = policy_state.is_none_or(|state| state == "enabled");
    Some(CopilotModel {
        // An agent harness can't use a model that won't call tools.
        selectable: picker && enabled && tool_calls,
        needs_enabling: picker
            && tool_calls
            && matches!(policy_state, Some(state) if state != "enabled" && state != "disabled"),
        id,
    })
}

async fn enable_model(base: &str, token: &str, model_id: &str) -> bool {
    let url = format!("{}/models/{model_id}/policy", base.trim_end_matches('/'));
    let mut request = crate::http::shared_client()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header("openai-intent", "chat-policy")
        .header("x-interaction-type", "chat-policy");
    for (name, value) in EDITOR_HEADERS {
        request = request.header(*name, *value);
    }
    request
        .json(&serde_json::json!({ "state": "enabled" }))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}
