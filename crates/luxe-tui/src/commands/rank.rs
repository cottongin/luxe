//! Command ranking — the order the palette and the slash menu list matches in.

use super::Command;

/// Ranks `commands` against `query`: fuzzy on each [`Command::haystack`], by keyword prefix
/// otherwise, best first, ties in the order given.
///
/// Returns each match's index into `commands` with the haystack positions that matched, empty for
/// a keyword match since nothing it matched is on screen. A blank query keeps the order given.
pub fn rank(query: &str, commands: &[&Command]) -> Vec<(usize, Vec<usize>)> {
    let query = query.trim();
    if query.is_empty() {
        return (0..commands.len()).map(|index| (index, Vec::new())).collect();
    }
    let haystacks: Vec<String> = commands.iter().map(|command| command.haystack()).collect();
    let haystacks: Vec<&str> = haystacks.iter().map(String::as_str).collect();
    let mut found: Vec<(usize, i64, Vec<usize>)> = crate::fuzzy::rank(query, &haystacks)
        .into_iter()
        .map(|matched| (matched.idx, matched.score, matched.positions))
        .collect();
    for (index, command) in commands.iter().enumerate() {
        if found.iter().any(|(kept, ..)| *kept == index) {
            continue;
        }
        if let Some(score) = command.keyword_score(query) {
            found.push((index, score, Vec::new()));
        }
    }
    found.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    found.into_iter().map(|(index, _, positions)| (index, positions)).collect()
}
