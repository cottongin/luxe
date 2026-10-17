// fuzzy — thin wrapper over nucleo-matcher for the pickers' ranking.

use nucleo_matcher::{Config, Matcher, Utf32Str};

/// A ranked match: index into the original list, score (higher = better), and the *char* positions
/// in the item text that contributed to the match.
#[derive(Clone, Debug)]
pub(crate) struct Match {
    pub(crate) idx: usize,
    pub(crate) score: i64,
    pub(crate) positions: Vec<usize>,
}

/// Rank `items` against `query`, returning matches sorted by score descending.
///
/// Empty query returns all items in their original order with a default score of 0.
pub(crate) fn rank(query: &str, items: &[&str]) -> Vec<Match> {
    rank_with(Config::DEFAULT, query, items)
}

/// [`rank`] for file paths: `/` counts as a word boundary, so a match that starts a folder or the
/// filename scores the way one starting a word does. The matcher Helix's file picker uses, set the
/// way it sets it.
pub(crate) fn rank_paths(query: &str, items: &[&str]) -> Vec<Match> {
    rank_with(Config::DEFAULT.match_paths(), query, items)
}

fn rank_with(config: Config, query: &str, items: &[&str]) -> Vec<Match> {
    if query.is_empty() {
        return items
            .iter()
            .enumerate()
            .map(|(i, item)| Match { idx: i, score: 0, positions: (0..item.len()).collect() })
            .collect();
    }

    let mut matcher = Matcher::new(config);
    let mut haystack_buf = Vec::new();
    let mut needle_buf = Vec::new();
    // The default config ignores case by folding the haystack, and expects the needle folded
    // already: an unfolded capital failed its prefilter and panicked, so typing `@Re` to find a
    // README took the whole TUI down
    let query = query.to_lowercase();
    let needle = Utf32Str::new(&query, &mut needle_buf);

    let mut matches: Vec<Match> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            haystack_buf.clear();
            let haystack = Utf32Str::new(item, &mut haystack_buf);
            let mut positions = Vec::new();
            let score = matcher
                .fuzzy_indices(haystack, needle, &mut positions)
                .map(|s| s as i64)
                .unwrap_or(-1);
            if score > 0 {
                Some(Match {
                    idx: i,
                    score,
                    positions: positions.into_iter().map(|p| p as usize).collect(),
                })
            } else {
                None
            }
        })
        .collect();

    matches.sort_by_key(|b| std::cmp::Reverse(b.score));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capital in the query matches as its lowercase does, and never panics the matcher.
    #[test]
    fn a_capital_in_the_query_matches_and_does_not_panic() {
        let items = ["README.md", "#13 Route by category", "src/readme_parser.rs"];
        for query in ["Re", "README", "Ro", "rOuTe"] {
            let upper: Vec<usize> = rank(query, &items).iter().map(|found| found.idx).collect();
            let lower: Vec<usize> =
                rank(&query.to_lowercase(), &items).iter().map(|found| found.idx).collect();
            assert_eq!(upper, lower, "{query}");
            assert!(!upper.is_empty(), "{query}");
        }
    }

    #[test]
    fn empty_query_returns_all() {
        let items = vec!["abc", "def", "ghi"];
        let matches = rank("", &items);
        assert_eq!(matches.len(), 3);
        // Original order preserved, score 0
        for (i, m) in matches.iter().enumerate() {
            assert_eq!(m.idx, i);
            assert_eq!(m.score, 0);
        }
    }

    #[test]
    fn exact_match_scores_higher() {
        let items = vec!["open palette", "submit", "quit"];
        let matches = rank("open", &items);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].idx, 0);
        assert!(matches[0].score > 0);
    }

    #[test]
    fn lowercase_queries_match_mixed_case_paths() {
        assert!(!rank("car", &["Cargo.toml"]).is_empty());
    }

    #[test]
    fn partial_match_returns_matches() {
        let items = vec!["open palette", "open slash menu", "submit"];
        let matches = rank("open", &items);
        assert_eq!(matches.len(), 2);
        // Both start with "open" — both should match
        assert_eq!(matches[0].idx, 0);
        assert_eq!(matches[1].idx, 1);
    }

    #[test]
    fn no_match_empty() {
        let items = vec!["submit", "quit"];
        let matches = rank("zzznonexistent", &items);
        assert!(matches.is_empty());
    }

    #[test]
    fn match_positions_populated() {
        let items = vec!["open palette"];
        let matches = rank("opn", &items);
        assert_eq!(matches.len(), 1);
        // "opn" should match positions 0,1 and 3 in "open palette"
        assert!(!matches[0].positions.is_empty());
    }
}
