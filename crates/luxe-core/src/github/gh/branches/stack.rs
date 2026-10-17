//! Stack — the chain of pull requests a branch sits in, as GitHub's answer lays it out.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::github::gh) struct StackNode {
    pub(in crate::github::gh) number: u64,
    pub(in crate::github::gh) title: String,
    pub(in crate::github::gh) is_draft: bool,
    pub(in crate::github::gh) head_ref_name: String,
    pub(in crate::github::gh) base_ref_name: String,
    pub(in crate::github::gh) head_repository_owner: Option<Login>,
    pub(in crate::github::gh) commits: Nodes<StackCommitNode>,
}

#[derive(Deserialize)]
pub(in crate::github::gh) struct StackCommitNode {
    commit: StackCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::github::gh) struct StackCommit {
    status_check_rollup: Option<serde_json::Value>,
}

/// Where the pull request from `head` into `base` sits among the repository's `open` ones, or
/// `None` when nothing is stacked below or above it.
///
/// Only pull requests whose branch is in this repository count, since a fork's branch cannot be
/// another pull request's base. Down, each base is followed to the pull request that has it as
/// its branch, until one has none; up, each branch to the pull request based on it — the most
/// recently updated when several are — which a loop in the data cannot turn into an endless walk.
pub(in crate::github::gh) fn stack_of(
    number: u64,
    head: &str,
    base: &str,
    open: &[StackNode],
    owner: &str,
    default_branch: Option<&str>,
) -> Option<Stack> {
    let ours: Vec<&StackNode> = open
        .iter()
        .filter(|node| node.number != number)
        .filter(|node| {
            node.head_repository_owner
                .as_ref()
                .is_some_and(|login| login.login.eq_ignore_ascii_case(owner))
        })
        .collect();
    let entry = |node: &StackNode| StackEntry {
        number: node.number,
        title: status_text(&node.title),
        head_ref: node.head_ref_name.clone(),
        draft: node.is_draft,
        checked: node
            .commits
            .nodes
            .first()
            .is_some_and(|commit| commit.commit.status_check_rollup.is_some()),
    };
    let mut seen = std::collections::HashSet::from([number]);
    let mut below = Vec::new();
    let mut trunk = base.to_string();
    while let Some(node) = ours.iter().find(|node| node.head_ref_name == trunk) {
        if !seen.insert(node.number) {
            break;
        }
        below.push(entry(node));
        trunk = node.base_ref_name.clone();
    }
    below.reverse();
    let mut above = Vec::new();
    let mut branch = head.to_string();
    while let Some(node) = ours.iter().find(|node| node.base_ref_name == branch) {
        if !seen.insert(node.number) {
            break;
        }
        above.push(entry(node));
        branch = node.head_ref_name.clone();
    }
    if below.is_empty() && above.is_empty() {
        return None;
    }
    let trunk_is_default = default_branch == Some(trunk.as_str());
    Some(Stack { trunk, trunk_is_default, below, above })
}
