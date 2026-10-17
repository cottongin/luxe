//! Chat conversation fixtures — idle, reasoning, streaming, wide prose, a question, a plan, a
//! screenshot, a paste and pending submissions.

use super::*;

/// Idle chat with a fixed short exchange.
pub(super) fn seed_idle(app: &mut App) {
    app.push_convo(ConvoBlock::User {
        text: "Summarize the current project state.".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant(
        "The project is ready for the next focused increment.".into(),
    ));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.workspace = Some(dock_workspace_snapshot());
}

/// Chat showing model reasoning: one expanded and one collapsed thinking block.
pub(super) fn seed_thinking(app: &mut App) {
    app.push_convo(ConvoBlock::User {
        text: "Refactor the parser to stream tokens instead of buffering.".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    // Shaped like real reasoning: a `**title**` per section, symbols in backticks, and a
    // bulleted aside — the markdown a GPT model actually emits.
    app.push_convo(ConvoBlock::Thinking {
        text: "**Tracing the buffering**\n\nThe buffering happens in `Parser::feed`, \
               which collects the whole input before yielding.\nI can turn it into an \
               iterator adapter that emits tokens as bytes arrive, keeping a small carry \
               buffer for partial tokens.\n\n**Edge cases**\n\n- a multi-byte token split \
               across two chunks must not be dropped\n- `feed` still has to report a \
               trailing partial token as an error"
            .into(),
        expanded: true,
    });
    app.push_convo(ConvoBlock::Assistant(
        "I'll convert `Parser::feed` into a streaming adapter with a carry buffer for \
         split tokens."
            .into(),
    ));
    app.push_convo(ConvoBlock::User {
        text: "Good. Now add tests for the split-token case.".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Thinking {
        text: "**Planning the test**\n\nA table-driven test feeding the input one byte at \
               a time exercises every split boundary in a single case."
            .into(),
        expanded: false,
    });
    app.push_convo(ConvoBlock::Assistant(
        "Added a byte-at-a-time table test covering split-token boundaries.".into(),
    ));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

/// Chat caught mid-reveal: a streaming answer with its newest characters still fading in.
pub(super) fn seed_streaming(app: &mut App) {
    // Driven through the production reducer so the reveal arms exactly as it does live, then
    // stepped on a pinned clock: the frame lands mid-sentence, with the fade on the tail.
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);
    app.update(Msg::Engine(Event::TokenDelta {
        block: 0,
        delta: "The provider does not hand text over at the rate it produced it \u{2014} one \
                measured delivery is a single seventy-five character delta every ~470 ms, \
                which paints as two visible jumps a second. A reveal cursor paces those \
                characters back out at the rate the model is actually working at."
            .into(),
    }));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.status.state = crate::app::AppState::Streaming;
}

/// Chat with prose, fenced code, and a table for canvas-width review.
pub(super) fn seed_wide_prose(app: &mut App) {
    app.push_convo(ConvoBlock::Assistant(
        "The conversation canvas keeps the implementation discussion readable on a wide terminal, \
         while preserving a wider surface for code, tables, paths, and other structured details. \
         This fixture makes both policies visible without relying on a live provider.\n\n\
         ```rust\n\
         let widths = conversation_canvas_widths(frame.area().width);\n\
         render_markdown_block(widths.prose_width, widths.wide_width);\n\
         ```\n\n\
         | Content | Canvas |\n\
         | --- | --- |\n\
         | Narrative prose | Readable 100-cell maximum |\n\
         | Code and details | Wide 140-cell maximum |"
            .into(),
    ));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

/// Chat with the interactive question modal open (multi-select + free-text).
pub(super) fn seed_question(app: &mut App) {
    // An earlier decision, as the transcript keeps it: the modal is long gone, so the card
    // is the only record of what was chosen.
    let asked = serde_json::json!({
        "prompt": "Where should the ingest worker live?",
        "options": [{"label": "its own crate"}, {"label": "inside luxe-core"}],
    });
    let mut answered = ToolActivity::running(
        "fixture-session",
        "call-question",
        "question",
        luxe_core::describe_call("question", &asked, "/work/luxe"),
        1,
    );
    answered.state = ToolActivityState::Succeeded;
    answered.summary = Some("Completed".into());
    answered.elapsed_ms = Some(14_500);
    answered.input_preview = Some(asked.to_string());
    answered.output_preview = Some(
        luxe_core::question::QuestionOutcome::Answered(vec!["its own crate".into()]).result_text(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: answered,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    app.push_convo(ConvoBlock::Assistant(
        "Its own crate it is. Before I scaffold it, which async features do you want?".into(),
    ));
    app.overlays.question = Some(crate::question_prompt::QuestionPromptState::new(
        luxe_core::question::QuestionRequestId::new(),
        "Which async features should I enable?".into(),
        vec![
            luxe_core::question::QuestionOption::new("tokio full")
                .with_description("everything the runtime offers; simplest to start from")
                .recommended(),
            luxe_core::question::QuestionOption::new("rustls TLS")
                .with_description("no OpenSSL to build against"),
            luxe_core::question::QuestionOption::new("tracing"),
        ],
        true,
    ));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

/// Plan mode, mid-interview: the composer and the agent's question in the plan color.
pub(super) fn seed_plan(app: &mut App) -> Result<(), String> {
    app.push_convo(ConvoBlock::User {
        text: "Add rate limiting to the ingest API.".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant(
        "The server has no middleware layer yet, so a limiter goes in `router()`. Two \
         decisions before I write the plan."
            .into(),
    ));
    let asked = serde_json::json!({
        "prompt": "Limit per client, or across the whole service?",
        "options": [{"label": "per API key"}, {"label": "global"}],
    });
    let mut answered = ToolActivity::running(
        "fixture-session",
        "call-question",
        "question",
        luxe_core::describe_call("question", &asked, "/work/luxe"),
        1,
    );
    answered.state = ToolActivityState::Succeeded;
    answered.summary = Some("Completed".into());
    answered.elapsed_ms = Some(9_000);
    answered.input_preview = Some(asked.to_string());
    answered.output_preview = Some(
        luxe_core::question::QuestionOutcome::Answered(vec!["per API key".into()]).result_text(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: answered,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    app.push_convo(ConvoBlock::Assistant("Per key it is. Last one:".into()));
    app.overlays.question = Some(crate::question_prompt::QuestionPromptState::new(
        luxe_core::question::QuestionRequestId::new(),
        "What should a client over its limit get?".into(),
        vec![
            luxe_core::question::QuestionOption::new("429 with Retry-After")
                .with_description("what clients and proxies already understand")
                .recommended(),
            luxe_core::question::QuestionOption::new("Queue the request")
                .with_description("smoother, but holds memory under load"),
        ],
        false,
    ));
    app.status.mode = luxe_core::mode::PLAN.into();
    app.status.model = "claude-opus-5".into();
    app.status.provider = "anthropic".into();
    app.status.thinking = luxe_core::provider::ThinkingLevel::High;
    app.workspace = Some(dock_workspace_snapshot());
    Ok(())
}

/// A `screenshot` card showing its capture inline, at whatever fidelity the terminal has.
pub(super) fn seed_screenshot(app: &mut App) -> Result<(), String> {
    // Prose on both sides of the capture, so the scenario also exercises the thing that is
    // genuinely hard: scrolling a picture through a viewport that is made of text.
    app.push_convo(ConvoBlock::Assistant(
        "The header component now reads its spacing from the layout tokens rather than \
         hard-coded padding, so it should line up with the sidebar at every breakpoint. \
         Two things worth checking by eye: the accent rule under the bar, and whether the \
         first card still clears the fold."
            .into(),
    ));
    app.push_convo(ConvoBlock::User {
        text: "Does the dev server render the new header correctly?".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant(
        "Capturing the page now \u{2014} here is what it looks like.".into(),
    ));
    let mut captured = ToolActivity::running(
        "fixture-session",
        "call-shot",
        "screenshot",
        "mode=url http://localhost:4321",
        1,
    );
    captured.revision = 1;
    captured.state = ToolActivityState::Succeeded;
    captured.elapsed_ms = Some(1_820);
    captured.summary = Some("Completed".into());
    captured.output_preview = Some(
        "Captured url via headless browser (1280x800, 143 KB on disk). \
         Saved: .luxe/screenshots/shot-1762000000000.png"
            .into(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: captured,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    let key = crate::graphics::ImageKey::tool_result("fixture-session", "call-shot", 1);
    app.images.insert(
        key.clone(),
        fixture_capture(),
        crate::graphics::ImageOrigin { label: "screenshot".into(), path: None },
    );
    app.tool_images.insert(("fixture-session".into(), "call-shot".into()), vec![key]);
    app.push_convo(ConvoBlock::Assistant(
        "That matches the mock: the accent rule sits flush under the bar and the first \
         card clears the fold with room to spare. The sidebar rows are still using the \
         old 56px rhythm though \u{2014} they should be 48 to match the cards. Want me to \
         change that and re-capture?"
            .into(),
    ));
    Ok(())
}

/// A submitted message carrying a paste long enough to be previewed.
pub(super) fn seed_paste(app: &mut App) {
    let payload = (1..=32)
        .map(|line| format!("    log line {line}: connection reset by peer"))
        .collect::<Vec<_>>()
        .join("\n");
    app.push_convo(ConvoBlock::User {
        text: format!("Here is the failing run:\n{payload}"),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant(
        "That's the socket dropping mid-IDLE. I'll add a reconnect with backoff.".into(),
    ));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.workspace = Some(dock_workspace_snapshot());
}

/// Running turn with mid-turn submissions pinned above the composer.
pub(super) fn seed_pending(app: &mut App) {
    app.push_convo(ConvoBlock::User {
        text: "Wire the idle watcher into the sync loop.".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant(
        "Starting with the IMAP IDLE session, then the reconnect path.".into(),
    ));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.status.state = crate::app::AppState::Streaming;
    app.status.active_tool = Some("read".into());
    app.status.on_turn_start();
    app.workspace = Some(dock_workspace_snapshot());
    app.dock.show();
    for (delivery, text) in [
        (MidTurnDelivery::Steer, "also check the reconnect backoff"),
        (MidTurnDelivery::Queue, "then update the architecture doc"),
        (MidTurnDelivery::Queue, "and run the full gate"),
    ] {
        app.hold_submission(crate::app::PendingSubmission {
            display: text.into(),
            chips: Vec::new(),
            attached: Vec::new(),
            content: text.into(),
            draft: text.into(),
            images: Vec::new(),
            delivery,
        });
    }
    // A review asked for mid-turn waits with them, and says which of the three states it
    // is in rather than taking the screen.
    app.review.queued = Some(crate::app::QueuedReview {
        target: luxe_core::guided_review::ReviewTarget::WorkingTree,
        state: crate::app::QueuedReviewState::Waiting,
    });
}
