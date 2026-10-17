use super::headless::headless_json_event;
use super::tui::should_show_dashboard;
use super::worktree::{generated_branch, worktree_branch, worktree_notice};
use crate::engine_setup::ResumeMode;
use luxe_core::config::Config;

#[test]
fn should_show_dashboard_default() {
    let config = Config::default(); // dashboard = None → shown
    assert!(should_show_dashboard(&config, &ResumeMode::New, false));
}

#[test]
fn should_show_dashboard_explicit_true() {
    let config = Config {
        dashboard: Some(luxe_core::config::DashboardSetting::Enabled(true)),
        ..Default::default()
    };
    assert!(should_show_dashboard(&config, &ResumeMode::New, false));
}

#[test]
fn should_hide_dashboard_explicit_false() {
    let config = Config {
        dashboard: Some(luxe_core::config::DashboardSetting::Enabled(false)),
        ..Default::default()
    };
    assert!(!should_show_dashboard(&config, &ResumeMode::New, false));
}

#[test]
fn should_hide_dashboard_on_resume() {
    let config = Config::default();
    assert!(!should_show_dashboard(&config, &ResumeMode::Latest, false));
    assert!(!should_show_dashboard(&config, &ResumeMode::Session("abc".into()), false));
}

#[test]
fn should_hide_dashboard_when_started_on_an_issue_or_a_pull_request() {
    let config = Config {
        dashboard: Some(luxe_core::config::DashboardSetting::Enabled(true)),
        ..Default::default()
    };
    assert!(!should_show_dashboard(&config, &ResumeMode::New, true));
}

// === headless --format json event contract ===

use luxe_core::event::{Event, StopReason};
use luxe_core::message::{ContentBlock, Message, Role};
use luxe_core::session::EntryId;

#[test]
fn json_usage_event_carries_input_output_total() {
    let value = headless_json_event(&Event::TurnUsage {
        prompt_tokens: 1200,
        completion_tokens: 42,
        cost_usd: Some(0.01),
        cache_read_tokens: 900,
        cache_write_tokens: 100,
        reasoning_tokens: 30,
        source: luxe_core::usage::UsageSource::Agent,
    })
    .expect("usage emits a line");
    assert_eq!(value["type"], "usage");
    assert_eq!(value["input"], 1200);
    assert_eq!(value["output"], 42);
    assert_eq!(value["total"], 1242);
    assert_eq!(value["cost_usd"], 0.01);
    assert_eq!(value["cache_read"], 900);
    assert_eq!(value["cache_write"], 100);
    assert_eq!(value["reasoning"], 30);
}

#[test]
fn json_assistant_message_flattens_text_and_names_tool_calls() {
    let message = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Text("Hello ".into()),
            ContentBlock::Text("world".into()),
            ContentBlock::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            },
        ],
    };
    let value = headless_json_event(&Event::MessageCommitted {
        session_id: "s".into(),
        entry_id: EntryId("e1".into()),
        message,
    })
    .expect("assistant message emits a line");
    assert_eq!(value["type"], "message");
    assert_eq!(value["text"], "Hello world");
    assert_eq!(value["tool_calls"], serde_json::json!(["read"]));
}

#[test]
fn json_skips_non_assistant_messages_and_unmapped_events() {
    let user = Message { role: Role::User, content: vec![ContentBlock::Text("hi".into())] };
    assert!(headless_json_event(&Event::MessageCommitted {
        session_id: "s".into(),
        entry_id: EntryId("e2".into()),
        message: user,
    })
    .is_none());
    assert!(headless_json_event(&Event::TokenDelta { block: 0, delta: "x".into() }).is_none());
}

#[test]
fn json_terminal_events_map_to_turn_end_and_error() {
    let end = headless_json_event(&Event::TurnEnded { reason: StopReason::EndTurn })
        .expect("turn_end line");
    assert_eq!(end["type"], "turn_end");
    let err = headless_json_event(&Event::Error("boom".into())).expect("error line");
    assert_eq!(err["type"], "error");
    assert_eq!(err["message"], "boom");
}

#[test]
fn the_worktree_branch_follows_the_flag_then_the_session_then_the_config() {
    let made_up = || "luxe/session-made".to_string();
    let resumed = || Some("topic".to_string());
    assert_eq!(worktree_branch(Some("mine"), resumed(), false, made_up).as_deref(), Some("mine"));
    assert_eq!(worktree_branch(Some(""), resumed(), false, made_up).as_deref(), Some("topic"));
    assert_eq!(
        worktree_branch(Some(""), None, false, made_up).as_deref(),
        Some("luxe/session-made")
    );
    assert_eq!(worktree_branch(None, resumed(), false, made_up).as_deref(), Some("topic"));
    assert_eq!(worktree_branch(None, None, true, made_up).as_deref(), Some("luxe/session-made"));
    assert_eq!(worktree_branch(None, None, false, made_up), None, "nobody asked");
}

#[test]
fn a_made_up_branch_is_short_and_says_whose_it_is() {
    let branch = generated_branch();
    assert!(branch.starts_with("luxe/session-") && branch.len() == "luxe/session-".len() + 8);
    assert_ne!(generated_branch(), branch);
}

#[test]
fn the_worktree_notice_says_where_what_from_and_what_is_untouched() {
    let made = |base: Option<&str>, created, fetch_failed| luxe_core::worktree::SessionWorktree {
        path: std::path::PathBuf::from("/work/app/.luxe/worktrees/topic"),
        branch: "topic".into(),
        base: base.map(str::to_string),
        created,
        fetch_failed,
    };
    let project = std::path::Path::new("/work/app");
    let fresh = worktree_notice(&made(Some("origin/main"), true, false), project);
    assert_eq!(
        fresh,
        "Working in the session's own worktree, `.luxe/worktrees/topic` on branch `topic`, made \
             from `origin/main`. Your checkout is untouched; `/worktree` says more."
    );
    let offline = worktree_notice(&made(Some("origin/main"), true, true), project);
    assert!(offline.contains("as last fetched: the remote could not be reached"), "{offline}");
    let rebuilt = worktree_notice(&made(None, true, false), project);
    assert!(rebuilt.contains("made again from its branch"), "{rebuilt}");
    let found = worktree_notice(&made(None, false, false), project);
    assert!(found.contains("on branch `topic`. Your checkout"), "{found}");
}
