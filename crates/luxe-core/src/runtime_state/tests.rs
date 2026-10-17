use super::*;

#[test]
fn a_record_round_trips() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let state = RuntimeState {
        schema_version: RUNTIME_STATE_SCHEMA_VERSION,
        model: Some(RememberedModel { provider: "anthropic".into(), id: "claude-opus-5".into() }),
        thinking: Some(ThinkingLevel::High),
        recent_commands: vec!["cmd.model".into(), "cmd.new".into()],
        panes: PaneWidths { dock: Some(50), review_guide: None, peek: Some(90) },
    };

    state.save_to(&path).unwrap();

    assert_eq!(RuntimeState::load_from(&path), state);
}

/// Forgetting is the right failure mode: a convenience must never break a launch.
#[test]
fn an_unreadable_or_future_record_reads_as_nothing_remembered() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");

    std::fs::write(&path, "{ not json").unwrap();
    assert_eq!(RuntimeState::load_from(&path), RuntimeState::default());

    std::fs::write(&path, r#"{"schema_version":99,"model":{"provider":"x","id":"y"}}"#).unwrap();
    assert_eq!(RuntimeState::load_from(&path).model, None, "a future schema is ignored");
}

/// The whole point: what one session records, the next one reads.
#[test]
fn what_is_remembered_can_be_read_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");

    // A default-constructed record must be readable, or the first write poisons the file.
    RuntimeState::default().save_to(&path).unwrap();
    let mut state = RuntimeState::load_from(&path);
    assert_eq!(state.schema_version, RUNTIME_STATE_SCHEMA_VERSION, "a fresh record is current");

    state.thinking = Some(ThinkingLevel::Low);
    state.save_to(&path).unwrap();
    assert_eq!(
        RuntimeState::load_from(&path).thinking,
        Some(ThinkingLevel::Low),
        "the choice survives the round trip"
    );
}

/// A record written by a build that predates recents, copied from a real `state.json`: it still
/// reads, and reads as nothing used. A round trip cannot catch a rename of the new field, so the
/// current shape is pinned against a literal too.
#[test]
fn the_persisted_shape_is_pinned_and_an_older_record_still_reads() {
    let older = r#"{
  "schema_version": 1,
  "model": {
    "provider": "lmstudio",
    "id": "qwen3.8-27b"
  },
  "thinking": "off"
}"#;
    let state: RuntimeState = serde_json::from_str(older).unwrap();
    assert_eq!(state.model.as_ref().map(|model| model.id.as_str()), Some("qwen3.8-27b"));
    assert!(state.recent_commands.is_empty(), "an older record has used nothing");

    let current = RuntimeState { recent_commands: vec!["cmd.diff".into()], ..state };
    let written: serde_json::Value = serde_json::to_value(&current).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
            "schema_version": 1,
            "model": { "provider": "lmstudio", "id": "qwen3.8-27b" },
            "thinking": "off",
            "recent_commands": ["cmd.diff"]
        }),
        "no widths dragged, so no panes key"
    );

    let dragged = RuntimeState {
        panes: PaneWidths { dock: Some(50), review_guide: None, peek: Some(90) },
        ..current
    };
    assert_eq!(
        serde_json::to_value(&dragged).unwrap()["panes"],
        serde_json::json!({ "dock": 50, "peek": 90 })
    );
}

/// A dragged width is kept beside everything else the record holds, and survives a relaunch.
#[test]
fn pane_widths_are_remembered_without_disturbing_the_rest() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let with_model = RuntimeState {
        model: Some(RememberedModel { provider: "anthropic".into(), id: "claude".into() }),
        ..RuntimeState::default()
    };
    with_model.save_to(&path).unwrap();
    let panes = PaneWidths { dock: Some(48), ..PaneWidths::default() };
    RuntimeState::remember_panes_at(&path, panes);
    let read = RuntimeState::load_from(&path);
    assert_eq!(read.panes, panes);
    assert_eq!(read.model, with_model.model, "the model is still remembered");
}

/// Most recent first, each id once, and the oldest falls off the end.
#[test]
fn a_command_moves_to_the_front_once_and_the_list_is_bounded() {
    let mut recent = Vec::new();
    for id in ["a", "b", "c", "a"] {
        push_recent(&mut recent, id);
    }
    assert_eq!(recent, ["a", "c", "b"]);
    for n in 0..RECENT_COMMANDS_KEPT {
        push_recent(&mut recent, &format!("filler-{n}"));
    }
    assert_eq!(recent.len(), RECENT_COMMANDS_KEPT);
    assert_eq!(recent[0], format!("filler-{}", RECENT_COMMANDS_KEPT - 1));
    assert!(!recent.iter().any(|id| id == "a"), "the oldest fell off");
}

/// Remembering a command leaves what the engine remembered alone.
#[test]
fn remembering_a_command_keeps_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let with_model = RuntimeState {
        model: Some(RememberedModel { provider: "anthropic".into(), id: "opus".into() }),
        ..RuntimeState::default()
    };
    with_model.save_to(&path).unwrap();

    assert_eq!(RuntimeState::remember_command_at(&path, "cmd.pr"), ["cmd.pr"]);
    assert_eq!(RuntimeState::remember_command_at(&path, "cmd.new"), ["cmd.new", "cmd.pr"]);
    let read = RuntimeState::load_from(&path);
    assert_eq!(read.model, with_model.model);
    assert_eq!(read.recent_commands, ["cmd.new", "cmd.pr"]);
}

#[test]
fn a_missing_record_is_simply_empty() {
    let directory = tempfile::tempdir().unwrap();
    let state = RuntimeState::load_from(&directory.path().join("absent.json"));
    assert!(state.model.is_none() && state.thinking.is_none());
}

#[cfg(unix)]
#[test]
fn the_record_is_owner_readable_only() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    RuntimeState::default().save_to(&path).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// The state dir, never the config dir: this is machine-written, not hand-edited. Which
/// directory that *is* differs by platform — `~/.local/state` on unix, `%LOCALAPPDATA%` on
/// Windows — so the assertion is about the shape and the neighborhood, not a literal path. An
/// `XDG_STATE_HOME` moves the neighborhood, and a test run that sets one must still pass.
#[test]
fn the_path_follows_the_state_directory_convention() {
    let rendered = crate::paths::display(&RuntimeState::path());
    assert!(rendered.ends_with("luxe/state.json"), "{rendered}");
    assert!(!rendered.contains("/.config/"), "state is not configuration: {rendered}");
    if cfg!(windows) {
        assert!(rendered.contains("/Local/"), "machine-local, not roaming: {rendered}");
    } else {
        let neighborhood = std::env::var("XDG_STATE_HOME")
            .ok()
            .filter(|dir| dir.starts_with('/'))
            .map_or_else(|| "/.local/state/".to_string(), |dir| format!("{dir}/"));
        assert!(rendered.contains(&neighborhood), "{rendered} is not under {neighborhood}");
    }
}

/// Remembering what is already remembered writes nothing: a file nobody changed keeps its
/// contents, so two Luxe processes that agree do not race to rewrite it.
#[test]
fn an_unchanged_record_is_not_written() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let panes = PaneWidths { dock: Some(50), review_guide: None, peek: Some(90) };
    RuntimeState::remember_panes_at(&path, panes);
    // a hand edit a rewrite would erase
    let marked = std::fs::read_to_string(&path).unwrap().replace("50", "50 ");
    std::fs::write(&path, &marked).unwrap();

    RuntimeState::remember_panes_at(&path, panes);

    assert_eq!(std::fs::read_to_string(&path).unwrap(), marked);
}
