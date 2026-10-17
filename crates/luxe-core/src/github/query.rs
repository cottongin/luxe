//! Search queries — `/github`'s tabs read and rewritten as the words a reader types.
//!
//! A tab is a GitHub search with `{repo}` standing for the checkout's repository. A tab that names
//! `repo:{repo}` is *scoped*: the reader never sees that term and can never type their way out of
//! it, because GitHub ORs several `repo:` terms together, so one typed beside the scope would widen
//! it rather than narrow it. A configured tab without it searches wherever it says and is run as
//! typed. The state a tab lists (open, closed, merged, all) is a word in the query like any other,
//! rewritten here so every front-end cycles it the same way.

/// The scope term a tab names to stay in the checkout's repository.
pub const SCOPE: &str = "repo:{repo}";

/// What a search lists by state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    /// Closed without merging, for pull requests; closed, for issues
    Closed,
    Merged,
    /// No state term at all
    All,
}

impl State {
    /// The word a header says it with.
    pub fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
            Self::All => "all",
        }
    }
}

/// What a search lists by kind, which decides what its states mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    PullRequests,
    Issues,
    Both,
}

/// Whether `query` keeps to the checkout's repository.
pub fn is_scoped(query: &str) -> bool {
    terms(query).iter().any(|term| term.eq_ignore_ascii_case(SCOPE))
}

/// What the reader sees and edits of `query`: everything but the scope a scoped tab carries
/// without saying.
pub fn shown(query: &str) -> String {
    join(terms(query).into_iter().filter(|term| !term.eq_ignore_ascii_case(SCOPE)))
}

/// `typed` as a tab first searching `original` runs it: kept in the checkout's repository when the
/// tab was, with any repository, organization or user typed beside it dropped, and as typed when
/// it was not.
pub fn edited(original: &str, typed: &str) -> String {
    if !is_scoped(original) {
        return join(terms(typed));
    }
    let kept = terms(typed).into_iter().filter(|term| !names_a_place(term));
    join(kept.chain(std::iter::once(SCOPE)))
}

/// What `query` lists by kind.
pub fn kind(query: &str) -> Kind {
    let terms = terms(query);
    let has =
        |words: &[&str]| terms.iter().any(|t| words.iter().any(|w| t.eq_ignore_ascii_case(w)));
    match (has(&["is:pr", "type:pr"]), has(&["is:issue", "type:issue"])) {
        (true, false) => Kind::PullRequests,
        (false, true) => Kind::Issues,
        _ => Kind::Both,
    }
}

/// What `query` lists by state.
pub fn state(query: &str) -> State {
    let terms = terms(query);
    let has =
        |words: &[&str]| terms.iter().any(|t| words.iter().any(|w| t.eq_ignore_ascii_case(w)));
    if has(&["is:merged"]) {
        State::Merged
    } else if has(&["is:closed", "state:closed"]) {
        State::Closed
    } else if has(&["is:open", "state:open"]) {
        State::Open
    } else {
        State::All
    }
}

/// The state after `query`'s, in the order a reader cycles them: open, closed, merged, all. An
/// issue is never merged, so a search for issues alone skips it.
pub fn next_state(query: &str) -> State {
    match (state(query), kind(query)) {
        (State::Open, _) => State::Closed,
        (State::Closed, Kind::Issues) => State::All,
        (State::Closed, _) => State::Merged,
        (State::Merged, _) => State::All,
        (State::All, _) => State::Open,
    }
}

/// `query` listing `state` instead: every state term dropped and the new one put first, where a
/// reader looks for it.
pub fn with_state(query: &str, state: State) -> String {
    let words: &[&str] = match (state, kind(query)) {
        (State::Open, _) => &["is:open"],
        // Closed alone would bring the merged ones too, which have a state of their own
        (State::Closed, Kind::PullRequests) => &["is:closed", "is:unmerged"],
        (State::Closed, _) => &["is:closed"],
        (State::Merged, _) => &["is:merged"],
        (State::All, _) => &[],
    };
    let rest = terms(query).into_iter().filter(|term| !is_state(term));
    join(words.iter().copied().chain(rest))
}

/// Whether `term` says where to search: a repository, an organization or a user, or not one.
fn names_a_place(term: &str) -> bool {
    let bare = term.strip_prefix('-').unwrap_or(term).to_ascii_lowercase();
    ["repo:", "org:", "user:"].iter().any(|qualifier| bare.starts_with(qualifier))
}

/// Whether `term` says which states to list, or which not to.
fn is_state(term: &str) -> bool {
    let bare = term.strip_prefix('-').unwrap_or(term).to_ascii_lowercase();
    ["is:open", "is:closed", "is:merged", "is:unmerged", "state:open", "state:closed"]
        .contains(&bare.as_str())
}

/// `query`'s terms, split on whitespace outside double quotes, so `label:"good first issue"` is one.
fn terms(query: &str) -> Vec<&str> {
    let mut terms = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (index, character) in query.char_indices() {
        match character {
            '"' => {
                quoted = !quoted;
                start.get_or_insert(index);
            }
            c if c.is_whitespace() && !quoted => {
                if let Some(from) = start.take() {
                    terms.push(&query[from..index]);
                }
            }
            _ => {
                start.get_or_insert(index);
            }
        }
    }
    if let Some(from) = start {
        terms.push(&query[from..]);
    }
    terms
}

fn join<'a>(terms: impl IntoIterator<Item = &'a str>) -> String {
    terms.into_iter().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PULLS: &str = "is:open is:pr sort:updated-desc repo:{repo}";
    const ISSUES: &str = "is:open is:issue sort:updated-desc repo:{repo}";

    #[test]
    fn a_scoped_query_shows_without_its_scope() {
        assert!(is_scoped(PULLS));
        assert_eq!(shown(PULLS), "is:open is:pr sort:updated-desc");
        assert!(!is_scoped("is:open is:pr review-requested:@me"));
        assert_eq!(shown("is:pr  author:@me"), "is:pr author:@me", "spacing is tidied");
    }

    /// Typing a repository beside the scope would widen it, since GitHub ORs `repo:` terms, so a
    /// scoped tab drops every place the reader names and keeps its own.
    #[test]
    fn a_scoped_tab_stays_in_its_repository_whatever_is_typed() {
        assert_eq!(
            edited(PULLS, "is:merged repo:other/lib -org:acme user:mara label:bug"),
            "is:merged label:bug repo:{repo}"
        );
        assert_eq!(edited(PULLS, "REPO:Other/Lib"), "repo:{repo}", "qualifiers ignore case");
        assert_eq!(edited(PULLS, ""), "repo:{repo}");
        assert_eq!(edited(PULLS, "reporter"), "reporter repo:{repo}", "a word is not a qualifier");
    }

    #[test]
    fn an_unscoped_tab_runs_as_typed() {
        let everywhere = "is:open is:pr review-requested:@me archived:false";
        assert_eq!(edited(everywhere, "is:pr repo:cli/cli"), "is:pr repo:cli/cli");
    }

    /// A phrase is one term, so a qualifier inside one is a word of it: split on every space, the
    /// `is:open` in `"fix is:open bug"` would be rewritten and the phrase torn open.
    #[test]
    fn a_quoted_phrase_is_one_term() {
        let typed = r#"label:"good first issue" "see repo:x here" repo:x/y"#;
        assert_eq!(
            edited(PULLS, typed),
            r#"label:"good first issue" "see repo:x here" repo:{repo}"#
        );
        assert_eq!(
            with_state(r#"is:pr "fix is:open bug""#, State::Merged),
            r#"is:merged is:pr "fix is:open bug""#
        );
        assert_eq!(state(r#"is:pr "a is:closed b""#), State::All);
        assert_eq!(kind(r#""about is:issue" is:pr"#), Kind::PullRequests);
    }

    #[test]
    fn a_query_says_its_kind_and_state() {
        assert_eq!((kind(PULLS), state(PULLS)), (Kind::PullRequests, State::Open));
        assert_eq!((kind(ISSUES), state("is:issue")), (Kind::Issues, State::All));
        assert_eq!(kind("type:PR is:issue"), Kind::Both);
        assert_eq!(state("is:pr is:closed is:unmerged"), State::Closed);
        assert_eq!(state("is:pr state:closed"), State::Closed);
        assert_eq!(state("is:pr is:merged"), State::Merged);
        assert_eq!(state("is:pr STATE:open"), State::Open);
    }

    #[test]
    fn pull_requests_cycle_through_every_state() {
        let mut query = PULLS.to_string();
        let mut seen = Vec::new();
        for _ in 0..4 {
            query = with_state(&query, next_state(&query));
            seen.push(query.clone());
        }
        assert_eq!(
            seen,
            [
                "is:closed is:unmerged is:pr sort:updated-desc repo:{repo}",
                "is:merged is:pr sort:updated-desc repo:{repo}",
                "is:pr sort:updated-desc repo:{repo}",
                "is:open is:pr sort:updated-desc repo:{repo}",
            ]
        );
    }

    /// An issue is never merged, so its tab goes from closed straight to all.
    #[test]
    fn issues_skip_merged() {
        let closed = with_state(ISSUES, next_state(ISSUES));
        assert_eq!(closed, "is:closed is:issue sort:updated-desc repo:{repo}");
        assert_eq!(next_state(&closed), State::All);
    }

    /// A search for both kinds closes without `is:unmerged`, which no issue matches.
    #[test]
    fn both_kinds_close_without_unmerged() {
        assert_eq!(with_state("label:bug", State::Closed), "is:closed label:bug");
        assert_eq!(next_state("is:closed label:bug"), State::Merged);
    }

    #[test]
    fn every_state_term_is_replaced_including_negations() {
        assert_eq!(
            with_state("is:pr state:open -is:merged is:unmerged is:draft", State::Merged),
            "is:merged is:pr is:draft"
        );
    }

    #[test]
    fn a_state_has_a_word() {
        let words: Vec<&str> =
            [State::Open, State::Closed, State::Merged, State::All].map(State::word).to_vec();
        assert_eq!(words, ["open", "closed", "merged", "all"]);
    }
}
