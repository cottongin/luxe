// LSP message types for JSON-RPC communication.

use serde::{Deserialize, Serialize};

/// A JSON-RPC message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcMessage {
    /// JSON-RPC version (must be "2.0").
    pub jsonrpc: String,

    /// Message ID (for requests/responses) or null (for notifications).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,

    /// Method name (for requests/notifications) or null (for responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,

    /// Parameters (for requests/notifications).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,

    /// Response result (only for responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Error object (only for error responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i32,

    /// Error message.
    pub message: String,

    /// Optional error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcMessage {
    /// Create a request message.
    pub fn request(id: u64, method: String, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: Some(method),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    /// Create a notification message (no ID).
    pub fn notification(method: String, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: Some(method),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    /// Create a success response.
    pub fn response(id: u64, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: None,
            params: None,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error_response(
        id: u64,
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: None,
            params: None,
            result: None,
            error: Some(JsonRpcError { code, message, data }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let msg = JsonRpcMessage::request(
            1,
            "textDocument/hover".to_string(),
            serde_json::json!({"position": {}}),
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("2.0"));
        assert!(json.contains("textDocument/hover"));
    }

    #[test]
    fn test_notification_serialization() {
        let msg = JsonRpcMessage::notification("initialized".to_string(), serde_json::json!({}));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("initialized"));
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn test_response_serialization() {
        let msg = JsonRpcMessage::response(1, serde_json::json!({"contents": "test"}));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("test"));
        assert!(!json.contains("method"));
    }

    #[test]
    fn test_error_response_serialization() {
        let msg = JsonRpcMessage::error_response(1, -32600, "Invalid Request".to_string(), None);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("-32600"));
        assert!(json.contains("Invalid Request"));
    }

    #[test]
    fn test_roundtrip_request() {
        let original = JsonRpcMessage::request(
            1,
            "textDocument/hover".to_string(),
            serde_json::json!({"position": {}}),
        );
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: JsonRpcMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, original.id);
        assert_eq!(deserialized.method, original.method);
    }
}
