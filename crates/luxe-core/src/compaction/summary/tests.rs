use super::*;
use crate::compaction::*;
use crate::message::{ContentBlock, Message, Role};
use crate::session::{Entry, EntryKind};

/// A span with no conversation in it is not compacted, however many entries it holds.
///
/// The break this catches, measured on a loop run: the planner's guard asked only whether the
/// span was non-*empty*, and a span of cost records is not empty — so the summarizer was sent
/// "Summarize the following conversation:" with nothing under it. Three paid calls, each
/// answering, correctly, that no conversation had been provided.
#[test]
fn a_span_of_cost_records_is_not_a_conversation() {
    use crate::usage::{UsageSource, UsageTotals};

    let usage = || Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Usage {
            source: UsageSource::Agent,
            model: "test-model".into(),
            totals: UsageTotals { calls: 1, ..UsageTotals::default() },
        },
    };
    let said = |body: &str| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user(body)),
    };

    assert!(!contributes_to_summary(&usage()));
    assert!(contributes_to_summary(&said("something worth keeping")));
    assert!(
        serialize_for_summary(&[usage(), usage()]).trim().is_empty(),
        "which is exactly what would have been sent"
    );
}

#[test]
fn test_serialize_for_summary() {
    let entries = vec![
        Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Message(Message {
                role: Role::User,
                content: vec![ContentBlock::Text("What is Rust?".to_string())],
            }),
        },
        Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Message(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text(
                    "Rust is a systems programming language.".to_string(),
                )],
            }),
        },
    ];

    let serialized = serialize_for_summary(&entries);
    assert!(serialized.contains("[User]: What is Rust?"));
    assert!(serialized.contains("[Assistant]: Rust is a systems programming language."));
}

fn files<const N: usize>(names: [&str; N]) -> HashSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn message_entry(role: Role, content: Vec<ContentBlock>) -> Entry {
    Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message { role, content }),
    }
}

/// What the summarizer is shown is what the summary can possibly say. Tool activity is most of
/// a coding session, and it used to be dropped entirely — the function truncated "long tool
/// results" it never actually matched.
#[test]
fn the_summarizer_is_shown_the_tool_work_not_only_the_prose() {
    let serialized = serialize_for_summary(&[
        message_entry(
            Role::Assistant,
            vec![
                ContentBlock::Thinking {
                    text: "let me think".into(),
                    signature: None,
                    id: None,
                    encrypted: None,
                    opaque: None,
                },
                ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "edit".into(),
                    arguments: serde_json::json!({ "path": "src/lib.rs" }),
                },
            ],
        ),
        message_entry(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                call_id: "c1".into(),
                content: vec![ToolOutput::Text("no such file".into())],
                is_error: true,
            }],
        ),
    ]);

    assert!(serialized.contains("[Assistant]: called edit"), "{serialized}");
    assert!(serialized.contains("src/lib.rs"), "{serialized}");
    assert!(serialized.contains("[Tool]: failed no such file"), "{serialized}");
    assert!(!serialized.contains("let me think"), "reasoning is not the record: {serialized}");
}

/// A compaction summary is the history everything before it was replaced by, so a later pass
/// summarizing across one has to be shown it.
#[test]
fn an_earlier_compaction_summary_reaches_the_summarizer() {
    let serialized = serialize_for_summary(&[Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Compaction {
            summary: "## Goal\n\nShip the cache fix.".to_string(),
            first_kept: EntryId::new(),
            tokens_before: 90_000,
        },
    }]);
    assert!(serialized.starts_with("[Compaction]: ## Goal"), "{serialized}");
}

/// The summarizer is asked for every section a summary carries, and for no file list.
///
/// The break this catches: the prompt asking for `## Files` again, which spends the summary's
/// allowance on a list that's replaced anyway, or naming fewer sections than the summary is filed
/// under, which leaves the missing ones empty on every compaction.
#[test]
fn the_summarizer_is_asked_for_every_section_but_the_files() {
    for section in REQUIRED_SECTIONS {
        assert!(SUMMARIZER_PROMPT.contains(&format!("`## {section}`")), "{section} not asked for");
    }
    for files in FILE_SECTION_ALIASES {
        assert!(!SUMMARIZER_PROMPT.contains(&format!("## {files}")), "{files} asked for");
    }
}

fn earlier_summary(summary: &str) -> String {
    serialize_for_summary(&[Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Compaction {
            summary: summary.to_string(),
            first_kept: EntryId::new(),
            tokens_before: 90_000,
        },
    }])
}

/// The summarizer reads an earlier summary's prose but not its file list, which code carries.
///
/// The break this catches: showing the list again, which a summarizer copies out path by path,
/// hundreds of them on a long session, until the summary stops short and the compaction fails.
#[test]
fn an_earlier_summarys_file_list_is_kept_from_the_summarizer() {
    let read = earlier_summary("## Goal\n\nShip it.\n\n## Files\n\n- src/listed.rs\n");
    assert!(read.contains("Ship it."), "{read}");
    assert!(!read.contains("src/listed.rs"), "{read}");
}

/// A section after the file list still reaches the summarizer.
///
/// The break this catches: dropping everything from the first file heading on, which loses any
/// section a summary wrote after its list.
#[test]
fn a_section_after_the_file_list_still_reaches_the_summarizer() {
    let read = earlier_summary("## Files\n\n- src/listed.rs\n\n## Next Steps\n\nRun the gate.");
    assert!(read.contains("Run the gate."), "{read}");
}

/// The two file headings older builds wrote are kept from the summarizer too.
///
/// The break this catches: matching only today's heading, so a session compacted by an older build
/// shows its whole list once more and copies it into the next summary.
#[test]
fn an_older_summarys_file_lists_are_kept_from_the_summarizer() {
    let read =
        earlier_summary("## Read Files\n\n- src/old.rs\n\n## Modified Files\n\n- src/older.rs");
    assert!(!read.contains("src/old"), "{read}");
}

/// The crash this cap used to be: slicing byte 1997 of a block whose 1997th byte is the middle
/// of a character panics, in the middle of a compaction the user never asked for.
#[test]
fn an_over_long_block_is_cut_on_a_character_not_a_byte() {
    let straddles = format!("{}\u{e9}{}", "a".repeat(1_996), "b".repeat(100));
    let serialized =
        serialize_for_summary(&[message_entry(Role::User, vec![ContentBlock::Text(straddles)])]);
    assert!(serialized.ends_with("...\n"), "the cut is marked: {}", &serialized[..40]);
    assert_eq!(serialized.chars().count(), "[User]: ".len() + SUMMARY_BLOCK_CHARS + 4);

    // And a block that fits is passed through whole, with no ellipsis invented for it.
    let exact = "x".repeat(SUMMARY_BLOCK_CHARS);
    let fits = serialize_for_summary(&[message_entry(Role::User, vec![ContentBlock::Text(exact)])]);
    assert!(!fits.contains("..."), "nothing was cut");
}

/// The whole shape, pinned: what the summarizer wrote, every heading it skipped, and one file
/// list — sorted, so two summaries of the same work read the same.
#[test]
fn a_summary_carries_the_prose_the_missing_headings_and_one_file_list() {
    let formatted = format_structured_summary(
        "## Goal\nBuild a web server.\n\n## Next Steps\nDeploy to production.",
        &files(["src/main.rs", "Cargo.toml"]),
    );

    assert_eq!(
        formatted,
        "## Goal\n\nBuild a web server.\n\n\
             ## Next Steps\n\nDeploy to production.\n\n\
             ## Constraints & Preferences\n\n\
             ## Progress\n\n\
             ## Key Decisions\n\n\
             ## Critical Context\n\n\
             ## Files\n\n- Cargo.toml\n- src/main.rs\n\n"
    );
}

/// A summary of empty sections is not a summary, whatever it looks like.
///
/// The break this catches, measured on a live loop with a local summarizer: the model returned
/// nothing usable, the formatter added the six required headings and the running file list
/// anyway, and a 20,677-token transcript was replaced by that — reported as a successful
/// compaction. The session lost everything it knew and nothing said so.
#[test]
fn a_summary_of_empty_headings_does_not_get_to_stand_for_a_transcript() {
    let empty = format_structured_summary("", &files(["wordfreq.py", "test_wordfreq.py"]));
    assert!(empty.contains("## Goal"), "it still looks like a summary: {empty}");
    assert!(
        !summary_carries_content(&empty),
        "...and that is exactly why looking is not enough: {empty}"
    );

    let headings_only = format_structured_summary(
        "## Goal\n\n## Progress\n\n## Next Steps\n",
        &files(["wordfreq.py"]),
    );
    assert!(!summary_carries_content(&headings_only), "{headings_only}");

    let real = format_structured_summary(
        "## Goal\nBuild wordfreq, a standard-library word counter with a small argparse CLI.\n\n\
             ## Progress\nPhases 0 and 1 are done and the suite is green at 20 tests.\n",
        &files(["wordfreq.py"]),
    );
    assert!(summary_carries_content(&real), "{real}");
}

/// A long file list is structure, not content, and cannot carry an empty summary past the check.
#[test]
fn a_file_list_is_not_a_summary() {
    let listed = format_structured_summary(
        "",
        &files([
            "src/one/very/long/path/to/a/file.rs",
            "src/another/quite/long/path/second.rs",
            "src/a/third/long/path/third.rs",
            "src/a/fourth/long/path/fourth.rs",
        ]),
    );
    assert!(!summary_carries_content(&listed), "{listed}");
}

/// Prose the summarizer wrote before any heading is not a section, and must not be silently
/// swallowed into one — or filed under a heading it does not belong to.
#[test]
fn a_preamble_before_the_first_heading_is_dropped_rather_than_misfiled() {
    let formatted =
        format_structured_summary("Here is my summary!\n\n## Goal\nShip it.", &HashSet::new());
    assert!(!formatted.contains("Here is my summary"), "{formatted}");
    assert!(formatted.starts_with("## Goal\n\nShip it.\n"), "{formatted}");
    assert!(formatted.ends_with("## Files\n\n"), "an empty list still asks: {formatted}");
}

/// A summarizer that writes its own file list must not leave the summary carrying two that
/// disagree — the running record is the one the transcript can prove.
#[test]
fn the_running_file_list_replaces_one_the_summarizer_invented() {
    let formatted = format_structured_summary(
        "## Goal\nShip it.\n\n## Files\n- imaginary.rs\n\n## Read Files\n- also-imaginary.rs",
        &files(["src/real.rs", "src/also-real.rs"]),
    );
    assert!(!formatted.contains("imaginary.rs"), "{formatted}");
    assert_eq!(formatted.matches("## Files").count(), 1, "{formatted}");
    assert!(!formatted.contains("## Read Files"), "the retired heading is gone: {formatted}");
    assert!(formatted.contains("- src/real.rs"), "{formatted}");
}

/// The defect the file list existed to have and did not: it only ever repeated what an earlier
/// summary said, so it started empty and stayed that way. The transcript is what makes it grow.
#[test]
fn the_file_list_comes_from_the_calls_that_named_the_files() {
    let call = |tool: &str, path: &str| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: EntryId::new().to_string(),
                name: tool.to_string(),
                arguments: serde_json::json!({ "path": path }),
            }],
        }),
    };
    let touched = files_touched(&[
        call("read", "src/lib.rs"),
        call("edit", "src/main.rs"),
        call("write", "README.md"),
        // A search scope is not a file this session touched.
        call("grep", "crates/"),
        call("read", "   "),
    ]);

    let mut expected = files(["src/lib.rs", "src/main.rs"]);
    expected.insert("README.md".to_string());
    assert_eq!(touched, expected);
}

/// A summary is read back the way it was written, and the headings it used to be written under
/// still count — a session compacted before the rename must not lose its record on resume.
#[test]
fn a_file_list_survives_the_round_trip_and_the_rename() {
    let written = format_structured_summary("## Goal\nShip it.", &files(["a/b.rs", "c.toml"]));
    assert_eq!(files_listed_in(&written), files(["a/b.rs", "c.toml"]));

    let legacy = "## Progress\n\n- Rewrote the parser. See notes.md for why.\n\n\
                      ## Read Files\n\n- src/old.rs\n\n## Modified Files\n\n- src/older.rs\n";
    assert_eq!(files_listed_in(legacy), files(["src/old.rs", "src/older.rs"]));
}

/// A bullet in `Progress` is a sentence, not a filename. Parsed over the whole document — as it
/// was — every one of them that contained a dot was filed as a file, forever.
#[test]
fn prose_outside_the_file_section_is_never_mistaken_for_a_file() {
    let summary = "## Progress\n\n- Fixed the cache in compaction.rs and shipped it.\n\n\
                      ## Files\n\n- crates/luxe-core/src/compaction.rs\n";
    assert_eq!(
        files_listed_in(summary),
        ["crates/luxe-core/src/compaction.rs".to_string()].into_iter().collect::<HashSet<_>>()
    );
}
