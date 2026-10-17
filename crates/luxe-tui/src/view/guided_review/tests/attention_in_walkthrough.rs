//! Attention in walkthrough — tests moved out of the module they cover.

use crate::app::Msg;
use crate::fixture::{seeded_app, FixtureScenario};
use luxe_core::guided_review::{AttentionKind, AttentionSignal};

/// A concern about the file on screen belongs on the screen, not only in an index the reviewer
/// consulted before they started reading. Here it sits under the prose that should have
/// explained it, one `c` away from becoming a question the agent has to answer.
#[test]
fn the_walkthrough_carries_the_harness_signals_for_the_file_it_shows() {
    let (width, height) = (170, 44);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewWide).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let review = app.guided_review.clone().expect("the fixture has a review");
    let file = review.files.first().expect("the review has a file");
    let path = file
        .new_path
        .clone()
        .or_else(|| file.old_path.clone())
        .expect("a path")
        .to_string_lossy()
        .into_owned();
    app.review.flags = vec![AttentionSignal {
        kind: AttentionKind::BlindEdit,
        path: Some(path.clone()),
        detail: format!("{path} was edited without being read"),
        aggregate: false,
        file_id: Some(file.id.clone()),
        hunk_id: None,
    }];

    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| super::render(frame, frame.area(), &app))
        .expect("render the walkthrough");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(
        text.contains("edited without being read"),
        "the signal rides with the file in the walkthrough: {text}"
    );
    assert!(
        !text.contains("unread     "),
        "and without a badge column restating its own sentence: {text}"
    );
}

/// Past the cap of two a kind, every flagged file still wears its badge, and each kind over the
/// cap gets its own total rather than only the first kind a count.
#[test]
fn every_flagged_file_wears_its_badge_and_each_kind_over_the_cap_its_total() {
    let secrets = ["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"];
    let unread = ["src/e.rs", "src/f.rs", "src/g.rs"];
    let review = crate::fixture::flagged_review(&secrets, &unread);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::app::App::new(
        tx,
        luxe_core::keymap::KeyMap::defaults(),
        luxe_core::config::Config::default(),
    );
    app.update(Msg::Engine(luxe_core::event::Event::GuidedReviewLoaded {
        request_id: luxe_core::guided_review::ReviewLoadRequestId::new(),
        review: Box::new(review.clone()),
    }));
    let text = |line: ratatui::text::Line<'_>| -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    };

    let badges: Vec<String> = review
        .files
        .iter()
        .map(|file| text(super::brief::file_line(&app, file, false, 100)))
        .collect();
    let (secret, unread_badge) = ("secret?", "unread");
    assert!(badges[..4].iter().all(|row| row.contains(secret)), "{badges:#?}");
    assert!(badges[4..].iter().all(|row| row.contains(unread_badge)), "{badges:#?}");
    let notices: Vec<String> =
        super::brief::notice_lines(&app, &review).into_iter().map(text).collect();
    assert!(notices.contains(&"5 credential-shaped lines".to_string()), "{notices:#?}");
    assert!(notices.contains(&"3 files edited without being read".to_string()), "{notices:#?}");
}

/// Two credential lines in one file are two flags and one thing to say: the walkthrough under that
/// file says it once.
#[test]
fn the_walkthrough_says_a_repeated_reason_once() {
    let (width, height) = (170, 44);
    let review = crate::fixture::flagged_review(&["src/a.rs"], &[]);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::app::App::new(
        tx,
        luxe_core::keymap::KeyMap::defaults(),
        luxe_core::config::Config::default(),
    );
    app.update(Msg::TerminalResized { width, height });
    app.update(Msg::Engine(luxe_core::event::Event::GuidedReviewLoaded {
        request_id: luxe_core::guided_review::ReviewLoadRequestId::new(),
        review: Box::new(review),
    }));
    assert_eq!(app.review.flags.len(), 2, "one flag per credential line");

    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| super::render(frame, frame.area(), &app)).expect("render");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
    assert_eq!(text.matches("adds a line that looks like a credential").count(), 1, "{text}");
}
