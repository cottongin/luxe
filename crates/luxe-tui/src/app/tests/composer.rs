//! Composer — typing, pasting, submitting and the slash picker.

use super::*;

#[tokio::test]
async fn test_app_slash_quit() {
    let (tx, _rx) = mpsc::channel(100);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);

    // Type `/quit` — slash flyout opens.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)));
    assert!(app.overlays.slash_flyout.is_some());

    // Type `quit` to filter.
    for c in "quit".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
    }

    // Press Enter to dispatch.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.should_quit);
}

#[test]
fn printable_input_bypasses_legacy_plain_character_shortcuts() {
    let (tx, _rx) = mpsc::channel(8);
    let overrides = std::collections::HashMap::from([
        ("tui.convo.search".to_string(), vec!["f".to_string()]),
        ("tui.convo.select_block_up".to_string(), vec!["b".to_string()]),
        ("tui.convo.regenerate".to_string(), vec!["shift+g".to_string()]),
        ("tui.convo.copy_message".to_string(), vec!["y".to_string()]),
    ]);
    let keymap = luxe_core::keymap::KeyMap::defaults().with_overrides(overrides);
    let mut app = App::new(tx, keymap, luxe_core::config::Config::default());

    for (character, modifiers, action) in [
        ('f', KeyModifiers::NONE, UiAction::SearchConvo),
        ('b', KeyModifiers::NONE, UiAction::SelectBlockUp),
        ('g', KeyModifiers::SHIFT, UiAction::Regenerate),
        ('y', KeyModifiers::NONE, UiAction::CopyMessage),
    ] {
        let key = KeyEvent::new(KeyCode::Char(character), modifiers);
        assert_eq!(input::resolve_key_event(&app.keymap, key), Some(action));
        app.update(Msg::Key(key));
    }

    assert_eq!(app.editor.current_input(), "fbgy");
    assert!(!app.search_state.is_active());
    assert!(app.selected_block.is_none());

    // A capital is the shifted letter however the terminal flags it, so `shift+g` is pressable,
    // and typing one into the composer still types it
    for modifiers in [KeyModifiers::SHIFT, KeyModifiers::NONE] {
        let key = KeyEvent::new(KeyCode::Char('G'), modifiers);
        assert_eq!(input::resolve_key_event(&app.keymap, key), Some(UiAction::Regenerate));
    }
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)));
    assert_eq!(app.editor.current_input(), "fbgyG");
}

/// A submission with no model chosen opens the picker and keeps the draft.
///
/// luxe picks no model for the reader, so this is a state a session can legitimately be in. The
/// engine refuses such a turn anyway; the point of asking here is that the reader's message is
/// still in the composer when they come back from the picker, rather than gone into a turn that
/// was never going to run.
#[tokio::test]
async fn a_submission_with_no_model_opens_the_picker_and_keeps_the_draft() {
    use luxe_core::provider::{ModelInfo, ModelRef};

    let (tx, mut rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    let model = ModelInfo { r#ref: ModelRef::new("openai", "gpt-5.6"), ..Default::default() };
    app.model_sources = Some(crate::model_picker::ModelSources::reading(std::sync::Arc::new({
        let model = model.clone();
        move || crate::model_picker::ModelMenu {
            catalog: vec![crate::model_picker::ModelEntry {
                label: "gpt-5.6".into(),
                info: model.clone(),
                configured: true,
                unverified: false,
            }],
            active: None,
        }
    })));
    assert!(!app.model_selected(), "nothing is chosen for the reader");

    app.editor.insert_text("explain this stall");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert!(app.overlays.model_picker.is_some(), "the picker is what Enter reached");
    assert_eq!(app.editor.current_input(), "explain this stall", "the draft survived");
    assert!(rx.try_recv().is_err(), "nothing was sent to the engine");
    let said: Vec<&str> = app
        .convo
        .iter()
        .filter_map(|item| match item.block() {
            ConvoBlock::System(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(said.len(), 1, "said once: {said:?}");
    assert!(said[0].contains("No model selected"), "{said:?}");

    // An empty composer has no reason to open anything.
    let quiet = App::new(
        tokio::sync::mpsc::channel(8).0,
        luxe_core::keymap::KeyMap::defaults(),
        luxe_core::config::Config::default(),
    );
    let mut quiet = quiet;
    quiet.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(quiet.overlays.model_picker.is_none(), "Enter on nothing asked for a model");
    assert!(quiet.convo.is_empty(), "and said nothing about one");

    // Pressing Enter again says nothing new — the picker opening is the message by then.
    app.overlays.model_picker = None;
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.overlays.model_picker.is_some());
    assert_eq!(
        app.convo.iter().filter(|item| matches!(item.block(), ConvoBlock::System(_))).count(),
        1
    );

    // And once one is chosen, the draft that was waiting goes.
    app.overlays.model_picker = None;
    app.apply_model_selection(model);
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(matches!(rx.try_recv(), Ok(Action::SetModel(_))), "the engine is told what was picked");
    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => {
            assert_eq!(content, "explain this stall")
        }
        other => panic!("expected the held draft to be sent, got {other:?}"),
    }
}

/// Regression: pressing Enter on a typed message must forward it to the engine as
/// `SubmitUserMessage` (previously the editor's submitted text was silently dropped,
/// so the TUI could never hold a conversation).
#[tokio::test]
async fn test_enter_submits_user_message_to_engine() {
    let (tx, mut rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    // A turn needs a model, and nothing picks one for the reader.
    app.status.model = "test-model".into();
    app.review.last_turn = Some(("previous-session".into(), "previous-turn".into()));
    app.editor.insert_text("hello world");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    // The message reached the engine channel.
    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => assert_eq!(content, "hello world"),
        other => panic!("expected SubmitUserMessage, got {other:?}"),
    }
    // It is echoed locally and the buffer is cleared.
    assert!(
        matches!(app.convo.last().map(ConversationItem::block), Some(ConvoBlock::User { text: t, .. }) if t == "hello world")
    );
    assert!(app.editor.current_input().is_empty());
    assert!(matches!(app.status.state, AppState::Streaming));
    assert!(app.status.turn_started.is_some());
    assert!(app.review.last_turn.is_none());
}

/// The bug this exists to stop: an API key pasted into the login wizard went to the composer
/// behind it, so the prompt looked dead and the key turned up in the next session's draft — on
/// screen, in the transcript, and on its way to a provider that had no business with it.
#[tokio::test]
async fn a_paste_lands_in_the_login_prompt_rather_than_the_composer() {
    use crate::auth_wizard::{AuthUiMsg, AuthWizardState};
    use luxe_core::auth::{AuthMethod, AuthPrompt, CredentialStatus, LoginScope, ProviderOption};

    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    let provider = ProviderOption {
        id: "openai".into(),
        name: "OpenAI".into(),
        methods: vec![AuthMethod::api_key(None)],
        status: CredentialStatus::Missing,
        note: None,
        console_url: None,
        keyless: false,
        needs_endpoint: false,
    };
    let mut wizard = AuthWizardState::open(vec![provider], LoginScope::ApiKey);
    wizard.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let (reply, _answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("OpenAI API key:", None), reply });
    app.overlays.auth_wizard = Some(wizard);

    // A key copied out of a console arrives with the newline that ended the line.
    app.update(Msg::PasteText("sk-not-a-real-key\n".to_string()));

    let wizard = app.overlays.auth_wizard.as_ref().expect("the wizard is still open");
    assert_eq!(
        wizard.pending_prompt().map(|(_, input)| input),
        Some("sk-not-a-real-key"),
        "the prompt takes the paste, and the trailing newline is not part of the key"
    );
    assert_eq!(app.editor.current_input(), "", "nothing reaches the composer behind it");
}

/// Every other surface that owns input answers the same way: it either types the paste or eats
/// it, and the composer nobody can see is never the fallback.
#[tokio::test]
async fn an_overlay_never_leaks_a_paste_into_the_composer() {
    use crate::model_picker::{ModelEntry, ModelPickerState};
    use luxe_core::provider::{ModelInfo, ModelRef};

    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    // A search box takes it, the way typing would.
    let entries = ["gpt-5.6", "claude-sonnet-5"].map(|id| ModelEntry {
        info: ModelInfo { r#ref: ModelRef::new("openai", id), ..Default::default() },
        label: id.to_string(),
        configured: true,
        unverified: false,
    });
    app.overlays.model_picker = Some(ModelPickerState::open(entries.to_vec(), None));
    app.update(Msg::PasteText("claude".to_string()));
    let picker = app.overlays.model_picker.as_ref().expect("still open");
    assert_eq!(picker.query(), "claude");
    assert_eq!(picker.match_count(), 1);
    assert_eq!(app.editor.current_input(), "");

    // So does the command palette, which opens over anything.
    app.overlays.model_picker = None;
    app.overlays.palette =
        Some(crate::palette::PaletteState::open(super::commands::command_menu(&app)));
    app.update(Msg::PasteText("theme".to_string()));
    assert_eq!(app.overlays.palette.as_ref().map(|palette| palette.query()), Some("theme"));
    assert_eq!(app.editor.current_input(), "");
    app.overlays.palette = None;

    // An overlay with no field of its own eats it instead. Each is checked because each is a
    // separate chance to fall through to a composer the reader cannot see.
    type Show = fn(&mut App);
    let open: [(&str, Show); 4] = [
        ("the thinking picker", |app| app.open_thinking_picker()),
        ("the theme picker", |app| app.open_theme_picker()),
        ("the context list", |app| app.open_context_files()),
        ("the viewer", |a| a.overlays.image_viewer = Some(crate::graphics::ImageKey::new("img"))),
    ];
    for (surface, show) in open {
        show(&mut app);
        app.update(Msg::PasteText("sk-not-a-real-key".to_string()));
        assert_eq!(app.editor.current_input(), "", "{surface} let a paste through");
        app.overlays = crate::app::Overlays::default();
    }

    // The start screen's prompt is the composer, so a paste there lands in it and narrows the list.
    app.overlays.palette = None;
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    app.update(Msg::PasteText("a pasted prompt".to_string()));
    assert_eq!(app.editor.current_input(), "a pasted prompt", "the paste is the prompt");
}

#[tokio::test]
async fn bracketed_paste_stays_compact_but_submits_exact_text() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    app.status.model = "test-model".into();
    app.update(Msg::PasteText("alpha\r\nbeta\r\ngamma".to_string()));
    assert_eq!(app.editor.current_input(), "[Pasted 3 lines]");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => {
            assert_eq!(content, "alpha\nbeta\ngamma");
        }
        other => panic!("expected expanded pasted text, got {other:?}"),
    }
    // The compact token is a composing affordance. Once submitted, the transcript shows the
    // message that was actually sent.
    assert!(matches!(
        app.convo.first().map(ConversationItem::block),
        Some(ConvoBlock::User { text, .. }) if text == "alpha\nbeta\ngamma"
    ));
}

/// Shift+Enter inserts a newline instead of submitting.
#[tokio::test]
async fn test_shift_enter_does_not_submit() {
    let (tx, mut rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.editor.insert_text("line1");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));

    assert!(rx.try_recv().is_err(), "shift+enter must not submit");
    assert_eq!(app.editor.current_input(), "line1\n");
}

#[test]
fn shift_enter_edits_the_composer_while_completion_is_open() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    for character in "/he".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }
    assert!(app.overlays.slash_flyout.is_some());
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));
    assert_eq!(app.editor.current_input(), "/he\n");
    assert!(app.overlays.slash_flyout.is_none());

    app.editor.cancel();
    for character in "@c".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }
    assert!(app.overlays.mention.is_some());
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));
    assert_eq!(app.editor.current_input(), "@c\n");
    assert!(app.overlays.mention.is_none());
    assert!(rx.try_recv().is_err(), "shift+enter must not dispatch through a completion");
}

#[test]
fn slash_picker_only_opens_at_command_position() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    for character in "explain /help".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }

    assert!(app.overlays.slash_flyout.is_none());
    assert_eq!(app.editor.current_input(), "explain /help");
}

#[test]
fn slash_query_lives_in_the_editor_and_escape_preserves_it() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    for character in "/he".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }
    assert_eq!(app.editor.current_input(), "/he");
    assert_eq!(app.overlays.slash_flyout.as_ref().map(|state| state.query()), Some("he"));

    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.overlays.slash_flyout.is_none());
    assert_eq!(app.editor.current_input(), "/he");
}

/// Tab completes a partial slash token into the composer, leaving room for an argument.
///
/// Enter used to do this too for any command that takes arguments, so picking `/diff` printed
/// `"/diff "` and stopped — which reads as the key having done nothing, and was reported as
/// exactly that. Enter now runs the command; completion is Tab's job.
#[test]
fn tab_completes_a_partial_slash_token_and_enter_runs_the_command() {
    let (tx, _rx) = mpsc::channel(8);
    let new_app = || {
        App::new(
            tx.clone(),
            luxe_core::keymap::KeyMap::defaults(),
            luxe_core::config::Config::default(),
        )
    };
    let type_partial = |app: &mut App| {
        for character in "/di".chars() {
            app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
        }
    };

    let mut app = new_app();
    type_partial(&mut app);
    app.update(Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert_eq!(app.editor.current_input(), "/diff ", "Tab completes, ready for a target");
    assert_eq!(
        app.overlays.slash_flyout.as_ref().and_then(|flyout| flyout.arguments_of()),
        Some("/diff"),
        "and the picker moves on to the targets it takes"
    );
    assert!(matches!(app.current_view, ViewMode::Chat));

    let mut app = new_app();
    type_partial(&mut app);
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert_eq!(app.editor.current_input(), "", "Enter runs it and clears the draft");
    assert!(app.overlays.slash_flyout.is_none());
}

/// A launcher's key hands the terminal over between frames: it is queued for the loop, types
/// nothing, and wins over an action the keymap binds to the same chord.
#[tokio::test]
async fn a_launchers_key_queues_the_launch_and_types_nothing() {
    let (tx, _rx) = mpsc::channel(16);
    // Alt+D is the dock's by default; the reader bound it to something of their own
    let config = luxe_core::config::Config {
        launchers: vec![luxe_core::config::Launcher {
            key: "alt+d".into(),
            command: "gh-dash".into(),
            args: Vec::new(),
        }],
        ..Default::default()
    };
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    let dock_before = app.dock.runtime_override;
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT)));
    assert_eq!(app.outbox.launch.as_ref().map(|l| l.command.as_str()), Some("gh-dash"));
    assert_eq!(app.editor.current_input(), "");
    assert_eq!(app.dock.runtime_override, dock_before, "the dock's binding stood aside");
    app.outbox.launch = None;
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)));
    assert!(app.outbox.launch.is_none());
    assert_eq!(app.editor.current_input(), "d", "a plain d is typing");
}

/// Through the whole key path, not just the editor: Ctrl+A reaches the composer as the start of
/// the line — what macOS terminals send for Cmd+← — and select-all moved to Alt+A, since terminals
/// keep Ctrl+Shift+A for their own. Alt+← is Guided Review's "back to chat", and in chat a word.
#[tokio::test]
async fn composer_keys_reach_the_editor_and_select_all_moved() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.editor.insert_text("first line\nsecond line");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)));
    assert_eq!(app.editor.cursor(), "first line\n".len());
    assert_eq!(app.editor.selection(), None, "ctrl+a moves; it no longer selects");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)));
    assert_eq!(app.editor.cursor(), "first line\nsecond ".len());

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT)));
    assert_eq!(app.editor.selected_text().as_deref(), Some("first line\nsecond line"));
    assert_eq!(app.editor.current_input(), "first line\nsecond line", "nothing typed");
}

/// `/editor` opens the project, a file in it, or a file at a line, through the main loop.
#[test]
fn slash_editor_asks_the_loop_to_open_the_project_or_a_file() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new_fixture(tx, KeyMap::defaults(), Config::default());
    let root = repository_root();
    app.project_root = Some(root.clone());
    let run = |app: &mut App, typed: &str| {
        let command =
            crate::commands::CommandRegistry::all().parse_slash(typed).expect("a command");
        super::commands::dispatch(app, command);
        app.outbox.editor_open.take()
    };
    let project = run(&mut app, "/editor").expect("the project");
    assert_eq!(project, crate::launcher::OpenInEditor { path: root.clone(), line: None });
    let script = run(&mut app, "/editor scripts/install-dev.sh:3").expect("a file");
    assert_eq!(script.path, root.join("scripts/install-dev.sh"));
    assert_eq!(script.line, Some(3));
    assert!(run(&mut app, "/editor no/such/folder/file.rs").is_none(), "a typo opens nothing");
}

/// Typing a command's arguments keeps the picker open on its forms: Tab writes the chosen one in,
/// and Enter runs it once it is whole.
#[test]
fn the_slash_picker_follows_a_command_into_its_arguments() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new_fixture(tx, KeyMap::defaults(), Config::default());
    type_into(&mut app, "/mou");
    // Tab completes the trigger and goes straight on to its forms
    app.update(Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert_eq!(app.editor.current_input(), "/mouse ");
    let listing = |app: &App| {
        app.overlays.slash_flyout.as_ref().and_then(|f| f.arguments_of().map(str::to_string))
    };
    assert_eq!(listing(&app).as_deref(), Some("/mouse"), "the forms of /mouse");

    type_into(&mut app, "of");
    assert!(app.mouse_enabled, "precondition");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(!app.mouse_enabled, "`/mouse of` + Enter ran `/mouse off`");
    assert!(app.editor.current_input().is_empty(), "and the draft is spent");
    assert!(app.overlays.slash_flyout.is_none());
}

/// Put away, a command's forms stay away while its arguments are typed, and come back for the next
/// command.
#[test]
fn esc_puts_the_forms_away_for_that_command() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new_fixture(tx, KeyMap::defaults(), Config::default());
    type_into(&mut app, "/compact ");
    assert!(app.overlays.slash_flyout.is_some(), "the form of /compact, a sentence to come");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    type_into(&mut app, "keep the decisions");
    assert!(app.overlays.slash_flyout.is_none(), "stays away while the sentence is typed");
    assert_eq!(app.editor.current_input(), "/compact keep the decisions");

    app.editor.cancel();
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    type_into(&mut app, "/dock ");
    assert_eq!(
        app.overlays.slash_flyout.as_ref().and_then(|f| f.arguments_of()),
        Some("/dock"),
        "another command's forms are offered again"
    );
}
