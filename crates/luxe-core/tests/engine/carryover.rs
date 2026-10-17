//! Carryover — the reader's mode, model and thinking level, carried into an engine rebuilt to
//! resume another session elsewhere.
//!
//! An engine that loads another session in place keeps all three. One rebuilt for a session in
//! another checkout starts from the last one's carryover, and has to land exactly where that one
//! was: a mode with a model of its own still overriding the reader's pick, and leaving the mode
//! still restoring it.

use std::sync::Arc;

use crate::fixtures::{base_config, first, MockProvider};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::{EngineState, Event};
use luxe_core::mode::{Mode, Modes};
use luxe_core::provider::{ModelInfo, ModelRef, ThinkingLevel};
use luxe_core::session::{JsonlSessionStore, SessionStore};

/// A model that reasons, so the level it runs at is the level asked for.
fn model(id: &str) -> ModelInfo {
    ModelInfo { r#ref: ModelRef::new("mock", id), reasoning: true, ..ModelInfo::default() }
}

/// An engine on `launched`, whose plan mode runs on `planner` at `high`.
fn config(store: JsonlSessionStore, session: luxe_core::session::SessionId) -> EngineConfig {
    EngineConfig {
        model: model("launched"),
        modes: Modes::built_in(
            Mode::plan(Vec::new()).with_model(model("planner")).with_thinking(ThinkingLevel::High),
        ),
        ..base_config(store, session, Arc::new(MockProvider::new()))
    }
}

async fn state(events: &mut tokio::sync::mpsc::Receiver<Event>, what: &str) -> EngineState {
    first(events, what, |event| match event {
        Event::StateChanged(state) => Some(state),
        _ => None,
    })
    .await
}

/// The reader picks a model and a level, then plans; the rebuilt engine reports plan on the
/// planner when its session loads, and leaving plan lands on the model and level the reader picked.
#[tokio::test]
async fn a_rebuilt_engine_starts_where_the_last_one_was() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session = store.create("/tmp/test").unwrap();
    let (actions, events, task) = Engine::spawn_task(config(store, session.clone()));
    actions.send(Action::SetModel(model("picked"))).await.unwrap();
    actions.send(Action::SetThinkingLevel(ThinkingLevel::Low)).await.unwrap();
    actions.send(Action::SetMode(luxe_core::mode::PLAN.into())).await.unwrap();
    actions.send(Action::Close).await.unwrap();
    let carryover = task.await.unwrap();
    drop(events);

    let store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let (actions, mut events, _task) =
        Engine::spawn_task_carrying(config(store, session), Some(carryover));
    let started = state(&mut events, "the carried state").await;
    assert_eq!(started.mode, luxe_core::mode::PLAN);
    assert_eq!(started.model.model, "planner", "plan's own model still overrides the pick");

    actions.send(Action::SetMode(luxe_core::mode::BUILD.into())).await.unwrap();
    let built = state(&mut events, "leaving plan").await;
    assert_eq!(built.model.model, "picked", "and leaving plan restores it");
    assert_eq!(built.thinking, ThinkingLevel::Low);
    let _ = actions.send(Action::Close).await;
}

/// Without a carryover an engine starts as it always has, on the launch's model and mode.
#[tokio::test]
async fn without_a_carryover_an_engine_starts_on_its_launch_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session = store.create("/tmp/test").unwrap();
    let (actions, mut events, _task) = Engine::spawn_task_carrying(config(store, session), None);
    let started = state(&mut events, "the session loading").await;
    assert_eq!((started.mode.as_str(), started.model.model.as_str()), ("build", "launched"));
    let _ = actions.send(Action::Close).await;
}
