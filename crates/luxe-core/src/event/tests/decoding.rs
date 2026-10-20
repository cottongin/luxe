//! Decoding — what the derived decoders accept beyond what the serializer writes.

use super::*;

/// What a terse sender leaves out still decodes, field by field, the way the hand-written decoders
/// read it before the derived ones replaced them.
///
/// The break this catches: the mirror enums decode only what their attributes say, so a
/// `#[serde(default)]` missing there turns a frame a client or an older luxe sends into an error.
/// Every row leaves out a field that is not an `Option`, which decodes as `None` regardless.
#[test]
fn what_a_sender_may_leave_out_still_decodes() {
    for action in [
        r#"{"kind":"list_directory","content":{"request_id":"listing:1"}}"#,
        r#"{"kind":"commit_review_chapters","content":{"request_id":"review-save:2","review_id":"review:1","revision_id":"revision:1"}}"#,
        r#"{"kind":"open_pull_request","content":{"request_id":"pr-open:1","title":"Retry once","body":"Why."}}"#,
        r#"{"kind":"open_stack","content":{"request_id":"pr-open:1"}}"#,
    ] {
        let decoded = serde_json::from_str::<Action>(action);
        assert!(!matches!(decoded, Ok(Action::Other { .. }) | Err(_)), "{action}: {decoded:?}");
    }
    for event in [
        r#"{"kind":"token_delta","content":{"delta":"hi"}}"#,
        r#"{"kind":"thinking_delta","content":{"delta":"hm"}}"#,
        r#"{"kind":"github_reply_drafted","content":{"request_id":"d:1","body":null,"error":null}}"#,
        r#"{"kind":"permission_requested","content":{"id":"perm:1","session_id":"s","tool":"bash","permission":"bash","summary":"Run: ls"}}"#,
        r#"{"kind":"question_asked","content":{"id":"ask:1","session_id":"s","prompt":"Which?"}}"#,
        r#"{"kind":"review_finished","content":{"verdict":"continue"}}"#,
        r#"{"kind":"turn_usage","content":{"prompt_tokens":1,"completion_tokens":2,"cost_usd":null}}"#,
    ] {
        let decoded = serde_json::from_str::<Event>(event);
        assert!(!matches!(decoded, Ok(Event::Other { .. }) | Err(_)), "{event}: {decoded:?}");
    }
}

/// A bare action decodes for exactly the kinds the hand-written decoder took bare: the payload-less
/// ones, and the three whose payload is wholly optional.
///
/// The break this catches: reading no content as `{}` is what lets those three go bare, and it
/// would quietly do the same for any other payload whose fields all default.
#[test]
fn a_bare_action_decodes_only_where_it_always_did() {
    let bare: Vec<&str> = Action::KINDS
        .iter()
        .copied()
        .filter(|kind| serde_json::from_str::<Action>(&format!(r#"{{"kind":"{kind}"}}"#)).is_ok())
        .collect();
    assert_eq!(
        bare,
        [
            "withdraw_steering",
            "cancel",
            "retry",
            "regenerate",
            "request_review",
            "new_session",
            "refresh_sessions",
            "refresh_workspace",
            "request_context_composition",
            "compact_now",
            "list_github",
            "check_worktree_removal",
            "close",
        ]
    );
    for frame in [r#"{"kind":"request_review"}"#, r#"{"kind":"request_review","content":null}"#] {
        assert_eq!(
            serde_json::from_str::<Action>(frame).unwrap(),
            Action::RequestReview { focus: None },
            "{frame}"
        );
    }
}

/// An event's `"content":null` is not missing content: a payload event refuses it, as it always
/// did, even where every field would default.
#[test]
fn a_null_payload_on_an_event_is_refused() {
    for frame in
        [r#"{"kind":"session_usage","content":null}"#, r#"{"kind":"plan_usage","content":null}"#]
    {
        assert!(serde_json::from_str::<Event>(frame).is_err(), "{frame}");
    }
}

#[test]
fn action_non_exhaustive_forward_compat() {
    // A kind chosen not to exist. This test used to name a real candidate (`switch_role`) and
    // started failing the day that shipped, which proved the guard works and made the test a
    // hostage to the roadmap.
    let json = r#"{"kind":"a_variant_from_the_future","anything":"at all"}"#;
    let decoded: Action = serde_json::from_str(json).expect("should deserialize unknown variant");
    match decoded {
        Action::Other { kind, content } => {
            assert_eq!(kind, "a_variant_from_the_future");
            assert_eq!(content, serde_json::from_str::<serde_json::Value>(json).unwrap());
        }
        _ => panic!("expected Other variant, got something else"),
    }
}

#[test]
fn event_non_exhaustive_forward_compat() {
    // Simulate a future variant.
    let json = r#"{"kind":"session_archived","path":"/tmp/sess.jsonl"}"#;
    let decoded: Event = serde_json::from_str(json).expect("should deserialize unknown variant");
    match decoded {
        Event::Other { kind, content } => {
            assert_eq!(kind, "session_archived");
            assert_eq!(content["path"], "/tmp/sess.jsonl");
        }
        _ => panic!("expected Other variant, got something else"),
    }
}

/// A payload-less *event* is accepted empty and refused with content, same as an action.
///
/// The break this catches: inverting the guard, which would reject the two forms every
/// serializer actually emits and accept the one that means a payload was lost.
#[test]
fn a_payload_less_event_is_accepted_empty_and_refused_with_content() {
    for form in [
        r#"{"kind":"workspace_refresh_requested"}"#,
        r#"{"kind":"workspace_refresh_requested","content":null}"#,
        r#"{"kind":"workspace_refresh_requested","content":{}}"#,
    ] {
        assert_eq!(
            serde_json::from_str::<Event>(form).unwrap(),
            Event::WorkspaceRefreshRequested,
            "{form}"
        );
    }
    assert!(
        serde_json::from_str::<Event>(
            r#"{"kind":"workspace_refresh_requested","content":{"scope":"x"}}"#
        )
        .is_err(),
        "content on a payload-less event means something was lost in translation"
    );
}

/// A payload-less frame is accepted in all three forms a hand-written client produces, and a
/// payload on one is still an error rather than something silently dropped.
#[test]
fn payload_less_frames_are_forgiving_but_not_lax() {
    for form in [
        r#"{"kind":"cancel"}"#,
        r#"{"kind":"cancel","content":null}"#,
        r#"{"kind":"cancel","content":{}}"#,
    ] {
        assert_eq!(serde_json::from_str::<Action>(form).unwrap(), Action::Cancel, "{form}");
    }
    let error = serde_json::from_str::<Action>(r#"{"kind":"cancel","content":"oops"}"#)
        .expect_err("content on a payload-less frame is a mistake worth naming");
    assert!(error.to_string().contains("cancel takes no content"), "{error}");
}

/// A known frame with the wrong payload fails, and the message names the frame.
///
/// Without the name, a client author gets `missing field `text`` with no way to tell which of
/// the three text-carrying frames they got wrong.
#[test]
fn a_malformed_payload_names_the_frame_it_came_from() {
    let error = serde_json::from_str::<Action>(r#"{"kind":"steer_turn","content":{}}"#)
        .expect_err("steer_turn requires text");
    assert!(error.to_string().starts_with("steer_turn:"), "{error}");

    let error = serde_json::from_str::<Action>(
        r#"{"kind":"respond_permission","content":{"id":"perm:1"}}"#,
    )
    .expect_err("respond_permission requires a reply");
    assert!(error.to_string().starts_with("respond_permission:"), "{error}");
}

/// A malformed payload is an error, never a panic. The deserializer used to index its content
/// map directly (`obj["block"]`), so a truncated or foreign event took the process down
/// instead of being rejected — in the one impl whose entire job is to return `Err`.
#[test]
fn a_malformed_payload_is_rejected_rather_than_fatal() {
    for malformed in [
        r#"{"kind":"token_delta","content":{}}"#,
        r#"{"kind":"tool_started","content":{"call_id":"c1"}}"#,
        r#"{"kind":"tool_ended","content":{}}"#,
        r#"{"kind":"context_usage","content":{}}"#,
        r#"{"kind":"compaction_done","content":{}}"#,
        r#"{"kind":"turn_ended","content":{}}"#,
        r#"{"kind":"permission_requested","content":{"id":"x"}}"#,
        // a count past what its field holds
        r#"{"kind":"context_usage","content":{"used_tokens":4294967296,"context_window":8}}"#,
        r#"{"kind":"provider_retrying","content":{"attempt":4294967296,"attempts":3,"reason":"x"}}"#,
        // loose values the hand-written decoders read as defaults, refused now like any other
        r#"{"kind":"token_delta","content":{"block":"3","delta":"hi"}}"#,
        r#"{"kind":"tool_call_composing","content":{"call_id":"c1","name":"bash","subject":5}}"#,
    ] {
        let decoded = serde_json::from_str::<Event>(malformed);
        assert!(decoded.is_err(), "{malformed} decoded as {decoded:?}");
    }
}

/// Every tag the serializer can emit has a variant in the deserializer's mirror enum.
///
/// `Event` derives `Serialize` from itself but `Deserialize` from a mirror of its variants, and
/// an unrecognized tag degrades to [`Event::Other`] rather than failing. Those two facts together
/// mean a variant added without its mirror breaks nothing loudly — it just quietly stops arriving
/// at the front-end, as `Other`, forever.
///
/// Content is omitted on purpose: a *known* tag then fails on its missing payload, which is
/// fine here. Only an *unknown* tag yields `Other`, so that is the single thing asserted.
#[test]
fn every_event_tag_has_a_deserializer_arm() {
    let missing: Vec<&str> = EVENT_TAGS
        .iter()
        .copied()
        .filter(|tag| {
            let decoded = serde_json::from_str::<Event>(&format!(r#"{{"kind":"{tag}"}}"#));
            matches!(decoded, Ok(Event::Other { .. }))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "these variants have no arm in Event's deserializer, so each reaches a front-end as \
             Other: {missing:?}"
    );
}
