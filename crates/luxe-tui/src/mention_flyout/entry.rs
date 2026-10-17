//! Mention entries — the words a candidate is found by, and the candidate each entry becomes.

use super::Entry;
use crate::ref_provider::Candidate;

/// What a query is matched against for a row: its label, a space, then whatever else should find
/// it — the layout [`entry_candidate`] relies on to light the right characters.
pub(super) fn entry_words(entry: &Entry) -> String {
    match entry {
        Entry::Skill(skill) => format!("{} {}", skill.name, skill.description),
        Entry::GitHub(item) => format!("#{} {} {}", item.number, item.title, item.repository),
        // A tool's name finds its server: `tasks` turns up the one with `get_tasks`
        Entry::Mcp(server) => format!("{} {}", server.id, server.tools.join(" ")),
        Entry::Row(row) => format!("{} {}", row.label, row.detail.as_deref().unwrap_or_default()),
    }
}

/// A row as the picker lists it, lit where `positions` — into [`entry_words`] — matched.
///
/// A match in the label lights the label. One past it lands in the detail where the detail shows
/// that text: a skill's description, a source's detail, the repository a GitHub row names last.
/// A server found through one of its tools says which, since the id it lists by is not what matched.
pub(super) fn entry_candidate(entry: &Entry, positions: &[usize]) -> Candidate {
    let label_matched = |label: &str| -> Vec<usize> {
        let count = label.chars().count();
        positions.iter().copied().filter(|&at| at < count).collect()
    };
    // Positions past the label and its space, relative to the text after it
    let tail = |label: &str| -> Vec<usize> {
        let from = label.chars().count() + 1;
        positions.iter().filter_map(|&at| at.checked_sub(from)).collect()
    };
    match entry {
        Entry::Skill(skill) => Candidate {
            label: skill.name.clone(),
            insert: skill.name.clone(),
            detail: (!skill.description.is_empty()).then(|| skill.description.clone()),
            label_matched: label_matched(&skill.name),
            detail_matched: tail(&skill.name),
            ..Default::default()
        },
        // `#177 Title`, then what it is and where
        Entry::GitHub(item) => {
            let kind = if item.pull_request { "pull request" } else { "issue" };
            let draft = if item.draft { " · draft" } else { "" };
            let label = format!("#{} {}", item.number, item.title);
            let detail = format!("{kind} · {}{draft} · {}", item.state.word(), item.repository);
            let repository_from = detail.chars().count() - item.repository.chars().count();
            Candidate {
                insert: crate::mentions::github_token(item),
                label_matched: label_matched(&label),
                detail_matched: tail(&label).into_iter().map(|at| at + repository_from).collect(),
                detail: Some(detail),
                label,
                ..Default::default()
            }
        }
        Entry::Mcp(server) => {
            let (detail, detail_matched) = match matched_tool(server, &tail(&server.id)) {
                Some((tool, within)) => (format!("{tool} · {}", server_detail(server)), within),
                None => (server_detail(server), Vec::new()),
            };
            Candidate {
                label: server.id.clone(),
                insert: server.id.clone(),
                label_matched: label_matched(&server.id),
                detail: Some(detail),
                detail_matched,
                ..Default::default()
            }
        }
        Entry::Row(row) => Candidate {
            label: row.label.clone(),
            insert: row.id.clone(),
            detail: row.detail.clone(),
            label_matched: label_matched(&row.label),
            detail_matched: tail(&row.label),
            ..Default::default()
        },
    }
}

/// The tool whose name the first of `positions` — into the server's space-joined tool names —
/// falls in, with the positions inside it, relative to it.
fn matched_tool<'a>(
    server: &'a luxe_core::McpServerStatus,
    positions: &[usize],
) -> Option<(&'a str, Vec<usize>)> {
    let first = *positions.first()?;
    let mut from = 0;
    for tool in &server.tools {
        let count = tool.chars().count();
        if (from..from + count).contains(&first) {
            let within =
                positions.iter().filter_map(|&at| at.checked_sub(from)).filter(|&at| at < count);
            return Some((tool.as_str(), within.collect()));
        }
        from += count + 1;
    }
    None
}

/// `12 tools · signed in`, or what stands between the reader and a server's tools.
///
/// Sign-in comes first: a server with no token answers nothing, and the command that fixes it is
/// the useful thing to say. "Signed in" means a token is stored, not that it still works.
fn server_detail(server: &luxe_core::McpServerStatus) -> String {
    if server.signed_in == Some(false) {
        return format!("not signed in — luxe mcp login --id {}", server.id);
    }
    let tools = match (server.tool_count, server.tools.len()) {
        (None, _) => "tools not discovered yet".to_string(),
        (Some(_), 1) => "1 tool".to_string(),
        (Some(_), count) => format!("{count} tools"),
    };
    match server.signed_in {
        Some(true) => format!("{tools} · signed in"),
        _ => tools,
    }
}
