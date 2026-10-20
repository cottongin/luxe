//! Submitting the composer — refusing without a model, expanding mentions into what is sent and
//! chips into what is shown, then starting a turn or joining the one in flight.

use crate::app::{
    App, AppState, ConversationItemId, ConvoBlock, MidTurnDelivery, PendingSubmission,
};
use crate::editor::AttachedImage;
use crate::glyphs::Icon;
use crate::mentions::{MentionRef, RefKind};
use luxe_core::engine::{Action, ImageAttachment};
use std::ops::Range;

/// Commits the composer content locally and sends its engine action.
///
/// With a turn already in flight the message is *not* a new turn: by default it steers the running
/// one (delivered at the engine's next checkpoint), and `queue` holds it as a separate follow-up
/// turn instead. Neither path resets the turn clock — only a promoted message does.
pub(in crate::app) fn submit_prompt(app: &mut App) {
    submit_prompt_with(app, MidTurnDelivery::Steer);
}

/// Whether a submission made right now would start a turn, rather than joining the one in flight.
///
/// One owner for the question, because two things ask it: what to do with the message, and whether
/// there is a model to run it on.
fn starts_a_turn(app: &App) -> bool {
    !app.turn_in_flight() || app.edit_resend_mode()
}

/// [`submit_prompt`], with a mid-turn submission delivered as `delivery`.
pub(in crate::app) fn submit_prompt_with(app: &mut App, delivery: MidTurnDelivery) {
    if refuse_without_model(app) {
        return;
    }
    let Some((text, images)) = app.editor.submit() else {
        return;
    };
    let (content, refs) = expand_mentions(app, &text);
    let sources = &app.config.mentions;
    let (display, chips) =
        display_with_chips(&text, &refs, |reference| chip_of(sources, reference));
    // A mention is a reference in the message, not an attachment; only images travel beside it
    let attached = images
        .iter()
        .map(|image| {
            crate::glyphs::icon_label(
                Icon::Image.glyph(),
                &format!("{} ({}x{})", image.filename, image.width, image.height),
            )
        })
        .collect::<Vec<_>>();

    // Mid-turn submissions never start a turn, so they leave the turn clock, token counts, and
    // the engine's single-turn model alone — and stay out of the transcript — until they're
    // actually delivered or promoted.
    if !starts_a_turn(app) {
        let draft = text;
        let held = PendingSubmission { display, chips, attached, content, draft, images, delivery };
        deliver_mid_turn(app, held);
        return;
    }
    let message = push_user_turn(app, display, attached, chips, &images);
    send_user_turn(app, content, images, message);
}

/// Refuses a submission there is no model to run, saying which step is missing once and opening
/// the picker. Answers whether it refused.
///
/// Asked *before* the editor is drained, so the draft is still there when the picker closes — the
/// engine refuses a turn with no model either way, and learning that after your message has left
/// the composer is the worst of both. Only of a submission that has something to send and would
/// start a turn: a steer joins a turn already running, which had a model, and an empty composer
/// has no reason to open anything.
fn refuse_without_model(app: &mut App) -> bool {
    let drafting = !app.editor.current_input().trim().is_empty() || app.editor.has_images();
    if !drafting || !starts_a_turn(app) || app.model_selected() {
        return false;
    }
    if !app.said_to_choose_a_model {
        app.said_to_choose_a_model = true;
        // Which step is missing decides what to say. Sending someone to a model picker they
        // cannot fill is how a first run stalls: there are no models until a provider can
        // answer for some, and `open_model_picker` opens setup instead.
        let reason = match app.provider_configured() {
            true => {
                "No model selected — nothing was sent. Choose one and press Enter again; your \
                 message is still in the composer."
            }
            false => {
                "No provider is configured — nothing was sent. Set one up and press Enter \
                 again; your message is still in the composer."
            }
        };
        app.push_convo(ConvoBlock::System(reason.to_string()));
    }
    app.open_model_picker();
    true
}

/// What the model is sent for `text`, each mention replaced by what it references, and the
/// mentions found. Records the MCP servers whose tool names went with it.
fn expand_mentions(app: &mut App, text: &str) -> (String, Vec<MentionRef>) {
    let repo_root = std::env::current_dir().unwrap_or_default();
    let refs = crate::mentions::parse_mentions(text);
    let skills: Vec<String> = match refs.iter().any(|r| r.kind == RefKind::Skill) {
        true => {
            luxe_core::skills::discover(&repo_root).into_iter().map(|skill| skill.name).collect()
        }
        false => Vec::new(),
    };
    let servers = app.workspace.as_ref().map_or(&[][..], |w| &w.tooling.mcp_servers[..]);
    let known = crate::mentions::Known {
        github: &app.github_rows,
        rows: &app.mention_rows,
        sources: &app.config.mentions,
        skills: &skills,
        mcp: servers,
        mcp_listed: &app.mcp_listed,
    };
    let content = crate::mentions::references_text(text, &refs, &repo_root, &known);
    let listed = crate::mentions::mcp_names_sent(&refs, &known);
    app.mcp_listed.extend(listed);
    (content, refs)
}

/// The chip a mention is shown as in the transcript.
fn chip_of(sources: &[luxe_core::config::MentionSource], reference: &MentionRef) -> String {
    match reference.kind {
        RefKind::GitHub => {
            let (repository, _, number) = crate::mentions::split_github_target(&reference.target);
            crate::glyphs::icon_label(Icon::GitHub.glyph(), &format!("{repository}#{number}"))
        }
        RefKind::Skill => crate::glyphs::icon_label(Icon::Skill.glyph(), &reference.target),
        RefKind::Mcp => crate::glyphs::icon_label(Icon::Mcp.glyph(), &reference.target),
        RefKind::Source(ref kind) => {
            let source = sources.iter().find(|source| source.kind() == *kind);
            let name = source.map_or(kind.as_str(), |source| source.name.as_str());
            let glyph = crate::glyphs::configured_glyph(
                source.and_then(|source| source.icon()),
                Icon::MentionSource,
            );
            crate::glyphs::icon_label(glyph, &format!("{name} {}", reference.target))
        }
        _ => crate::glyphs::icon_label(Icon::Attachment.glyph(), &reference.target),
    }
}

/// `text` as the transcript shows it, each mention folded to its chip and the whole trimmed, and
/// where each chip landed.
///
/// The transcript shows what was *sent*, so it starts from the paste-expanded draft rather than
/// the composer's compact `[Pasted N lines]` token — that placeholder is a composing affordance and
/// says nothing about the message once it has been submitted. Mentions are the exception: their
/// expansion is a whole context block, so they stay folded to a chip.
fn display_with_chips(
    text: &str,
    refs: &[MentionRef],
    chip_of: impl Fn(&MentionRef) -> String,
) -> (String, Vec<Range<usize>>) {
    let chips: Vec<String> = refs.iter().map(chip_of).collect();
    let mut display = text.to_string();
    // From the end, so every span still points at the mention it names
    for (reference, chip) in refs.iter().zip(&chips).rev() {
        display.replace_range(reference.span.clone(), chip);
    }
    let lead = display.len() - display.trim_start().len();
    let display = display.trim().to_string();
    // A chip lands where its mention started, moved by what every chip to its left added or took
    // away, and by the whitespace trimmed off the front
    let (mut removed, mut added) = (0, 0);
    let mut placed = Vec::with_capacity(chips.len());
    for (reference, chip) in refs.iter().zip(&chips) {
        let at = reference.span.start - removed + added - lead;
        placed.push(at..at + chip.len());
        removed += reference.span.len();
        added += chip.len();
    }
    placed.retain(|chip| chip.end <= display.len());
    (display, placed)
}

/// Puts a message that starts a turn into the transcript, with its images after it, and answers
/// the message's id.
fn push_user_turn(
    app: &mut App,
    text: String,
    attached: Vec<String>,
    chips: Vec<Range<usize>>,
    images: &[AttachedImage],
) -> Option<ConversationItemId> {
    // The user typing outranks an animation: whatever the previous turn was still revealing appears
    // now, so their message never lands under text that is still arriving.
    app.reveal_catch_up();
    // An edit replaces the message it edits and everything after it, as the engine's branch does
    if app.edit_resend_mode() {
        if let Some(index) = app.last_user_message_index() {
            app.drop_transcript_from(index);
        }
    }
    app.push_convo(ConvoBlock::User { text, attached, chips });
    let message = app.convo.last().map(|item| item.id.clone());
    app.push_attached_images(images);
    message
}

/// Starts the turn for a message already in the transcript as `message`: a fresh one, or the
/// resend of an edit. A send the engine never takes marks the message as never sent.
fn send_user_turn(
    app: &mut App,
    content: String,
    images: Vec<AttachedImage>,
    message: Option<ConversationItemId>,
) {
    app.streaming = false;
    app.status.state = AppState::Streaming;
    app.status.on_turn_start();
    let resend_mode = std::mem::take(&mut app.edit_resend_mode);
    let action = if resend_mode {
        Action::EditResend { content }
    } else {
        let images = images
            .into_iter()
            .map(|image| ImageAttachment {
                data: image.data,
                mime: image.mime,
                name: Some(image.filename),
            })
            .collect();
        Action::SubmitUserMessage { content, images, remediation: None }
    };
    let starts_new_turn = matches!(action, Action::SubmitUserMessage { .. });
    if !app.send_action(action, "send a message") {
        app.never_sent.extend(message);
        app.push_convo(ConvoBlock::System("[engine unavailable — channel closed]".to_string()));
        app.status.state = AppState::Idle;
        return;
    }
    app.own_commit_pending = true;
    if starts_new_turn {
        app.review.last_turn = None;
    }
}

/// Routes a submission made while a turn is running: steer it into the turn, or hold it for one
/// of its own. Either way it is pinned above the composer rather than dropped into the
/// transcript, so the UI never claims a message started work it hasn't started.
///
/// A message with images always waits: steering carries text alone, so steering one would deliver
/// its words and drop its images.
fn deliver_mid_turn(app: &mut App, mut submission: PendingSubmission) {
    if !submission.images.is_empty() {
        submission.delivery = MidTurnDelivery::Queue;
    }
    if submission.delivery == MidTurnDelivery::Steer {
        let content = submission.content.clone();
        if !app.send_action(Action::SteerTurn { content }, "steer the turn") {
            // Nothing the user typed may disappear, least of all when the engine is gone: record
            // it in the transcript as never sent rather than holding it for a delivery that can
            // no longer happen.
            app.record_unsent_submission(submission);
            app.push_convo(ConvoBlock::System("[engine unavailable — channel closed]".to_string()));
            return;
        }
    }
    app.hold_submission(submission);
}

#[cfg(test)]
mod tests;
