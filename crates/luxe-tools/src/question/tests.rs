use super::*;
use luxe_core::ContentBlock;
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
async fn test_question_missing_prompt() {
    let tool = QuestionTool::new(QuestionConfig::default());
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_question_unattended_mode() {
    let config = QuestionConfig {
        default_mode: "unattended".to_string(),
        unattended_default: Some("Yes".to_string()),
    };
    let tool = QuestionTool::new(config);
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool
        .run(
            serde_json::json!({
                "prompt": "Do you want to proceed?",
                "options": ["Yes", "No"]
            }),
            &ctx,
        )
        .await;

    assert!(!result.is_error);
    let content = match &result.content[0] {
        ContentBlock::Text(s) => s.clone(),
        _ => panic!("Expected Text content block"),
    };
    assert!(content.contains("Auto-answered"));
    assert!(content.contains("Yes"));
}

#[tokio::test]
async fn interactive_question_emits_event_and_awaits_the_brokered_answer() {
    use luxe_core::question::QuestionBroker;
    let broker = Arc::new(QuestionBroker::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let tmp_dir = TempDir::new().unwrap();
    let ctx = ToolContext {
        cwd: tmp_dir.path().to_path_buf(),
        session_id: "s1".into(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: Some(broker.clone()),
    };

    let tool = QuestionTool::new(QuestionConfig::default());
    let handle = tokio::spawn(async move {
        tool.run(serde_json::json!({"prompt": "Pick a stack", "options": ["Rust", "Go"]}), &ctx)
            .await
    });

    // The tool emits QuestionAsked and blocks; answer it via the broker.
    let Some(Event::QuestionAsked { id, prompt, options, .. }) = event_rx.recv().await else {
        panic!("expected a QuestionAsked event");
    };
    assert_eq!(prompt, "Pick a stack");
    assert_eq!(options, vec![QuestionOption::new("Rust"), QuestionOption::new("Go")]);
    broker.resolve(&id, QuestionReply::Answered(vec!["Rust".into()]));

    let result = handle.await.unwrap();
    assert!(!result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("The user answered: Rust"), "got: {text}");
    // Any other front-end showing the question is told to take it down
    let closed = event_rx.recv().await;
    assert_eq!(closed, Some(Event::PromptClosed { id: id.as_str().to_string() }));
}

/// A multi-select answer reaches the model as a list, and comes back off the text as the same
/// list — which is what lets the transcript card name each choice after the modal has closed.
#[tokio::test]
async fn several_answers_reach_the_model_as_a_list_a_card_can_read_back() {
    use luxe_core::question::{QuestionBroker, QuestionOutcome};
    let broker = Arc::new(QuestionBroker::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let tmp_dir = TempDir::new().unwrap();
    let ctx = ToolContext {
        cwd: tmp_dir.path().to_path_buf(),
        session_id: "s1".into(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: Some(broker.clone()),
    };

    let tool = QuestionTool::new(QuestionConfig::default());
    let handle = tokio::spawn(async move {
        tool.run(serde_json::json!({"prompt": "Which features?", "multi": true}), &ctx).await
    });
    let Some(Event::QuestionAsked { id, .. }) = event_rx.recv().await else {
        panic!("expected a QuestionAsked event");
    };
    let chosen = vec!["Dark mode".to_string(), "and, while you are there, vim keys".to_string()];
    broker.resolve(&id, QuestionReply::Answered(chosen.clone()));

    let result = handle.await.unwrap();
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert_eq!(QuestionOutcome::parse(text), Some(QuestionOutcome::Answered(chosen)));
}

#[tokio::test]
async fn cancelled_question_reports_dismissal() {
    use luxe_core::question::QuestionBroker;
    let broker = Arc::new(QuestionBroker::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let tmp_dir = TempDir::new().unwrap();
    let ctx = ToolContext {
        cwd: tmp_dir.path().to_path_buf(),
        session_id: "s1".into(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: Some(broker),
    };

    let tool = QuestionTool::new(QuestionConfig::default());
    let handle =
        tokio::spawn(async move { tool.run(serde_json::json!({"prompt": "?"}), &ctx).await });
    let _ = event_rx.recv().await.expect("QuestionAsked emitted");
    cancel_tx.send(true).unwrap(); // abort the turn

    let result = handle.await.unwrap();
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("dismissed"), "cancel unblocks the tool: {text}");
}

#[tokio::test]
async fn options_carry_a_rationale_and_a_recommendation_to_the_front_end() {
    use luxe_core::question::QuestionBroker;
    let broker = Arc::new(QuestionBroker::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let tmp_dir = TempDir::new().unwrap();
    let ctx = ToolContext {
        cwd: tmp_dir.path().to_path_buf(),
        session_id: "s1".into(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: Some(broker.clone()),
    };

    let tool = QuestionTool::new(QuestionConfig::default());
    let handle = tokio::spawn(async move {
        tool.run(
            serde_json::json!({
                "prompt": "Which runtime?",
                "options": [
                    {"label": "tokio", "description": "already a dependency", "recommended": true},
                    "async-std"
                ]
            }),
            &ctx,
        )
        .await
    });

    let Some(Event::QuestionAsked { id, options, .. }) = event_rx.recv().await else {
        panic!("expected a QuestionAsked event");
    };
    assert_eq!(options[0].description.as_deref(), Some("already a dependency"));
    assert!(options[0].recommended);
    // A bare string still works; it just arrives without a rationale.
    assert_eq!(options[1], QuestionOption::new("async-std"));
    broker.resolve(&id, QuestionReply::Answered(vec!["tokio".into()]));
    assert!(!handle.await.unwrap().is_error);
}

#[tokio::test]
async fn too_many_options_is_rejected_with_an_actionable_message() {
    let tool = QuestionTool::new(QuestionConfig::default());
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool
        .run(
            serde_json::json!({
                "prompt": "Pick one",
                "options": ["a", "b", "c", "d", "e"]
            }),
            &ctx,
        )
        .await;

    assert!(result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("Merge them"), "tells the model what to do instead: {text}");
}

#[tokio::test]
async fn unattended_falls_back_to_the_recommended_option() {
    let tool = QuestionTool::new(QuestionConfig::default());
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool
        .run(
            serde_json::json!({
                "prompt": "Which runtime?",
                "options": [
                    {"label": "async-std"},
                    {"label": "tokio", "recommended": true}
                ]
            }),
            &ctx,
        )
        .await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("Answer: tokio"), "not merely the first option: {text}");
}

/// Asked in another harness's spelling, `question` rather than `prompt`, it still asks.
#[tokio::test]
async fn the_question_may_be_spelled_question() {
    let tool = QuestionTool::new(QuestionConfig::default());
    let tmp_dir = TempDir::new().unwrap();
    let args = serde_json::json!({"question": "Which runtime?", "options": ["tokio", "smol"]});
    let result = tool.run(args, &test_context(tmp_dir.path())).await;
    assert!(!result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("Which runtime?"), "{text}");
}

#[tokio::test]
async fn test_question_unattended_no_default() {
    let config =
        QuestionConfig { default_mode: "unattended".to_string(), unattended_default: None };
    let tool = QuestionTool::new(config);
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool
        .run(
            serde_json::json!({
                "prompt": "Confirm?",
                "default": "Maybe"
            }),
            &ctx,
        )
        .await;

    assert!(!result.is_error);
    let content = match &result.content[0] {
        ContentBlock::Text(s) => s.clone(),
        _ => panic!("Expected Text content block"),
    };
    assert!(content.contains("Maybe"));
}
