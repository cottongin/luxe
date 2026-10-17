use super::*;

/// The places a palette can open, from a fresh session to a pull request's review, so a property
/// that must hold everywhere is checked everywhere.
fn contexts() -> Vec<(&'static str, CommandContext)> {
    let idle = CommandContext::default();
    let busy = CommandContext {
        has_turn: true,
        has_image: true,
        messages_waiting: true,
        in_repository: true,
        ..CommandContext::default()
    };
    let changes = CommandContext { changes_pane: true, ..busy.clone() };
    let review = CommandContext { surface: Surface::Review, ..busy.clone() };
    let pull_request = CommandContext { pull_request_review: true, ..review.clone() };
    vec![
        ("a fresh session", idle),
        ("a conversation", busy),
        ("the Changes pane", changes),
        ("a review", review),
        ("a pull request's review", pull_request),
    ]
}

fn names(menu: &Menu) -> Vec<&str> {
    menu.entries.iter().map(|entry| entry.command.name.as_ref()).collect()
}

fn menu(context: &CommandContext, recent: &[&str]) -> Menu {
    let recent: Vec<String> = recent.iter().map(|id| id.to_string()).collect();
    CommandRegistry::all().arrange(context, &KeyMap::defaults(), &recent)
}

#[test]
fn ids_and_slash_triggers_are_unique() {
    let registry = CommandRegistry::all();
    let mut ids: Vec<&str> = registry.iter().map(|command| command.id.as_ref()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), registry.len(), "command ids must be unique");
    let mut slashes: Vec<&str> = registry.iter().filter_map(Command::slash_name).collect();
    let count = slashes.len();
    slashes.sort_unstable();
    slashes.dedup();
    assert_eq!(slashes.len(), count, "slash triggers must be unique");
}

/// Every slash command that shipped before still answers to its name, collapsed or not.
#[test]
fn every_slash_trigger_still_parses() {
    let registry = CommandRegistry::all();
    for name in [
        "/new",
        "/sessions",
        "/model",
        "/thinking",
        "/mode",
        "/review",
        "/theme",
        "/help",
        "/context",
        "/compact",
        "/cost",
        "/worktree",
        "/github",
        "/stack",
        "/pr",
        "/preview",
        "/diff",
        "/dock",
        "/statusline",
        "/mouse",
        "/permissions",
        "/lsp",
        "/debug",
        "/mcp",
        "/tools",
        "/images",
        "/login",
        "/logout",
        "/auth",
        "/config",
        "/quit",
        "/reload",
        "/search",
    ] {
        assert!(registry.parse_slash(name).is_some(), "missing slash trigger {name}");
    }
    assert!(registry.parse_slash("/new").is_some_and(|c| c.kind == CommandKind::Slash));
    assert_eq!(
        registry.parse_slash("/quit").map(|c| c.kind),
        Some(CommandKind::Action(UiAction::Quit)),
        "an action with a slash trigger runs as the action"
    );
}

/// `/peek` is what the preview was called, and it still opens it, arguments and all.
#[test]
fn a_renamed_trigger_still_works() {
    let registry = CommandRegistry::all();
    let old = registry.parse_slash("/peek src/app.rs").expect("the old trigger");
    assert_eq!(old.slash_name(), Some("/preview"));
    assert_eq!(old.args.as_deref(), Some("src/app.rs"));
}

#[test]
fn parse_slash_keeps_trimmed_arguments_and_only_at_the_start() {
    let registry = CommandRegistry::all();
    let diff = registry.parse_slash("/diff   base main  ").expect("known command");
    assert_eq!(diff.id, "cmd.diff");
    assert_eq!(diff.args.as_deref(), Some("base main"));
    assert_eq!(registry.parse_slash("/statusline").and_then(|c| c.args), None);
    assert!(registry.parse_slash("/unknown").is_none());
    assert!(registry.parse_slash("say /help").is_none());
}

/// A name is something you would say out loud: short, no punctuation standing in for a
/// description, and a summary that is one sentence.
#[test]
fn names_are_names_and_summaries_are_sentences() {
    for command in CommandRegistry::all().iter() {
        let name = command.name.as_ref();
        assert!(name.split_whitespace().count() <= 4, "{name:?} is more than four words");
        assert!(!name.contains(['(', ')', ':', ',', '<']), "{name:?} carries punctuation");
        let summary = command.summary.as_ref();
        assert!(summary.ends_with('.'), "{name}: the summary is a sentence: {summary:?}");
        assert!(summary.chars().count() <= 90, "{name}: the summary runs long: {summary:?}");
        assert!(!summary.contains(". "), "{name}: one sentence: {summary:?}");
        for (form, _) in &command.usage {
            let slash = command.slash_name().expect("only slash commands take arguments");
            assert!(form.starts_with(slash), "{name}: {form:?} is not a form of {slash}");
        }
    }
}

/// Two rows the reader has to choose between never share a name. The same words in contexts
/// that never list together are fine: that is the same act on two surfaces.
#[test]
fn names_are_unique_wherever_they_list() {
    for (place, context) in contexts() {
        let menu = menu(&context, &[]);
        let mut seen = names(&menu);
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), count, "a name repeats in {place}: {:?}", names(&menu));
    }
}

/// Keys that move around, editor mechanics, and commands about "the selected" thing are keys,
/// not commands; none of them may creep back into a list you search.
#[test]
fn keys_and_mechanics_are_not_commands() {
    let registry = CommandRegistry::all();
    let has = |action: UiAction| registry.iter().any(|command| command.keys == Some(action));
    for action in [
        UiAction::Submit,
        UiAction::NewLine,
        UiAction::Cancel,
        UiAction::OpenPalette,
        UiAction::PasteClipboard,
        UiAction::SelectAll,
        UiAction::CopyCode,
        UiAction::CopySelection,
        UiAction::EditLastMessage,
        UiAction::ScrollUp,
        UiAction::PageDown,
        UiAction::SelectBlockUp,
        UiAction::ShowDock,
        UiAction::HideDock,
        UiAction::ResetDockAuto,
        UiAction::ShowStatusline,
        UiAction::HideStatusline,
        UiAction::ResetStatuslineAuto,
        UiAction::ChangesFileNext,
        UiAction::ChangesFilePrev,
        UiAction::ReviewExit,
        UiAction::ReviewOpenSelection,
        UiAction::ReviewChapterNext,
        UiAction::ReviewFileNext,
        UiAction::ReviewHunkNext,
        UiAction::ReviewLineNext,
        UiAction::ReviewPagePrev,
        UiAction::ReviewScrollLeft,
        UiAction::ReviewLineStart,
        UiAction::ReviewEditFinding,
        UiAction::ReviewDeleteFinding,
        UiAction::ReviewResolveFinding,
        UiAction::ReviewReanchorFinding,
        UiAction::ReviewHelp,
        UiAction::DashboardQuit,
    ] {
        assert!(!has(action), "{} should not be a command", action.id());
    }
}

/// The complaint this list exists to answer: a fresh session lists nothing about a review, and a
/// review lists nothing that only means something in the conversation.
#[test]
fn a_menu_lists_only_what_can_run_there() {
    let fresh = menu(&CommandContext::default(), &[]);
    let listed = names(&fresh);
    for absent in
        ["Leave review", "Finish review", "Comment on this line", "Ask again", "View image"]
    {
        assert!(!listed.contains(&absent), "a fresh session lists {absent:?}");
    }
    for present in ["New session", "Change model", "Review changes", "Help", "Quit"] {
        assert!(listed.contains(&present), "a fresh session is missing {present:?}");
    }
    assert!(!listed.contains(&"Pull request"), "no checkout, no pull request");

    let review = CommandContext { surface: Surface::Review, ..CommandContext::default() };
    let review_menu = menu(&review, &[]);
    let listed = names(&review_menu);
    // A review draws no picker and no transcript over itself, so what opens one stays in the chat
    for absent in
        ["New session", "Review changes", "Session cost", "Open on GitHub", "Change model"]
    {
        assert!(!listed.contains(&absent), "a review lists {absent:?}");
    }
    for present in ["Leave review", "Finish review", "Side by side", "Help", "Quit"] {
        assert!(listed.contains(&present), "a review is missing {present:?}");
    }
    let every = CommandRegistry::all().len();
    assert!(fresh.entries.len() < every, "a fresh session lists a subset");
}

/// Unfiltered, a menu is predictable: alphabetical by name, with nothing out of place.
#[test]
fn an_unfiltered_menu_is_alphabetical() {
    for (place, context) in contexts() {
        let menu = menu(&context, &[]);
        assert_eq!(menu.recent, 0);
        let listed: Vec<String> = names(&menu).iter().map(|name| name.to_lowercase()).collect();
        let mut sorted = listed.clone();
        sorted.sort();
        assert_eq!(listed, sorted, "{place} is not alphabetical");
    }
}

/// What you just used leads, most recent first and at most five, then the alphabet resumes
/// without them. One that cannot run here is skipped, and the next recent one takes its place.
#[test]
fn recently_used_commands_lead_the_menu() {
    let recent = [
        "cmd.pr",
        "tui.review.finish",
        "cmd.model",
        "cmd.new",
        "cmd.cost",
        "tui.app.quit",
        "cmd.theme",
        "cmd.diff",
    ];
    let menu = menu(&CommandContext::default(), &recent);
    let listed = names(&menu);
    assert_eq!(menu.recent, RECENT_SHOWN);
    assert_eq!(
        &listed[..RECENT_SHOWN],
        ["Change model", "New session", "Session cost", "Quit", "Change theme"],
        "Pull request needs a checkout and Finish review a review, so both are skipped"
    );
    assert_eq!(listed.iter().filter(|name| **name == "Change model").count(), 1, "listed once");
    let rest: Vec<String> = listed[RECENT_SHOWN..].iter().map(|name| name.to_lowercase()).collect();
    let mut sorted = rest.clone();
    sorted.sort();
    assert_eq!(rest, sorted, "the rest stays alphabetical");
    assert!(rest.contains(&"review changes".to_string()), "a sixth recent is not lost");

    let none = self::menu(&CommandContext::default(), &["cmd.gone", "tui.review.finish"]);
    assert_eq!(none.recent, 0, "an id nobody knows, and one that cannot run, lead nothing");
}

/// The key shown beside a command is the one bound now, not the one that shipped.
#[test]
fn a_menu_shows_the_key_bound_now() {
    let chord_of = |menu: &Menu, name: &str| {
        menu.entries.iter().find(|entry| entry.command.name == name).and_then(|e| e.chord.clone())
    };
    let context = CommandContext::default();
    let shipped = CommandRegistry::all().arrange(&context, &KeyMap::defaults(), &[]);
    assert_eq!(chord_of(&shipped, "Review changes").as_deref(), Some("ctrl+g"));
    assert_eq!(chord_of(&shipped, "Context Dock").as_deref(), Some("alt+d"));
    assert_eq!(chord_of(&shipped, "Help"), None, "help ships unbound");
    let rebound = KeyMap::defaults().with_overrides(std::collections::HashMap::from([(
        "tui.app.toggle_help".to_string(),
        vec!["f1".to_string()],
    )]));
    let menu = CommandRegistry::all().arrange(&context, &rebound, &[]);
    assert_eq!(chord_of(&menu, "Help").as_deref(), Some("f1"));
}

#[test]
fn filtering_a_menu_keeps_its_recent_lead_honest() {
    let menu = menu(&CommandContext::default(), &["cmd.new", "tui.app.quit", "cmd.model"]);
    assert_eq!(menu.recent, 3);
    let slash_only = menu.filtered(|command| command.id != "tui.app.quit");
    assert_eq!(slash_only.recent, 2);
    assert_eq!(names(&slash_only)[..2], ["New session", "Change model"]);
}

/// The first list to claim an id or a slash trigger keeps it, so nothing added after the
/// built-ins can take over a name Luxe already answers to.
#[test]
fn a_later_source_cannot_take_a_built_in_id_or_trigger() {
    let impostor = |id: &'static str, slash: &'static str| {
        Command::slash(id, "Impostor", slash).summary("Pretends.")
    };
    let registry = CommandRegistry::from_sources([
        built_in(),
        vec![impostor("cmd.new", "/fresh"), impostor("my.new", "/new"), impostor("my.x", "/x")],
    ]);
    assert_eq!(registry.len(), built_in().len() + 1, "only the command with a free name joins");
    assert_eq!(registry.parse_slash("/new").map(|c| c.name), Some("New session".into()));
    assert!(registry.parse_slash("/x").is_some());
    assert!(!registry.is_empty());
    let none = CommandRegistry::from_sources(Vec::<Vec<Command>>::new());
    assert!(none.is_empty() && none.iter().next().is_none(), "no sources, no commands");
}

#[test]
fn the_haystack_is_the_name_and_trigger_and_never_the_summary() {
    let registry = CommandRegistry::all();
    let credentials = registry.iter().find(|c| c.id == "cmd.auth").expect("present");
    assert_eq!(credentials.haystack(), "Credentials auth");
    let swap = registry.iter().find(|c| c.id == "tui.workspace.toggle_dock_position").unwrap();
    assert_eq!(swap.haystack(), "Swap the sidebar's side", "no trigger, just the name");
}

fn ranked(query: &str) -> Vec<String> {
    let menu = menu(&CommandContext::default(), &[]);
    let commands: Vec<&Command> = menu.entries.iter().map(|entry| &entry.command).collect();
    rank(query, &commands).into_iter().map(|(index, _)| commands[index].name.to_string()).collect()
}

/// A command found by what it is called outranks one found by what it is about: `/diff` is
/// Review changes, and Preview only knows the word.
#[test]
fn a_name_or_trigger_outranks_a_keyword() {
    let found = ranked("diff");
    assert_eq!(found[..2], ["Review changes", "Preview"], "{found:?}");
    assert_eq!(ranked("exit").first().map(String::as_str), Some("Quit"));
    assert_eq!(ranked("keys").first().map(String::as_str), Some("Help"), "the old name finds it");
    assert_eq!(ranked("api").first().map(String::as_str), Some("Credentials"), "and API keys");
}

/// Keywords are vocabulary: they match from the start of one of their words, never scattered.
#[test]
fn a_keyword_matches_from_the_start_of_a_word() {
    let registry = CommandRegistry::all();
    let compact = registry.parse_slash("/compact").expect("present");
    assert!(compact.keyword_score("shri").is_some(), "the start of a word");
    assert!(compact.keyword_score("hunk").is_none(), "not the letters of \"shrink\"");
    assert!(compact.keyword_score("rink").is_none(), "not the middle of a word");
    let permissions = registry.parse_slash("/permissions").expect("present");
    assert!(permissions.keyword_score("walk aw").is_some(), "a phrase from its start");
    assert!(permissions.keyword_score("away").is_some(), "or any word in it");
    assert!(ranked("hunk").is_empty(), "nothing in a conversation is about hunks");
    assert!(ranked("   ").len() > 20, "a blank query keeps everything");
}

#[test]
fn needs_describe_what_is_missing() {
    let fresh = CommandContext::default();
    let pr = CommandRegistry::all().parse_slash("/pr").expect("present");
    assert_eq!(pr.unmet(&fresh), Some(Need::Repository));
    assert_eq!(Need::Repository.describe(), "a git checkout");
    let in_checkout = CommandContext { in_repository: true, ..fresh.clone() };
    assert!(pr.available(&in_checkout));
    let review = CommandContext { surface: Surface::Review, in_repository: true, ..fresh };
    assert_eq!(pr.unmet(&review), Some(Need::Chat), "the first unmet need is the one named");
}

/// What one launch ran leads the next one's menu; the default holder never writes anywhere, which
/// is what keeps a test's commands out of the reader's state file.
#[test]
fn recent_commands_survive_a_relaunch_and_the_default_stays_in_memory() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("state.json");
    let mut first = RecentCommands::persisted(path.clone());
    assert!(first.ids().is_empty());
    first.record("cmd.cost");
    first.record("cmd.new");
    first.record("cmd.cost");
    assert_eq!(first.ids(), ["cmd.cost", "cmd.new"]);
    assert_eq!(RecentCommands::persisted(path.clone()).ids(), ["cmd.cost", "cmd.new"]);

    let mut transient = RecentCommands::default();
    transient.record("cmd.model");
    assert_eq!(transient.ids(), ["cmd.model"]);
    assert_eq!(RecentCommands::persisted(path).ids(), ["cmd.cost", "cmd.new"], "untouched");
}
