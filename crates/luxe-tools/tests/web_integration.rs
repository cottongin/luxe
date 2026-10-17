// Integration tests for web tools.
//
// These tests verify the full pipeline from tool invocation to result formatting.

use luxe_core::tool::{Tool, ToolContext};
use luxe_tools::web::{SearchHit, WebConfig, WebTools};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

fn test_context(tmp_dir: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: tmp_dir.to_path_buf(),
        session_id: "test-session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

#[tokio::test]
async fn test_web_search_integration_no_query() {
    let tools = WebTools::new(WebConfig::default());
    let tool = tools.web_search();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
}

/// Search with no key is a working capability, not an error — it falls back to the keyless
/// endpoint. Asserted through backend selection rather than by running a search, because a suite
/// that reaches the internet is a suite that fails on a plane.
#[tokio::test]
async fn search_without_a_key_resolves_a_backend_instead_of_failing() {
    let config = WebConfig::default();
    assert!(!config.has_search_key(), "nothing configured");
    assert!(
        luxe_tools::web::create_backend(&config).await.is_ok(),
        "a fresh install still gets web search"
    );
}

#[tokio::test]
async fn test_web_fetch_integration_no_url() {
    let tools = WebTools::new(WebConfig::default());
    let tool = tools.web_fetch();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_web_fetch_integration_no_api_key() {
    let tools = WebTools::new(WebConfig::default());
    let tool = tools.web_fetch();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"url": "https://example.com"}), &ctx).await;
    // web_fetch returns a placeholder result without checking API keys
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_search_hit_serialization() {
    let hits = vec![SearchHit {
        title: "Test Title".to_string(),
        url: "https://example.com".to_string(),
        snippet: "Test Snippet".to_string(),
    }];

    let json_str = serde_json::to_string_pretty(&hits).unwrap();
    assert!(json_str.contains("Test Title"));
    assert!(json_str.contains("https://example.com"));
    assert!(json_str.contains("Test Snippet"));
}

#[tokio::test]
async fn test_search_hit_deserialization() {
    let json = r#"[{"title":"Test","url":"https://example.com","snippet":"Snippet"}]"#;
    let hits: Vec<SearchHit> = serde_json::from_str(json).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Test");
    assert_eq!(hits[0].url, "https://example.com");
    assert_eq!(hits[0].snippet, "Snippet");
}
