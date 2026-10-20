use super::*;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Result<String, std::env::VarError> {
    let pairs: Vec<(String, String)> =
        pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
    move |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .ok_or(std::env::VarError::NotPresent)
    }
}

#[test]
fn test_noop_notifier_does_nothing() {
    let notifier = NoopNotifier;
    notifier.notify("title", "body"); // Should not panic.
}

#[test]
fn test_mock_notifier_records_calls() {
    let notifier = MockNotifier::new();
    notifier.notify("title", "body");
    let calls = notifier.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], ("title".to_string(), "body".to_string()));
}

/// The default, and the whole point of the switch: nothing is written, so nothing asks the
/// window manager for attention.
#[test]
fn a_bell_nobody_asked_for_is_never_written() {
    assert!(rings(true, true), "asked for, with something there to ring");
    assert!(!rings(false, true), "silent by default, whatever the terminal would do with it");
    assert!(!rings(true, false), "and a redirected log never gets the stray byte");

    let mut sink = Vec::new();
    ring(&mut sink, false).expect("writing nothing cannot fail");
    assert!(sink.is_empty(), "a silent bell writes no bytes at all");
    ring(&mut sink, true).expect("a sink takes the bell");
    assert_eq!(sink, b"\x07");
}

#[test]
fn auto_picks_the_sequence_each_known_terminal_shows() {
    for (pairs, expected) in [
        (vec![("TERM_PROGRAM", "ghostty")], Some(Protocol::Osc9)),
        (vec![("TERM", "xterm-ghostty")], Some(Protocol::Osc9)),
        (vec![("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty")], Some(Protocol::Osc9)),
        (vec![("KITTY_WINDOW_ID", "1")], Some(Protocol::Osc9)),
        (vec![("TERM", "xterm-kitty")], Some(Protocol::Osc9)),
        (vec![("TERM_PROGRAM", "iTerm.app")], Some(Protocol::Osc9)),
        (vec![("LC_TERMINAL", "iTerm2")], Some(Protocol::Osc9)),
        (vec![("ITERM_SESSION_ID", "w0t0p0:1")], Some(Protocol::Osc9)),
        (vec![("TERM_PROGRAM", "WezTerm")], Some(Protocol::Osc9)),
        (vec![("WEZTERM_PANE", "0")], Some(Protocol::Osc9)),
        (vec![("TERM_PROGRAM", "WarpTerminal")], Some(Protocol::Osc9)),
        (vec![("TERM", "foot")], Some(Protocol::Osc777)),
        (vec![("TERM", "foot-extra")], Some(Protocol::Osc777)),
        // Inside tmux, `TERM` and `TERM_PROGRAM` are tmux's; the terminal's own variables remain
        (
            vec![
                ("TERM", "tmux-256color"),
                ("TERM_PROGRAM", "tmux"),
                ("GHOSTTY_RESOURCES_DIR", "/x"),
            ],
            Some(Protocol::Osc9),
        ),
        (vec![("TERM", "alacritty")], None),
        (vec![("TERM_PROGRAM", "Apple_Terminal")], None),
        // herdr's pane: its own `TERM_PROGRAM`, Ghostty's `TERM`, and the outer terminal's
        // variables leaked through. It keeps an OSC 9 for itself, so nothing is sent
        (
            vec![
                ("TERM", "xterm-ghostty"),
                ("TERM_PROGRAM", "herdr"),
                ("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty"),
                ("HERDR_ENV", "1"),
            ],
            None,
        ),
        // tmux inside herdr still is inside herdr
        (vec![("TERM_PROGRAM", "tmux"), ("KITTY_WINDOW_ID", "1"), ("HERDR_ENV", "1")], None),
        (vec![("TERM", "xterm-256color")], None),
        (vec![], None),
    ] {
        assert_eq!(protocol_for(NotificationMethod::Auto, &env(&pairs)), expected, "{pairs:?}");
    }
}

/// The lever for a terminal the list does not know, which `auto` leaves silent.
#[test]
fn naming_a_method_overrides_the_list() {
    let alacritty = env(&[("TERM", "alacritty")]);
    assert_eq!(protocol_for(NotificationMethod::Osc9, &alacritty), Some(Protocol::Osc9));
    assert_eq!(protocol_for(NotificationMethod::Osc777, &alacritty), Some(Protocol::Osc777));
    assert_eq!(protocol_for(NotificationMethod::Unknown, &alacritty), None, "means auto");
}

#[test]
fn a_notifier_exists_only_when_notifications_are_on_and_knows_about_tmux() {
    let ghostty = env(&[("TERM_PROGRAM", "ghostty")]);
    assert_eq!(terminal_notifier(false, NotificationMethod::Auto, &ghostty), None, "off");
    assert_eq!(
        terminal_notifier(true, NotificationMethod::Auto, &ghostty),
        Some(TerminalNotifier { protocol: Protocol::Osc9, tmux: false })
    );
    let in_tmux = env(&[("TERM_PROGRAM", "ghostty"), ("TMUX", "/tmp/tmux-1000/default,1,0")]);
    assert_eq!(
        terminal_notifier(true, NotificationMethod::Auto, &in_tmux),
        Some(TerminalNotifier { protocol: Protocol::Osc9, tmux: true })
    );
    let unknown = env(&[("TERM", "alacritty")]);
    assert_eq!(terminal_notifier(true, NotificationMethod::Auto, &unknown), None);
}

#[test]
fn the_sequence_reaches_a_terminal_and_never_a_redirected_stream() {
    let notifier = TerminalNotifier { protocol: Protocol::Osc9, tmux: false };
    let mut sink = Vec::new();
    notifier.write_to(&mut sink, false, "luxe", "done");
    assert!(sink.is_empty(), "not a terminal: nothing written");
    notifier.write_to(&mut sink, true, "luxe", "done");
    assert_eq!(sink, b"\x1b]9;luxe: done\x07");
}

#[test]
fn each_protocol_is_written_as_its_terminals_expect() {
    assert_eq!(
        sequence(Protocol::Osc9, false, "luxe · app", "Turn finished"),
        "\x1b]9;luxe · app: Turn finished\x07"
    );
    assert_eq!(
        sequence(Protocol::Osc777, false, "luxe; app", "Turn failed: a; b"),
        "\x1b]777;notify;luxe, app;Turn failed: a; b\x07",
        "the title cannot carry the field separator; the last field can"
    );
}

#[test]
fn inside_tmux_the_sequence_is_wrapped_for_passthrough() {
    assert_eq!(
        sequence(Protocol::Osc9, true, "luxe", "done"),
        "\x1bPtmux;\x1b\x1b]9;luxe: done\x07\x1b\\"
    );
}

/// An error message is outside text. A `BEL` in it would end the sequence early and an `ESC`
/// would start another, which the terminal would act on: here, writing the clipboard.
#[test]
fn outside_text_cannot_end_the_sequence_or_start_its_own() {
    let hostile = "boom\x07\x1b]52;c;cHduZWQ=\x07\nsecond line\r\u{9b}31m";
    let written = sequence(Protocol::Osc9, false, "luxe", hostile);
    let inner = &written["\x1b]9;".len()..written.len() - 1];
    assert!(!inner.chars().any(char::is_control), "{written:?}");
    assert_eq!(inner, "luxe: boom ]52;c;cHduZWQ= second line 31m");
}

#[test]
fn a_long_message_is_cut_to_a_glance() {
    let text = clean(&"word ".repeat(100));
    assert_eq!(text.chars().count(), MAX_TEXT_CHARS);
    assert!(text.ends_with('…'));
    assert_eq!(clean(&"x".repeat(MAX_TEXT_CHARS)).chars().count(), MAX_TEXT_CHARS, "whole");
}
