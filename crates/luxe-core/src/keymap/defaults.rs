//! Default bindings — every action's chords, as a table.
use super::{KeyChord, UiAction};
use std::collections::BTreeMap;

/// Every default binding: an action and its chords, the first being the one hints name.
const DEFAULTS: &[(UiAction, &[&str])] = &[
    // Editor
    (UiAction::Submit, &["ctrl+enter"]),
    (UiAction::Cancel, &["ctrl+esc"]),
    (UiAction::NewLine, &["shift+enter"]),
    (UiAction::PasteClipboard, &["ctrl+v", "shift+insert"]),
    // Submitting mid-turn steers the running turn; this queues a separate follow-up instead.
    // Chat also accepts Alt+Enter for it (handled in the composer, where Alt+Enter is free).
    (UiAction::QueueMessage, &["alt+q"]),
    // The counterpart, and the chord pi uses for it: whatever is still waiting comes back to
    // the composer to be edited or dropped. It took Alt+Up from the transcript's line scroll,
    // which moved to Ctrl+Up/Ctrl+Down — the wheel, PageUp/PageDown and Ctrl+End are the keys
    // people actually reach for, and a message you typed and want back is worth more than one
    // of four ways to move three lines.
    (UiAction::DequeueMessages, &["alt+up"]),
    // Alt+A, because Ctrl+A is the start of the line (readline's key, and what macOS terminals
    // send for Cmd+←) and terminals keep Ctrl+Shift+A for their own select-all
    (UiAction::SelectAll, &["alt+a"]),
    // App
    (UiAction::Quit, &["ctrl+c"]),
    (UiAction::OpenPalette, &["ctrl+k"]),
    (UiAction::Reload, &["ctrl+shift+r"]),
    // NOTE: `ToggleHelp` ships unbound. `Ctrl+?` is undeliverable on a terminal:
    // without the keyboard protocol the control is dropped and a bare `?` is typed, and with it
    // the event carries Shift, which a chord declaring none never matches. `Ctrl+H` works but is
    // Backspace on any terminal set to send BS for it. `/help` and the palette's `Keys` reach
    // the card, and `tui.app.toggle_help` stays bindable for anyone who wants a key of their own.

    // Conversation
    (UiAction::ScrollUp, &["ctrl+up"]),
    (UiAction::ScrollDown, &["ctrl+down"]),
    (UiAction::PageUp, &["pgup"]),
    (UiAction::PageDown, &["pgdn"]),
    (UiAction::SearchConvo, &["ctrl+f"]),
    (UiAction::CopyMessage, &["alt+y"]),
    // The chords terminals send for "copy": Ctrl+Insert is what a desktop copy shortcut
    // usually lowers to (Hyprland/Omarchy's Super+C sends exactly this), and Ctrl+Shift+C is
    // the X11/Linux convention. Both only reach us when the terminal doesn't consume them
    // first — which it only does when *it* has a selection, so there's no conflict.
    (UiAction::CopySelection, &["ctrl+insert", "ctrl+shift+c"]),
    (UiAction::SelectBlockUp, &["alt+k"]),
    (UiAction::SelectBlockDown, &["alt+j"]),
    // Ctrl+O for "open the output", the same chord pi uses. It is a transcript-wide flip rather
    // than one card's, because when you want to see what a tool actually returned you rarely want
    // it for exactly one call.
    (UiAction::ToggleToolOutput, &["ctrl+o"]),
    // Alt+I for "image", in the Alt+letter family the rest of the conversation actions use.
    // Deliberately not Enter: Enter submits, and a viewer is not worth making anyone pause
    // over that.
    (UiAction::OpenImage, &["alt+i"]),
    (UiAction::Retry, &["alt+t"]),
    (UiAction::Regenerate, &["alt+g"]),
    (UiAction::EditResend, &["alt+e"]),
    // Workspace
    (UiAction::ToggleDock, &["alt+d"]),
    // Shift+Tab, as the other agents that have a plan mode bind it. Every surface that already
    // uses it (the `@` picker's tabs, GitHub, the review) is drawn over the composer and takes
    // the key first, so this only ever fires from the composer itself.
    (UiAction::CycleMode, &["shift+tab"]),
    (UiAction::ToggleStatusline, &["alt+s"]),
    // `Ctrl+P` for the preview, which also leaves `Alt+P` free to mean "previous" inside it. It is the
    // chord editors give to "open a file quickly", which is close enough to what this does that
    // the association helps; `Alt+C` would have read better still and belongs to the review's
    // findings drawer, and a chord bound twice resolves to the action `actions.rs` declares first.
    (UiAction::ToggleChanges, &["ctrl+p"]),
    // Next/previous, said in the letters they start with. The composer keeps the keyboard
    // while the pane is open, so a bare `n`/`p` would type rather than navigate; `Alt+J`/`Alt+K`
    // belong to the transcript's block selection and `Alt+Up` to the message tray.
    (UiAction::ChangesFileNext, &["alt+n"]),
    (UiAction::ChangesFilePrev, &["alt+p"]),
    (UiAction::ChangesToggleWordWrap, &["alt+w"]),
    // `O` for open, a chord for the same reason as the rest of the pane's: the composer keeps
    // the keyboard. The review's bare `E` would type a letter here.
    (UiAction::ChangesOpenInEditor, &["alt+o"]),
    // `M` for mode: the file, or what changed in it
    (UiAction::ChangesToggleView, &["alt+m"]),
    // `L` for lines. `Alt+A` would have read better and selects the draft.
    (UiAction::AskAboutSelection, &["alt+l"]),
    // `T` for tree. The composer reads `Ctrl+T` as nothing, and every fuzzy finder that binds
    // it means files by it.
    (UiAction::FocusFiles, &["ctrl+t"]),
    // Guided Review workspace. Contextual routing owns these while review is active.
    // ReviewOpen is the entry point from chat ("g" for Guided review); it no-ops in review.
    (UiAction::ReviewOpen, &["ctrl+g"]),
    // Enter opens what the cursor is on when there is something more to see: a collapsed run
    // of churn, or a comment shown as a gutter label. Safe in chat, where the action is
    // view-gated and Enter still submits.
    (UiAction::ReviewOpenSelection, &["enter"]),
    (UiAction::ReviewReturnChat, &["alt+left"]),
    (UiAction::ReviewAddFinding, &["c"]),
    (UiAction::ReviewEditFinding, &["e"]),
    (UiAction::ReviewDeleteFinding, &["x"]),
    (UiAction::ReviewReanchorFinding, &["alt+r"]),
    (UiAction::ReviewFindings, &["alt+c"]),
    (UiAction::ReviewResolveFinding, &["r"]),
    (UiAction::ReviewFinish, &["alt+enter"]),
    (UiAction::ReviewRefresh, &["ctrl+r"]),
    (UiAction::ReviewChapterNext, &["]"]),
    (UiAction::ReviewChapterPrev, &["["]),
    (UiAction::ReviewHunkNext, &["n"]),
    (UiAction::ReviewHunkPrev, &["shift+n"]),
    (UiAction::ReviewLineStart, &["home"]),
    (UiAction::ReviewToggleNoise, &["f"]),
    (UiAction::ReviewToggleSplit, &["ctrl+b"]),
    (UiAction::ReviewToggleWordWrap, &["w"]),
    (UiAction::ReviewAskQuestion, &["a"]),
    (UiAction::ReviewOpenInBrowser, &["o"]),
    // `E` beside `e`: lowercase edits the comment under the cursor, capital opens the file it
    // is on — the same pairing the GitHub cards use for a draft and the editor it can go to.
    (UiAction::ReviewOpenInEditor, &["shift+e"]),
    // `Alt+S` is the statusline toggle; `Alt+U` reads as "send it up to the agent" and is free.
    (UiAction::ReviewSend, &["alt+u"]),
    // `g` for guide, a bare letter like the review's other reading keys (`f`, `w`, `z`), and
    // the first chord is the one hints name. `Alt+V` still works for hands that learned it.
    // `Tab` stays unbound rather than quietly turning a focus move into a pane move.
    (UiAction::ReviewToggleGuide, &["g", "alt+v"]),
    (UiAction::ReviewSource, &["s"]),
    (UiAction::ReviewHelp, &["?"]),
];

/// The default bindings, parsed.
pub(super) fn bindings() -> BTreeMap<UiAction, Vec<KeyChord>> {
    let bindings: BTreeMap<_, _> = DEFAULTS
        .iter()
        .map(|(action, chords)| {
            let chords = chords
                .iter()
                .map(|chord| {
                    KeyChord::parse(chord).expect("the default chords are written to parse")
                })
                .collect();
            (*action, chords)
        })
        .collect();
    debug_assert_eq!(bindings.len(), DEFAULTS.len(), "an action is listed twice");
    bindings
}
