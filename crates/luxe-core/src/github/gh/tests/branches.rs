//! Branches — a branch's pull request, its stack and its search results.

use super::*;

pub(super) const STATUS_OPEN: &str = include_str!("../../fixtures/branch_status_open.json");
pub(super) const STATUS_NONE: &str = include_str!("../../fixtures/branch_status_none.json");
pub(super) const STATUS_FAILING: &str = include_str!("../../fixtures/branch_status_failing.json");
pub(super) const STATUS_NO_CHECKS: &str =
    include_str!("../../fixtures/branch_status_no_checks.json");
pub(super) const STATUS_FORK: &str = include_str!("../../fixtures/branch_status_fork_threads.json");
pub(super) const STATUS_STACKED: &str = include_str!("../../fixtures/branch_status_stacked.json");
pub(super) const MERGED_SQUASHED: &str = include_str!("../../fixtures/merged_below_squashed.json");
pub(super) const SEARCH_MINE: &str = include_str!("../../fixtures/search_mine.json");
pub(super) const SEARCH_ISSUES: &str = include_str!("../../fixtures/search_issues.json");

/// Real searches: this repository's pull requests by the reader, and cli/cli's bugs, of any
/// state and newest first.
#[test]
fn a_search_lists_pull_requests_and_issues_with_where_they_are() {
    let mine = parse_search(SEARCH_MINE).unwrap().items;
    assert_eq!(mine.len(), 30);
    assert!(mine.iter().all(|item| item.pull_request && item.repository == "nora-quinn/luxe-dev"));
    assert_eq!(mine[0].number, 177);
    assert!(mine[0].url.ends_with("/pull/177") && mine[0].author == "nora-quinn");
    assert!(!mine[0].updated_at.is_empty());
    let states: Vec<(u64, PullRequestState, bool)> =
        mine[..3].iter().map(|item| (item.number, item.state, item.draft)).collect();
    assert_eq!(
        states,
        [
            (177, PullRequestState::Open, false),
            (179, PullRequestState::Closed, true),
            (178, PullRequestState::Merged, false),
        ]
    );
    let issues = parse_search(SEARCH_ISSUES).unwrap().items;
    assert!(issues.iter().all(|item| !item.pull_request && item.repository == "cli/cli"));
    assert_eq!(issues[0].number, 14495);
    assert_eq!(
        (issues[0].state, issues[2].state),
        (PullRequestState::Closed, PullRequestState::Open)
    );
    let deleted =
        SEARCH_ISSUES.replacen("\"author\":{\"login\":", "\"author\":null,\"x\":{\"login\":", 1);
    assert_eq!(parse_search(&deleted).unwrap().items[0].author, "", "a deleted account");
    // A frame from a luxe that sent no state reads as open, not as an error
    let older = r#"{"pull_request":true,"number":1,"title":"T","url":"u","repository":"a/b","author":"","draft":false,"updated_at":""}"#;
    let item: ListItem = serde_json::from_str(older).unwrap();
    assert_eq!(item.state, PullRequestState::Open);
}

pub(super) const MERGED_NONE: &str = include_str!("../../fixtures/merged_below_none.json");

/// Real answers from restacking a scratch stack: the bottom squash-merged with its
/// branch kept, and a layer with nothing merged below it. A retargeted one names the base it
/// was moved from.
#[test]
fn what_merged_below_a_stack_is_read_with_where_it_went() {
    let (merged, previous) = parse_merged_below(MERGED_SQUASHED).unwrap();
    let merged = merged.expect("#158 merged");
    assert_eq!(merged.number, 158);
    assert_eq!(merged.head_oid, "e934880d96cdfd85a9b9771a086c152d84ab142f");
    assert_eq!(merged.base_ref, "scratch/stack-trunk");
    assert_eq!(previous, None, "never retargeted");
    assert_eq!(parse_merged_below(MERGED_NONE).unwrap(), (None, None));
    let retargeted = MERGED_NONE.replace(
        "\"timelineItems\":{\"nodes\":[]}",
        "\"timelineItems\":{\"nodes\":[{\"previousRefName\":\"scratch/stack-1\"}]}",
    );
    assert_eq!(parse_merged_below(&retargeted).unwrap().1.as_deref(), Some("scratch/stack-1"));
}

pub(super) const STATUS_UNSTACKED: &str =
    include_str!("../../fixtures/branch_status_unstacked.json");
/// A real chain of luxe's own, read from the middle: seven below down to `master`, two above,
/// and the evidence that checks wait for trunk — the bottom one has them, this one none.
#[test]
fn a_pull_request_in_the_middle_of_a_stack_knows_where_it_sits() {
    let status =
        parse_branch_status(STATUS_STACKED, "feat/session-worktrees", "nora-quinn").unwrap();
    let pr = status.pull_request.unwrap();
    assert_eq!(pr.number, 149);
    let stack = pr.stack.expect("in a stack");
    let numbers = |entries: &[StackEntry]| entries.iter().map(|e| e.number).collect::<Vec<_>>();
    assert_eq!(numbers(&stack.below), [138, 139, 140, 143, 145, 147, 148], "bottom first");
    assert_eq!(numbers(&stack.above), [153, 156], "nearest first");
    assert_eq!((stack.position(), stack.height()), (8, 10));
    assert_eq!(stack.trunk, "master");
    assert!(stack.trunk_is_default);
    assert!(stack.below[0].checked && !stack.below[1].checked);
    assert!(stack.checks_wait_for_trunk(&pr.checks), "none here, and #138's ran");
    assert!(!stack.checks_wait_for_trunk(&CheckRollup { passed: 1, ..Default::default() }));
}

/// A pull request into trunk with nothing stacked on it is in no stack, and is not listed as
/// part of anyone else's.
#[test]
fn a_pull_request_on_its_own_is_in_no_stack() {
    let status =
        parse_branch_status(STATUS_UNSTACKED, "polish/composer-text-navigation", "nora-quinn")
            .unwrap();
    assert_eq!(status.pull_request.unwrap().stack, None);
}

/// The walk follows only this repository's branches, stops at a base with no pull request of
/// its own (which is then not trunk), and a loop in the data ends it rather than hanging it.
#[test]
fn the_stack_walk_stops_at_forks_plain_branches_and_loops() {
    let node = |number: u64, head: &str, base: &str, owner: &str| StackNode {
        number,
        title: format!("#{number}"),
        is_draft: false,
        head_ref_name: head.into(),
        base_ref_name: base.into(),
        head_repository_owner: Some(Login { login: owner.into() }),
        commits: Nodes { nodes: Vec::new() },
    };
    let open = [
        node(2, "one", "shared-base", "acme"),
        node(3, "fork-branch", "two", "mara"),
        node(4, "loop-a", "loop-b", "acme"),
        node(5, "loop-b", "loop-a", "acme"),
    ];
    let stack = stack_of(9, "two", "one", &open, "acme", Some("main")).unwrap();
    assert_eq!(stack.below.iter().map(|e| e.number).collect::<Vec<_>>(), [2]);
    assert!(stack.above.is_empty(), "a fork's branch is no base");
    assert_eq!(stack.trunk, "shared-base");
    assert!(!stack.trunk_is_default && !stack.checks_wait_for_trunk(&CheckRollup::default()));

    let looped = stack_of(9, "x", "loop-a", &open, "acme", Some("main")).unwrap();
    assert_eq!(looped.below.iter().map(|e| e.number).collect::<Vec<_>>(), [5, 4]);
}
