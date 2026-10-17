use super::*;

/// Every key shows in lower case, as a keybindings file writes it, and parses back to itself.
#[test]
fn a_bare_letter_displays_in_lower_case() {
    for (text, shown) in [("g", "g"), ("ctrl+b", "ctrl+b"), ("shift+n", "shift+n"), ("?", "?")] {
        let chord = KeyChord::parse(text).unwrap();
        assert_eq!(chord.display(), shown);
        assert_eq!(KeyChord::parse(&chord.display()).unwrap(), chord);
    }
}

#[test]
fn parse_simple_chord() {
    let chord = KeyChord::parse("ctrl+k").unwrap();
    assert!(chord.ctrl);
    assert!(!chord.shift);
    assert!(!chord.alt);
    assert_eq!(chord.key, KeyName::Char('k'));
}

#[test]
fn parse_multi_modifier() {
    let chord = KeyChord::parse("ctrl+shift+enter").unwrap();
    assert!(chord.ctrl);
    assert!(chord.shift);
    assert!(!chord.alt);
    assert_eq!(chord.key, KeyName::Enter);
}

#[test]
fn parse_case_insensitive() {
    let a = KeyChord::parse("CTRL+K").unwrap();
    let b = KeyChord::parse("ctrl+k").unwrap();
    assert_eq!(a, b);
}

#[test]
fn parse_named_keys() {
    for (input, expected) in [
        ("enter", KeyName::Enter),
        ("esc", KeyName::Esc),
        ("pageup", KeyName::PageUp),
        ("pgdn", KeyName::PageDown),
        ("insert", KeyName::Insert),
        ("f5", KeyName::F5),
    ] {
        let chord = KeyChord::parse(input).unwrap();
        assert_eq!(chord.key, expected, "failed for input '{input}'");
    }
}

#[test]
fn parse_rejects_unknown_modifier() {
    assert!(KeyChord::parse("meta+k").is_err());
}

#[test]
fn parse_rejects_unknown_key() {
    assert!(KeyChord::parse("ctrl+xyz").is_err());
}

#[test]
fn parse_rejects_empty() {
    assert!(KeyChord::parse("").is_err());
}

#[test]
fn round_trip_display() {
    let chord = KeyChord::parse("ctrl+shift+enter").unwrap();
    let s = chord.display();
    // Display normalizes to title case.
    let reparsed = KeyChord::parse(&s).unwrap();
    assert_eq!(chord, reparsed);
}

/// Help ships with no key, and the id someone may already have bound still resolves and still
/// takes a binding. `Ctrl+?` in particular must not come back: no terminal delivers it as the
/// chord it declares.
#[test]
fn help_ships_unbound_and_stays_bindable() {
    let keymap = KeyMap::defaults();
    assert!(keymap.chords_for(UiAction::ToggleHelp).is_none());
    for shift in [false, true] {
        assert_eq!(keymap.resolve(true, shift, false, &KeyName::Char('?')), None);
    }
    assert_eq!(UiAction::from_id("tui.app.toggle_help"), Some(UiAction::ToggleHelp));
    let bound = KeyMap::defaults().with_overrides(HashMap::from([(
        "tui.app.toggle_help".to_string(),
        vec!["f1".to_string()],
    )]));
    assert_eq!(bound.resolve(false, false, false, &KeyName::F1), Some(UiAction::ToggleHelp));
}

#[test]
fn workspace_actions_have_stable_ids_and_default_toggle_chord() {
    let keymap = KeyMap::defaults();
    assert_eq!(UiAction::from_id("tui.workspace.toggle_dock"), Some(UiAction::ToggleDock));
    assert_eq!(UiAction::ShowDock.id(), "tui.workspace.show_dock");
    assert_eq!(UiAction::HideDock.label(), "Hide Context Dock");
    assert_eq!(keymap.resolve(false, false, true, &KeyName::Char('d')), Some(UiAction::ToggleDock));
    assert!(keymap.chords_for(UiAction::ShowDock).is_none());
    // The informational dock has no keyboard section navigation, so Tab is unbound at the
    // workspace level (it stays free for other surfaces).
    assert_eq!(keymap.resolve(false, false, false, &KeyName::Tab), None);
}

#[test]
fn statusline_and_dock_position_actions_have_stable_ids_and_default_chord() {
    let keymap = KeyMap::defaults();
    assert_eq!(UiAction::from_id("tui.statusline.toggle"), Some(UiAction::ToggleStatusline));
    assert_eq!(UiAction::ToggleStatusline.id(), "tui.statusline.toggle");
    assert_eq!(UiAction::ShowStatusline.id(), "tui.statusline.show");
    assert_eq!(UiAction::HideStatusline.label(), "Hide statusline");
    assert_eq!(UiAction::ResetStatuslineAuto.id(), "tui.statusline.reset_auto");
    assert_eq!(
        UiAction::from_id("tui.workspace.toggle_dock_position"),
        Some(UiAction::ToggleDockPosition)
    );
    assert_eq!(
        keymap.resolve(false, false, true, &KeyName::Char('s')),
        Some(UiAction::ToggleStatusline)
    );
}

#[test]
fn guided_review_actions_have_stable_namespaced_ids() {
    let mut count = 0;
    for action in UiAction::ALL.iter().filter(|action| action.is_review()) {
        assert!(action.id().starts_with("tui.review."), "{action:?}");
        assert!(!action.label().is_empty());
        count += 1;
    }
    assert_eq!(count, 35, "review actions expose stable namespaced ids");
}

/// No two actions share an id, so a keybinding written for one never lands on another.
#[test]
fn every_action_id_round_trips_and_is_its_own() {
    let mut ids: Vec<&str> = UiAction::ALL.iter().map(UiAction::id).collect();
    for action in UiAction::ALL {
        assert_eq!(UiAction::from_id(action.id()), Some(*action), "{action:?}");
    }
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "an id names two actions");
}

/// A chord bound to two actions resolves the same way every time: to the one declared first.
///
/// The break this catches: bindings lived in a `HashMap`, so which action a doubly bound chord
/// fired depended on that run's hash seed.
#[test]
fn a_doubly_bound_chord_resolves_to_the_action_declared_first() {
    let chord = KeyChord::parse("ctrl+y").unwrap();
    for _ in 0..32 {
        let mut keymap = KeyMap::default();
        keymap.bindings.insert(UiAction::DashboardQuit, vec![chord.clone()]);
        keymap.bindings.insert(UiAction::Submit, vec![chord.clone()]);
        let resolved = keymap.resolve(true, false, false, &KeyName::Char('y'));
        assert_eq!(resolved, Some(UiAction::Submit));
    }
}

#[test]
fn guided_review_defaults_match_the_documented_shortcuts() {
    let keymap = KeyMap::defaults();
    assert_eq!(
        keymap.resolve(false, false, false, &KeyName::Char('c')),
        Some(UiAction::ReviewAddFinding)
    );
    assert_eq!(
        keymap.resolve(false, false, false, &KeyName::Char('e')),
        Some(UiAction::ReviewEditFinding)
    );
    assert_eq!(
        keymap.resolve(false, false, false, &KeyName::Char('o')),
        Some(UiAction::ReviewOpenInBrowser)
    );
    assert_eq!(
        keymap.resolve(false, true, false, &KeyName::Char('e')),
        Some(UiAction::ReviewOpenInEditor)
    );
    assert_eq!(
        keymap.resolve(false, false, true, &KeyName::Char('o')),
        Some(UiAction::ChangesOpenInEditor),
        "The preview's, a chord since the composer keeps the keyboard"
    );
    assert_eq!(
        keymap.resolve(false, false, false, &KeyName::Char('x')),
        Some(UiAction::ReviewDeleteFinding)
    );
    assert_eq!(
        keymap.resolve(false, false, true, &KeyName::Char('r')),
        Some(UiAction::ReviewReanchorFinding)
    );
    assert_eq!(
        keymap.resolve(false, false, false, &KeyName::Char('r')),
        Some(UiAction::ReviewResolveFinding)
    );
    assert_eq!(
        keymap.resolve(false, false, true, &KeyName::Char('v')),
        Some(UiAction::ReviewToggleGuide)
    );
    assert_eq!(
        keymap.resolve(false, false, false, &KeyName::Char('g')),
        Some(UiAction::ReviewToggleGuide)
    );
    assert_eq!(
        keymap.resolve(false, false, false, &KeyName::Char(']')),
        Some(UiAction::ReviewChapterNext)
    );
    assert_eq!(
        keymap.resolve(false, true, false, &KeyName::Char('n')),
        Some(UiAction::ReviewHunkPrev)
    );
    assert_eq!(
        keymap.resolve(true, false, false, &KeyName::Char('b')),
        Some(UiAction::ReviewToggleSplit)
    );
    assert!(keymap.chords_for(UiAction::ReviewRefresh).is_some_and(|chords| {
        chords == &vec![KeyChord { ctrl: true, shift: false, alt: false, key: KeyName::Char('r') }]
    }));
    assert!(keymap.chords_for(UiAction::Reload).is_some_and(|chords| {
        chords == &vec![KeyChord { ctrl: true, shift: true, alt: false, key: KeyName::Char('r') }]
    }));
}

#[test]
fn default_keymap_has_no_duplicate_chords() {
    // Every chord in the default map should resolve to exactly one action.
    let map = KeyMap::defaults();
    let mut seen: HashMap<KeyChord, UiAction> = HashMap::new();
    for action in map.actions() {
        if let Some(chords) = map.chords_for(action) {
            for chord in chords {
                if let Some(existing) = seen.get(chord) {
                    panic!(
                        "duplicate chord {:?} maps to both {:?} and {:?}",
                        chord, existing, action
                    );
                }
                seen.insert(chord.clone(), action);
            }
        }
    }
}

#[test]
fn resolve_returns_right_action() {
    let map = KeyMap::defaults();
    // Ctrl+K → OpenPalette (default).
    let action = map.resolve(true, false, false, &KeyName::Char('k'));
    assert_eq!(action, Some(UiAction::OpenPalette));
}

/// The queue keys are a pair, and Alt+Up belongs to the one that gives a message back.
///
/// Line scroll had it first; it moved to Ctrl+Up/Ctrl+Down rather than being dropped, and
/// nothing may quietly claim Alt+Up back — a scroll key that eats the draft you were trying to
/// recover is a worse trade than the one this made.
#[test]
fn the_queue_keys_bracket_each_other_and_scrolling_moved_aside() {
    let map = KeyMap::defaults();
    assert_eq!(map.resolve(false, false, true, &KeyName::Up), Some(UiAction::DequeueMessages));
    assert_eq!(map.resolve(false, false, true, &KeyName::Char('q')), Some(UiAction::QueueMessage));
    assert_eq!(map.resolve(true, false, false, &KeyName::Up), Some(UiAction::ScrollUp));
    assert_eq!(map.resolve(true, false, false, &KeyName::Down), Some(UiAction::ScrollDown));
    assert_eq!(map.resolve(false, false, true, &KeyName::Down), None, "and alt+down is free");
    // Every other way to move the transcript is untouched.
    assert_eq!(map.resolve(false, false, false, &KeyName::PageUp), Some(UiAction::PageUp));
    assert_eq!(map.resolve(false, false, false, &KeyName::PageDown), Some(UiAction::PageDown));
}

#[test]
fn standard_paste_chords_share_one_clipboard_action() {
    let map = KeyMap::defaults();
    assert_eq!(
        map.resolve(true, false, false, &KeyName::Char('v')),
        Some(UiAction::PasteClipboard)
    );
    assert_eq!(map.resolve(false, true, false, &KeyName::Insert), Some(UiAction::PasteClipboard));
    assert_eq!(UiAction::from_id("tui.editor.paste_image"), Some(UiAction::PasteClipboard));
    assert_eq!(UiAction::PasteClipboard.id(), "tui.editor.paste");
}

/// The preview's actions were `tui.changes.*`; a keybindings file written then still binds.
#[test]
fn the_preview_answers_to_its_old_names() {
    assert_eq!(UiAction::from_id("tui.changes.toggle"), Some(UiAction::ToggleChanges));
    assert_eq!(UiAction::ToggleChanges.id(), "tui.preview.toggle");
    assert_eq!(UiAction::from_id("tui.preview.toggle"), Some(UiAction::ToggleChanges));
}

#[test]
fn resolve_returns_none_for_unbound() {
    let map = KeyMap::defaults();
    let action = map.resolve(false, false, false, &KeyName::Char('z'));
    assert_eq!(action, None);
}

#[test]
fn overrides_replace_defaults() {
    let map = KeyMap::defaults();
    let mut overrides = HashMap::new();
    overrides.insert("tui.app.open_palette".to_string(), vec!["ctrl+p".to_string()]);
    let map = map.with_overrides(overrides);

    // Ctrl+K no longer resolves (replaced).
    assert_eq!(map.resolve(true, false, false, &KeyName::Char('k')), None);
    // Ctrl+P now resolves to OpenPalette.
    assert_eq!(map.resolve(true, false, false, &KeyName::Char('p')), Some(UiAction::OpenPalette));
}

#[test]
fn overrides_evict_colliding_default_chords() {
    let mut overrides = HashMap::new();
    overrides.insert("tui.convo.search".to_string(), vec!["f".to_string()]);
    let map = KeyMap::defaults().with_overrides(overrides);

    assert_eq!(map.resolve(false, false, false, &KeyName::Char('f')), Some(UiAction::SearchConvo));
    assert_eq!(map.chords_for(UiAction::ReviewToggleNoise), None);
}

#[test]
fn unknown_action_id_skipped() {
    let map = KeyMap::defaults();
    let mut overrides = HashMap::new();
    overrides.insert("tui.nonexistent.action".to_string(), vec!["ctrl+x".to_string()]);
    let merged = map.clone().with_overrides(overrides);
    assert_eq!(merged.to_config_map(), map.to_config_map(), "an unknown id changes nothing");
}

#[test]
fn unparseable_chord_skipped() {
    let map = KeyMap::defaults();
    let mut overrides = HashMap::new();
    overrides.insert("tui.app.open_palette".to_string(), vec!["garbage!!".to_string()]);
    let map = map.with_overrides(overrides);
    // All chords invalid → binding removed.
    assert_eq!(map.chords_for(UiAction::OpenPalette), None);
}

#[test]
fn to_config_map_round_trips() {
    let map = KeyMap::defaults();
    let config = map.to_config_map();
    // Feeding what it wrote back in as overrides must give the same bindings, so a saved file
    // reads back as the keymap it came from.
    let again = KeyMap::defaults().with_overrides(config.clone());
    assert_eq!(again.to_config_map(), config);
    for action in UiAction::ALL {
        assert_eq!(again.chords_for(*action), map.chords_for(*action), "{}", action.id());
    }
}

/// The ids every `keybindings.json` names, written out: a typo or a rename in the action table
/// passes every round-trip, and breaks the bindings someone already wrote.
///
/// In declaration order, because that order is also which action a doubly bound chord fires.
#[test]
fn the_action_ids_are_pinned() {
    let ids: Vec<&str> = UiAction::ALL.iter().map(UiAction::id).collect();
    assert_eq!(
        ids,
        [
            "tui.editor.submit",
            "tui.editor.cancel",
            "tui.editor.new_line",
            "tui.editor.paste",
            "tui.editor.queue_message",
            "tui.editor.dequeue_message",
            "tui.editor.select_all",
            "tui.app.quit",
            "tui.app.open_palette",
            "tui.app.open_slash_menu",
            "tui.app.open_at_menu",
            "tui.app.toggle_help",
            "tui.app.reload",
            "tui.app.cycle_mode",
            "tui.convo.scroll_up",
            "tui.convo.scroll_down",
            "tui.convo.page_up",
            "tui.convo.page_down",
            "tui.convo.search",
            "tui.convo.copy_message",
            "tui.convo.copy_code",
            "tui.convo.copy_selection",
            "tui.convo.select_block_up",
            "tui.convo.select_block_down",
            "tui.convo.toggle_tool_output",
            "tui.convo.open_image",
            "tui.convo.edit_last",
            "tui.convo.retry",
            "tui.convo.regenerate",
            "tui.convo.edit_resend",
            "tui.workspace.toggle_dock",
            "tui.workspace.show_dock",
            "tui.workspace.hide_dock",
            "tui.workspace.reset_dock_auto",
            "tui.workspace.toggle_dock_position",
            "tui.workspace.focus_files",
            "tui.statusline.toggle",
            "tui.statusline.show",
            "tui.statusline.hide",
            "tui.statusline.reset_auto",
            "tui.app.toggle_mouse",
            "tui.preview.toggle",
            "tui.preview.file_next",
            "tui.preview.file_prev",
            "tui.preview.toggle_word_wrap",
            "tui.preview.open_in_editor",
            "tui.preview.toggle_view",
            "tui.preview.ask_about_selection",
            "tui.review.open",
            "tui.review.open_selection",
            "tui.review.exit",
            "tui.review.return_chat",
            "tui.review.add_finding",
            "tui.review.ask",
            "tui.review.open_in_browser",
            "tui.review.open_in_editor",
            "tui.review.send",
            "tui.review.edit_finding",
            "tui.review.delete_finding",
            "tui.review.reanchor_finding",
            "tui.review.findings",
            "tui.review.resolve_finding",
            "tui.review.finish",
            "tui.review.refresh",
            "tui.review.chapter_next",
            "tui.review.chapter_prev",
            "tui.review.file_next",
            "tui.review.file_prev",
            "tui.review.hunk_next",
            "tui.review.hunk_prev",
            "tui.review.line_next",
            "tui.review.line_prev",
            "tui.review.page_next",
            "tui.review.page_prev",
            "tui.review.scroll_left",
            "tui.review.scroll_right",
            "tui.review.line_start",
            "tui.review.toggle_noise",
            "tui.review.toggle_split",
            "tui.review.toggle_word_wrap",
            "tui.review.toggle_guide",
            "tui.review.source",
            "tui.review.help",
            "tui.dashboard.new_session",
            "tui.dashboard.resume_latest",
            "tui.dashboard.find_session",
            "tui.dashboard.recent_sessions",
            "tui.dashboard.grep_sessions",
            "tui.dashboard.review_changes",
            "tui.dashboard.open_config",
            "tui.dashboard.quit",
        ]
    );
    for (retired, action) in [
        ("tui.editor.paste_image", UiAction::PasteClipboard),
        ("tui.changes.toggle", UiAction::ToggleChanges),
        ("tui.changes.file_next", UiAction::ChangesFileNext),
        ("tui.changes.file_prev", UiAction::ChangesFilePrev),
        ("tui.changes.toggle_word_wrap", UiAction::ChangesToggleWordWrap),
        ("tui.changes.open_in_editor", UiAction::ChangesOpenInEditor),
        ("tui.changes.toggle_view", UiAction::ChangesToggleView),
        ("tui.changes.ask_about_selection", UiAction::AskAboutSelection),
    ] {
        assert_eq!(UiAction::from_id(retired), Some(action), "{retired}");
    }
}

/// Every default binding, written out, so a chord moved between actions in the table fails here.
#[test]
fn the_default_bindings_are_pinned() {
    let mut bound: Vec<(String, Vec<String>)> =
        KeyMap::defaults().to_config_map().into_iter().collect();
    bound.sort();
    let expected: Vec<(&str, Vec<&str>)> = vec![
        ("tui.app.cycle_mode", vec!["shift+tab"]),
        ("tui.app.open_palette", vec!["ctrl+k"]),
        ("tui.app.quit", vec!["ctrl+c"]),
        ("tui.app.reload", vec!["ctrl+shift+r"]),
        ("tui.convo.copy_message", vec!["alt+y"]),
        ("tui.convo.copy_selection", vec!["ctrl+insert", "ctrl+shift+c"]),
        ("tui.convo.edit_resend", vec!["alt+e"]),
        ("tui.convo.open_image", vec!["alt+i"]),
        ("tui.convo.page_down", vec!["pgdn"]),
        ("tui.convo.page_up", vec!["pgup"]),
        ("tui.convo.regenerate", vec!["alt+g"]),
        ("tui.convo.retry", vec!["alt+t"]),
        ("tui.convo.scroll_down", vec!["ctrl+down"]),
        ("tui.convo.scroll_up", vec!["ctrl+up"]),
        ("tui.convo.search", vec!["ctrl+f"]),
        ("tui.convo.select_block_down", vec!["alt+j"]),
        ("tui.convo.select_block_up", vec!["alt+k"]),
        ("tui.convo.toggle_tool_output", vec!["ctrl+o"]),
        ("tui.editor.cancel", vec!["ctrl+esc"]),
        ("tui.editor.dequeue_message", vec!["alt+up"]),
        ("tui.editor.new_line", vec!["shift+enter"]),
        ("tui.editor.paste", vec!["ctrl+v", "shift+insert"]),
        ("tui.editor.queue_message", vec!["alt+q"]),
        ("tui.editor.select_all", vec!["alt+a"]),
        ("tui.editor.submit", vec!["ctrl+enter"]),
        ("tui.preview.ask_about_selection", vec!["alt+l"]),
        ("tui.preview.file_next", vec!["alt+n"]),
        ("tui.preview.file_prev", vec!["alt+p"]),
        ("tui.preview.open_in_editor", vec!["alt+o"]),
        ("tui.preview.toggle", vec!["ctrl+p"]),
        ("tui.preview.toggle_view", vec!["alt+m"]),
        ("tui.preview.toggle_word_wrap", vec!["alt+w"]),
        ("tui.review.add_finding", vec!["c"]),
        ("tui.review.ask", vec!["a"]),
        ("tui.review.chapter_next", vec!["]"]),
        ("tui.review.chapter_prev", vec!["["]),
        ("tui.review.delete_finding", vec!["x"]),
        ("tui.review.edit_finding", vec!["e"]),
        ("tui.review.findings", vec!["alt+c"]),
        ("tui.review.finish", vec!["alt+enter"]),
        ("tui.review.help", vec!["?"]),
        ("tui.review.hunk_next", vec!["n"]),
        ("tui.review.hunk_prev", vec!["shift+n"]),
        ("tui.review.line_start", vec!["home"]),
        ("tui.review.open", vec!["ctrl+g"]),
        ("tui.review.open_in_browser", vec!["o"]),
        ("tui.review.open_in_editor", vec!["shift+e"]),
        ("tui.review.open_selection", vec!["enter"]),
        ("tui.review.reanchor_finding", vec!["alt+r"]),
        ("tui.review.refresh", vec!["ctrl+r"]),
        ("tui.review.resolve_finding", vec!["r"]),
        ("tui.review.return_chat", vec!["alt+left"]),
        ("tui.review.send", vec!["alt+u"]),
        ("tui.review.source", vec!["s"]),
        ("tui.review.toggle_guide", vec!["g", "alt+v"]),
        ("tui.review.toggle_noise", vec!["f"]),
        ("tui.review.toggle_split", vec!["ctrl+b"]),
        ("tui.review.toggle_word_wrap", vec!["w"]),
        ("tui.statusline.toggle", vec!["alt+s"]),
        ("tui.workspace.focus_files", vec!["ctrl+t"]),
        ("tui.workspace.toggle_dock", vec!["alt+d"]),
    ];
    let bound: Vec<(&str, Vec<&str>)> = bound
        .iter()
        .map(|(id, chords)| (id.as_str(), chords.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(bound, expected);
}

/// A file that binds an action under its retired id and its current one is read as the current
/// entry alone: the retired one neither wins nor unbinds anything on its way past.
///
/// The break this catches: both entries named one action, the hash map's order picked which
/// applied last, and the loser still evicted its chords from every other action.
#[test]
fn an_action_bound_under_two_names_takes_its_current_one() {
    for _ in 0..32 {
        let overrides = HashMap::from([
            ("tui.changes.toggle".to_string(), vec!["ctrl+k".to_string()]),
            ("tui.preview.toggle".to_string(), vec!["alt+z".to_string()]),
        ]);
        let keymap = KeyMap::defaults().with_overrides(overrides);
        assert_eq!(
            keymap.chords_for(UiAction::ToggleChanges),
            Some(&vec![KeyChord::parse("alt+z").unwrap()])
        );
        assert_eq!(
            keymap.resolve(true, false, false, &KeyName::Char('k')),
            Some(UiAction::OpenPalette),
            "the retired entry took the palette's chord with it"
        );

        let overrides = HashMap::from([
            ("tui.changes.toggle".to_string(), vec!["alt+x".to_string()]),
            ("tui.preview.toggle".to_string(), Vec::new()),
        ]);
        let keymap = KeyMap::defaults().with_overrides(overrides);
        assert_eq!(keymap.chords_for(UiAction::ToggleChanges), None, "the current id unbinds it");
    }
}
