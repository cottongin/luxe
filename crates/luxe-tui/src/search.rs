//! In-conversation search state.
//!
//! Tracks query, match positions, and current match index for next/prev navigation.

/// Search state for in-conversation text search.
#[derive(Debug, Clone)]
pub struct SearchState {
    /// Whether the search UI is active (query bar visible, input routed here).
    pub searching: bool,
    /// Current search query (empty = no active search).
    pub query: String,
    /// Line indices where the query matches (0-indexed into the flattened line list).
    pub matches: Vec<usize>,
    /// Index of the currently selected match in `matches`.
    pub current_match: Option<usize>,
}

impl SearchState {
    /// Create a new empty search state.
    pub fn new() -> Self {
        Self { searching: false, query: String::new(), matches: Vec::new(), current_match: None }
    }

    /// Whether the search UI is active (query bar visible, input routed here).
    pub fn is_active(&self) -> bool {
        self.searching
    }

    /// Enter search mode (called on SearchConvo action).
    pub fn enter(&mut self) {
        self.searching = true;
        self.query.clear();
        self.matches.clear();
        self.current_match = None;
    }

    /// Search the given lines for the query. Returns the number of matches found.
    ///
    /// Takes an iterator rather than a slice so a caller can search the lines it has already laid
    /// out, instead of cloning the whole transcript into a second vector to be scanned once.
    pub fn search<'a, 'b: 'a>(
        &mut self,
        lines: impl IntoIterator<Item = &'a ratatui::text::Line<'b>>,
    ) -> usize {
        if self.query.is_empty() {
            self.matches.clear();
            self.current_match = None;
            return 0;
        }

        let query_lower = self.query.to_lowercase();
        self.matches = lines
            .into_iter()
            .enumerate()
            .filter(|(_, line)| {
                line.spans.iter().any(|span| span.content.to_lowercase().contains(&query_lower))
            })
            .map(|(idx, _)| idx)
            .collect();

        // Reset current match if it's out of bounds.
        if let Some(idx) = self.current_match {
            if idx >= self.matches.len() {
                self.current_match = None;
            }
        }

        self.matches.len()
    }

    /// Advance to the next match (wraps around). Returns the new current match index.
    pub fn next_match(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let idx = match self.current_match {
            Some(i) => i,
            None => self.matches.len() - 1, // start from last, so first next wraps to 0
        };
        let new_idx = (idx + 1) % self.matches.len();
        self.current_match = Some(new_idx);
        Some(self.matches[new_idx])
    }

    /// Go to the previous match (wraps around). Returns the new current match index.
    pub fn prev_match(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let idx = self.current_match.unwrap_or(0);
        let new_idx = if idx == 0 { self.matches.len() - 1 } else { idx - 1 };
        self.current_match = Some(new_idx);
        Some(self.matches[new_idx])
    }

    /// Clear the search state and exit search mode.
    pub fn clear(&mut self) {
        self.searching = false;
        self.query.clear();
        self.matches.clear();
        self.current_match = None;
    }

    /// Type a character into the search query and re-search.
    pub fn type_char(&mut self, c: char, lines: &[ratatui::text::Line]) {
        self.query.push(c);
        self.search(lines);
        // Reset current match when query changes.
        self.current_match = None;
    }
}

impl Default for SearchState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::{Line, Span};

    fn line(text: &str) -> Line<'_> {
        Line::from(Span::raw(text))
    }

    #[test]
    fn test_empty_query_no_matches() {
        let mut state = SearchState::new();
        let lines = vec![line("hello"), line("world")];
        assert_eq!(state.search(&lines), 0);
        assert!(!state.is_active());
    }

    #[test]
    fn test_search_finds_matches() {
        let mut state = SearchState::new();
        state.searching = true;
        state.query = "hello".to_string();
        let lines = vec![line("hello"), line("world"), line("HELLO")];
        assert_eq!(state.search(&lines), 2);
        assert!(state.is_active());
    }

    #[test]
    fn test_next_wraps_around() {
        let mut state = SearchState::new();
        state.query = "x".to_string();
        let lines = vec![line("x"), line("y"), line("x")];
        let count = state.search(&lines);
        assert_eq!(count, 2, "expected 2 matches");
        assert_eq!(state.matches, vec![0, 2], "matches should contain line indices [0, 2]");

        // next() returns the LINE INDEX (not match index), so first call returns 0,
        // second returns 2, third wraps back to 0.
        let first = state.next_match();
        assert_eq!(first, Some(0), "first next should return line index 0");

        let second = state.next_match();
        assert_eq!(second, Some(2), "second next should return line index 2");

        let third = state.next_match();
        assert_eq!(third, Some(0), "third next should wrap to line index 0");
    }

    #[test]
    fn test_prev_wraps_around() {
        let mut state = SearchState::new();
        state.query = "x".to_string();
        let lines = vec![line("x"), line("y"), line("x")];
        state.search(&lines);

        assert_eq!(state.prev_match(), Some(2)); // wraps to end
        assert_eq!(state.prev_match(), Some(0));
        assert_eq!(state.prev_match(), Some(2)); // wraps
    }

    #[test]
    fn test_clear_resets_state() {
        let mut state = SearchState::new();
        state.query = "test".to_string();
        let lines = vec![line("test")];
        state.search(&lines);

        state.clear();
        assert!(!state.is_active());
        assert!(state.matches.is_empty());
        assert!(state.current_match.is_none());
    }

    #[test]
    fn test_case_insensitive_search() {
        let mut state = SearchState::new();
        state.query = "Hello".to_string();
        let lines = vec![line("hello"), line("HELLO"), line("HeLLo")];
        assert_eq!(state.search(&lines), 3);
    }

    #[test]
    fn test_no_matches_updates_current_match() {
        let mut state = SearchState::new();
        state.query = "x".to_string();
        let lines = vec![line("y"), line("z")];
        state.search(&lines);

        assert_eq!(state.matches.len(), 0);
        assert!(state.current_match.is_none());
    }
}
