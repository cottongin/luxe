//! The drafting turn's rule, its brief and its answer.

use super::*;

/// The draft is what is inside the fence, and nothing around it; no fence, or nothing in it,
/// is said as such.
#[test]
fn the_draft_is_what_the_fence_holds() {
    let answer = "Here it is.\n\n```luxe-draft\nFixed in `4c1e2a0`.\n\nThe router reads it.\n```\n\nAnything else?";
    assert_eq!(extract_draft(answer).unwrap(), "Fixed in `4c1e2a0`.\n\nThe router reads it.");
    assert_eq!(extract_draft("```luxe-draft\nCut off").unwrap(), "Cut off");
    assert!(extract_draft("I'd say: fixed.").is_err());
    assert!(extract_draft("```luxe-draft\n\n```").is_err());
}

fn comment(id: &str, author: &str, body: &str, mine: bool) -> ConversationEntry {
    ConversationEntry::Comment(ConversationComment {
        id: id.into(),
        author: author.into(),
        bot: author.ends_with("[bot]"),
        mine,
        body: body.into(),
        created_at: String::new(),
        url: String::new(),
    })
}

fn request(conversation: Vec<ConversationEntry>) -> DraftRequest {
    DraftRequest {
        request_id: "draft:1".into(),
        repository: None,
        number: 142,
        pull_request: true,
        target: DraftTarget::Comment(ConversationComment {
            id: "c-jon".into(),
            author: "jon".into(),
            bot: false,
            mine: false,
            body: "Why is the fallback decided twice?".into(),
            created_at: String::new(),
            url: String::new(),
        }),
        conversation,
        finding: None,
        note: None,
        current: None,
    }
}

/// The first draft carries the conversation; a second on the same pull request carries only
/// what is new; forgetting — compaction, a session load — sends it whole again. A bot's wall
/// becomes its title, and the comment answered is never in its own digest.
#[tokio::test]
async fn the_digest_is_sent_once_and_then_only_what_is_new() {
    let mut sent = SentDigests::default();
    let mut conversation = vec![
        comment("c-mara", "mara", "Reads well.", false),
        comment("c-bot", "coderabbitai[bot]", "## Walkthrough\n\nA very long wall.", false),
        comment("c-jon", "jon", "Why is the fallback decided twice?", false),
        comment("c-me", "nora", "Thanks — kept it small on purpose.", true),
    ];
    let (first, sources) = brief(&request(conversation.clone()), None, &mut sent).await;
    assert!(first.contains("@mara: Reads well."));
    assert!(first.contains("@coderabbitai (a bot): Walkthrough"));
    assert!(!first.contains("A very long wall"));
    assert_eq!(first.matches("Why is the fallback decided twice?").count(), 1, "quoted once");
    assert!(first.contains("> Thanks — kept it small on purpose."), "my own voice");
    assert!(first.contains("```luxe-draft"));
    assert_eq!(sources, ["this session", "the conversation"]);
    assert_eq!(crate::briefs::label(&first), Some("Drafted a reply"), "the transcript knows it");

    conversation.push(comment("c-li", "li", "Agree with jon.", false));
    let (second, _) = brief(&request(conversation.clone()), None, &mut sent).await;
    assert!(second.contains("@li: Agree with jon."));
    assert!(!second.contains("@mara: Reads well."), "already sent");

    sent.forget();
    let (third, _) = brief(&request(conversation), None, &mut sent).await;
    assert!(third.contains("@mara: Reads well."), "sent whole again");
}

/// A redraft carries the reader's edited text and what they want different; a verified fix
/// says so, and an unverified one says it is not.
#[tokio::test]
async fn a_redraft_keeps_the_readers_edits_and_says_what_was_verified() {
    let mut sent = SentDigests::default();
    let mut redraft = request(Vec::new());
    redraft.current = Some("Fixed — see 4c1e2a0.".into());
    redraft.note = Some("Shorter.".into());
    redraft.finding = Some(FindingEvidence {
        verified: false,
        remediation: Some("Removed the second fallback.".into()),
    });
    let (message, sources) = brief(&redraft, None, &mut sent).await;
    assert!(message.contains("keep my changes\n\nFixed — see 4c1e2a0."));
    assert!(message.contains("What I want different\n\nShorter."));
    assert!(message.contains("a comment in my review — not verified yet"));
    assert!(message.contains("The agent reported: Removed the second fallback."));
    assert!(sources.contains(&"the review comment".to_string()));
}

/// A fence closed on the draft's own last line still ends the draft there.
#[test]
fn a_fence_closed_on_the_last_line_ends_the_draft() {
    assert_eq!(extract_draft("```luxe-draft\nFixed.```").unwrap(), "Fixed.");
    assert_eq!(extract_draft("```luxe-draft\nFixed.\n```\nThanks!").unwrap(), "Fixed.");
}

/// A digest line reads the first line worth reading: past a quote, without a heading's marks.
#[test]
fn a_digest_line_skips_quotes_and_heading_marks() {
    assert_eq!(
        first_words("> @jon: why?\n\nBecause it was decided twice."),
        "Because it was decided twice."
    );
    assert_eq!(first_words("## Walkthrough\n\nLong."), "Walkthrough");
    let long = "x".repeat(DIGEST_LINE_CHARS + 10);
    let cut = first_words(&long);
    assert_eq!(cut.chars().count(), DIGEST_LINE_CHARS + 1);
    assert!(cut.ends_with('…'));
}

/// The digest stops at its budget and says how much it left out; what it left out was never
/// sent, so the next draft carries it.
#[tokio::test]
async fn the_digest_keeps_to_its_budget_and_sends_the_rest_later() {
    let mut sent = SentDigests::default();
    let line = "y".repeat(DIGEST_LINE_CHARS);
    let conversation: Vec<ConversationEntry> = (0..40)
        .map(|n| comment(&format!("c-{n}"), "mara", &format!("{n} {line}"), false))
        .collect();
    let (first, _) = brief(&request(conversation.clone()), None, &mut sent).await;
    let digest = first.split("## The rest of the conversation").nth(1).unwrap_or_default();
    let digest = digest.split("\n## ").next().unwrap_or_default();
    assert!(digest.len() <= MAX_DIGEST_CHARS + 400, "{}", digest.len());
    assert!(first.contains("more, left out to keep this short"));
    assert!(first.contains("@mara: 0 y"), "the oldest first");
    // Each line is 250 bytes, so 32 fill the budget exactly: the budget is inclusive
    assert!(first.contains("@mara: 31 y") && !first.contains("@mara: 32 y"));
    assert!(!first.contains("@mara: 39 y"));
    let (second, _) = brief(&request(conversation), None, &mut sent).await;
    assert!(second.contains("@mara: 39 y"), "what was left out comes next time");
    assert!(!second.contains("@mara: 0 y"), "what was sent is not sent again");
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git").args(args).current_dir(root).status().unwrap();
    assert!(status.success(), "git {args:?}");
}

/// On a thread, the brief quotes the code at its line in this checkout, the line marked, and
/// the commits since the thread opened that touch its file — and says it drew on both.
#[tokio::test]
async fn a_thread_brief_carries_the_code_now_and_the_commits_since() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "--quiet", "-b", "main"]);
    git(root, &["config", "user.name", "Luxe Tests"]);
    git(root, &["config", "user.email", "tests@luxe.local"]);
    let lines: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
    std::fs::write(root.join("router.rs"), lines.join("\n")).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "Decide the fallback once"]);
    let thread = ReviewThread {
        id: "PRRT_1".into(),
        path: "router.rs".into(),
        line: Some(10),
        start_line: None,
        side: crate::github::ThreadSide::Right,
        outdated: false,
        resolved: false,
        file_level: false,
        comments: vec![crate::github::ThreadComment {
            author: "jon".into(),
            body: "Why twice?".into(),
            created_at: "2000-01-01T00:00:00Z".into(),
            url: String::new(),
        }],
        total_comments: 1,
        anchor_text: None,
        can_reply: true,
        can_resolve: true,
    };
    let mut asked = request(Vec::new());
    asked.target = DraftTarget::Thread(thread);
    let (message, sources) = brief(&asked, Some(root), &mut SentDigests::default()).await;
    assert!(message.contains("   10> line 10"), "{message}");
    assert!(message.contains("    6  line 6") && message.contains("   14  line 14"));
    assert!(!message.contains("line 15") && !message.contains("line 5\n"));
    assert!(message.contains("Decide the fallback once"));
    assert!(sources.contains(&"the code there now".to_string()), "{sources:?}");
    assert_eq!(sources.len(), 4, "this session, the thread, the code, one commit: {sources:?}");
    assert!(!sources[3].contains(' '), "one commit is named by its short id: {sources:?}");

    // A thread opened after every commit has none since; one on a missing file has no code
    let mut later = asked.clone();
    if let DraftTarget::Thread(thread) = &mut later.target {
        thread.comments[0].created_at = "2999-01-01T00:00:00Z".into();
        thread.path = "missing.rs".into();
    }
    let (message, sources) = brief(&later, Some(root), &mut SentDigests::default()).await;
    assert!(!message.contains("## Commits since") && !message.contains("## The code at"));
    assert_eq!(sources, ["this session", "the thread"]);
}

/// A review's summary is briefed with its verdict, the line comments that post with it, and the
/// guide's reading — told not to repeat the comments — and nothing in the conversation is left out
/// as "the one answered".
#[tokio::test]
async fn a_review_summary_is_briefed_with_its_verdict_comments_and_guide() {
    let mut sent = SentDigests::default();
    let mut review = request(vec![comment("c-mara", "mara", "Reads well.", false)]);
    review.target = DraftTarget::Review(ReviewDraft {
        verdict: "Request changes".into(),
        comments: vec!["src/router.rs:88 — the fallback is decided twice".into()],
        guide: Some("Routes digests by category, and moves the fallback into the router.".into()),
    });
    let (message, sources) = brief(&review, None, &mut sent).await;
    assert!(message.contains("summary of my review of pull request #142"), "{message}");
    assert_eq!(crate::briefs::label(&message), Some("Drafted the review summary"));
    assert!(message.contains("post as \"Request changes\""));
    assert!(message.contains("- src/router.rs:88 — the fallback is decided twice"));
    assert!(message.contains("do not repeat them"));
    assert!(message.contains("moves the fallback into the router"));
    assert!(message.contains("@mara: Reads well."), "the conversation, whole");
    assert!(message.contains("```luxe-draft"));
    assert_eq!(sources, ["this session", "your comment", "the guide", "the conversation"]);

    let mut bare = request(Vec::new());
    bare.target = DraftTarget::Review(ReviewDraft {
        verdict: "Approve".into(),
        comments: Vec::new(),
        guide: None,
    });
    let (message, sources) = brief(&bare, None, &mut SentDigests::default()).await;
    assert!(message.contains("None: the summary is the whole review."));
    assert_eq!(sources, ["this session"]);
}
