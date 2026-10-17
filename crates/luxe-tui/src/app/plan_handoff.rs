//! Plan handoff — what happens to a plan once plan mode has written it.
//!
//! When a turn in plan mode ends having written a plan file, the TUI asks what next, in the same
//! picker the agent's own questions use. The choice is the reader's and is acted on here rather than
//! sent to the agent: leaving plan mode is not something the model may do for itself, or plan mode
//! would hold only as long as the model agreed to stay in it.

use super::{App, ConvoBlock};
use luxe_core::engine::Action;
use luxe_core::question::{QuestionOption, QuestionRequestId};

const BUILD_HERE: &str = "Build it here";
const BUILD_FRESH: &str = "Build it in a fresh session";
const RUN_AS_LOOP: &str = "Run it as a loop";
const KEEP_PLANNING: &str = "Keep planning";

/// Asks what to do with the plan at `path`, unless something else is already being asked.
pub(super) fn offer(app: &mut App, path: String) {
    if app.overlays.question.is_some() || app.overlays.permission.is_some() {
        return;
    }
    let id = QuestionRequestId::new();
    app.overlays.question = Some(crate::question_prompt::QuestionPromptState::new(
        id.clone(),
        format!("{path} is ready. What next?"),
        vec![
            QuestionOption::new(BUILD_HERE)
                .with_description("this session, with the planning conversation in context")
                .recommended(),
            QuestionOption::new(BUILD_FRESH).with_description(format!(
                "starts from {path} alone, which is cheaper after a long plan"
            )),
            QuestionOption::new(RUN_AS_LOOP)
                .with_description("phase by phase, with your checks and a review at each"),
            QuestionOption::new(KEEP_PLANNING),
        ],
        false,
    ));
    app.plan_handoff = Some(id);
    app.plan_written = Some(path);
}

/// Whether `id` is the handoff's question, which the TUI answers rather than the engine.
pub(super) fn is_handoff(app: &App, id: &QuestionRequestId) -> bool {
    app.plan_handoff.as_ref() == Some(id)
}

/// Acts on the reader's answer to the handoff: a choice, free text, or nothing (dismissed).
pub(super) fn answered(app: &mut App, answers: Option<Vec<String>>) {
    app.plan_handoff = None;
    let path = app.plan_written.take().unwrap_or_else(|| luxe_core::mode::DEFAULT_PLAN_FILE.into());
    let Some(answer) = answers.and_then(|answers| answers.into_iter().next()) else {
        return;
    };
    let build = format!("Build the plan in `{path}`, phase by phase, starting with phase 0.");
    match answer.as_str() {
        KEEP_PLANNING => {}
        BUILD_HERE => {
            set_build(app);
            submit(app, &build);
        }
        BUILD_FRESH => {
            set_build(app);
            // Sent once the new session has loaded, or it would land in the one being left; parked
            // only once there is a new session coming to send it to
            if app.send_action(Action::NewSession, "start a new session") {
                app.pending_build = Some(build);
            }
        }
        RUN_AS_LOOP => {
            set_build(app);
            app.push_convo(ConvoBlock::System(format!(
                "Run `luxe loop start` in this project to build {path} phase by phase. `luxe loop \
                 status` shows how it is going, and `gate` in your config sets what each phase \
                 has to pass."
            )));
        }
        // Typed rather than picked: what to build, said in the reader's own words
        typed => {
            set_build(app);
            submit(app, typed);
        }
    }
}

/// Sends what was waiting for a fresh session, now that one has loaded.
pub(super) fn session_loaded(app: &mut App) {
    // a plan another session wrote is not this session's to offer
    app.plan_written = None;
    if let Some(build) = app.pending_build.take() {
        submit(app, &build);
    }
    // A prompt typed on the start screen, which asked for this session to answer it
    if std::mem::take(&mut app.send_draft_on_load) {
        super::input::submit_prompt(app);
    }
}

fn set_build(app: &mut App) {
    app.send_action(Action::SetMode(luxe_core::mode::BUILD.into()), "change the mode");
}

/// Sends `text` through the composer's own path, so it shows and steers as anything typed does,
/// and leaves whatever the reader had drafted where it was.
fn submit(app: &mut App, text: &str) {
    let draft = app.editor.current_input().to_string();
    app.editor.cancel();
    app.editor.insert_text(text);
    super::input::submit_prompt(app);
    if !draft.is_empty() {
        app.editor.insert_text(&draft);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{seeded_app, seeded_session, FixtureScenario};
    use luxe_core::event::Event;

    /// A plan written in a turn that then failed is still offered once a turn finishes, in the
    /// same session; a session that loads instead does not inherit it.
    #[test]
    fn a_written_plan_outlives_a_failed_turn_but_not_its_session() {
        let (mut app, _engine) = seeded_session("nord", FixtureScenario::ChatIdle).unwrap();
        app.update(crate::app::Msg::Engine(Event::PlanWritten { path: "PLAN.md".into() }));
        app.update(crate::app::Msg::Engine(Event::Error("the provider went away".into())));
        assert_eq!(app.plan_written.as_deref(), Some("PLAN.md"), "kept for the next turn's end");

        session_loaded(&mut app);
        assert_eq!(app.plan_written, None, "another session's plan is not offered here");
    }

    /// Building fresh parks the build prompt only once a new session is on its way, or it would be
    /// sent into whatever session loads next.
    #[test]
    fn building_fresh_parks_nothing_when_no_session_is_coming() {
        let mut app = seeded_app("nord", FixtureScenario::ChatIdle).unwrap();
        answered(&mut app, Some(vec![BUILD_FRESH.to_string()]));
        assert_eq!(app.pending_build, None);
    }

    /// Building fresh asks for a new session and sends the build prompt once that session loads.
    #[test]
    fn building_fresh_sends_the_prompt_into_the_new_session() {
        let (mut app, mut engine) = seeded_session("nord", FixtureScenario::ChatIdle).unwrap();
        answered(&mut app, Some(vec![BUILD_FRESH.to_string()]));
        let asked: Vec<Action> = std::iter::from_fn(|| engine.try_recv().ok()).collect();
        assert!(asked.iter().any(|action| matches!(action, Action::NewSession)), "{asked:?}");
        assert!(app.pending_build.is_some());

        let loaded = Event::SessionLoaded { session_id: "fresh".into(), messages: Vec::new() };
        app.update(crate::app::Msg::Engine(loaded));
        assert!(app.pending_build.is_none(), "sent into the session that loaded");
        let sent: Vec<Action> = std::iter::from_fn(|| engine.try_recv().ok()).collect();
        let submitted = sent.iter().any(|action| {
            matches!(action, Action::SubmitUserMessage { content, .. } if content.contains("Build the plan"))
        });
        assert!(submitted, "{sent:?}");
    }
}
