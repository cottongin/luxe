//! Argument normalization for `git commit` — permissive at the edge, strict in the core.
//!
//! The tool's real invariant is that git receives a message from somewhere. *Where the caller put
//! it* is not one: a model that sends `args: ["-m", "…"]` instead of `message` has said what it
//! means, and refusing it costs failed calls and a fallback to `bash`.
//!
//! So: lift the message out of whichever spelling arrived, then be strict about what actually
//! matters. The schema still advertises exactly one spelling, so nothing here is a second contract
//! to keep.

use serde_json::Value;

/// Keys accepted for the commit message, canonical first.
const MESSAGE_KEYS: [&str; 4] = ["message", "msg", "commit_message", "commitMessage"];

/// Flags that hand git a message from somewhere other than `-m`, so a call carrying one needs no
/// `message` of its own.
///
/// Verified against git rather than assumed: each of these commits without `-m`, and combining one
/// with `-m` is an error git refuses outright ("Only one of -c/-C/-F/--fixup can be used"), so
/// demanding a message unconditionally would put them out of reach.
const MESSAGE_SOURCES: [&str; 9] = [
    "--no-edit",
    "-F",
    "--file",
    "-C",
    "--reuse-message",
    "-c",
    "--reedit-message",
    "--fixup",
    "--squash",
];

/// Short `git commit` flags that take no value, and so may cluster ahead of an `m` (`-am`).
///
/// An allow-list rather than "any letter", because `-F`, `-C`, `-c` and `-S` all take one: reading
/// `-Fm` as a message flag would silently reinterpret "take the message from the file named `m`".
const CLUSTERABLE: &str = "asnevqoip";

/// The canonical shape, repeated on every failure so a correction needs no second look.
const USAGE: &str = "Expected: {\"action\": \"commit\", \"message\": \"Subject line\\n\\nOptional \
                     body.\"}";

/// Why a commit call could not be understood, with enough detail to fix it in one move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommitError {
    /// Nothing in the call carries a message, and no flag supplies one either.
    NoMessage {
        /// Top-level keys the call actually arrived with.
        received: Vec<String>,
    },
    /// `--amend` with no new message and no `--no-edit`, which git can only resolve by opening an
    /// editor there is nobody to type into.
    AmendNeedsNoEdit,
}

impl CommitError {
    /// A message that names what arrived, what was expected, and the shape to send.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NoMessage { received } => format!(
                "Error: no commit message. Received keys: {}. Pass it as `message` — `args` is for \
                 git's other flags, so a message put there is a flag git does not have.\n{USAGE}\n\
                 A commit that takes its message from elsewhere needs no `message`: \
                 `--amend --no-edit`, `-F <file>`, `-C <commit>`, `--fixup=<commit>`.",
                render_keys(received)
            ),
            Self::AmendNeedsNoEdit => format!(
                "Error: `--amend` with no message. Add `--no-edit` to keep the existing one, or \
                 pass `message` to replace it.\nExpected: {{\"action\": \"commit\", \"args\": \
                 [\"--amend\", \"--no-edit\"]}}\n{USAGE}"
            ),
        }
    }
}

/// The full argument list for a `commit` call, starting at `commit`.
///
/// `extra` is the call's `args` array, already extracted.
pub(crate) fn commit_args(call: &Value, extra: &[String]) -> Result<Vec<String>, CommitError> {
    // The advertised spelling wins outright, and `args` then passes through untouched: a second
    // `-m` there is git's own way of writing a second paragraph, and lifting it out would quietly
    // drop what the caller asked for.
    if let Some(message) = first_string(call, &MESSAGE_KEYS) {
        return Ok(assemble(Some(message), extra.to_vec()));
    }

    let (lifted, rest) = lift_message_flag(extra);
    if let Some(message) = lifted {
        return Ok(assemble(Some(message), rest));
    }

    // A lone argument that is not a flag can only be the message. Git would read it as a pathspec,
    // and a pathspec commit with no message anywhere has nowhere to get one from — the editor is
    // closed — so the reading that lets the call succeed is the only one worth taking.
    if let [only] = rest.as_slice() {
        if !only.starts_with('-') {
            return Ok(assemble(Some(only.clone()), Vec::new()));
        }
    }

    if carries_its_own_message(&rest) {
        return Ok(assemble(None, rest));
    }
    if rest.iter().any(|arg| arg == "--amend") {
        return Err(CommitError::AmendNeedsNoEdit);
    }
    Err(CommitError::NoMessage { received: keys_of(call) })
}

/// Put the message where git wants it, ahead of whatever else the caller sent.
fn assemble(message: Option<String>, rest: Vec<String>) -> Vec<String> {
    let mut args = vec!["commit".to_string()];
    if let Some(message) = message {
        args.push("-m".to_string());
        args.push(message);
    }
    args.extend(rest);
    args
}

/// Pull a `-m`/`--message` value out of a raw git argument list, returning it and what is left.
///
/// Every spelling git itself takes: `-m x`, `-mx`, `-am x`, `--message x`, `--message=x`. Only the
/// first is lifted — a later `-m` is a second paragraph and stays where it is.
fn lift_message_flag(args: &[String]) -> (Option<String>, Vec<String>) {
    let mut message: Option<String> = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if message.is_some() {
            rest.push(arg.clone());
            continue;
        }
        if arg == "--message" {
            message = iter.next().cloned();
            continue;
        }
        if let Some(value) = arg.strip_prefix("--message=") {
            message = Some(value.to_string());
            continue;
        }
        match split_short_message(arg) {
            Some((companions, attached)) => {
                if let Some(companions) = companions {
                    rest.push(companions);
                }
                message = match attached {
                    Some(value) => Some(value),
                    None => iter.next().cloned(),
                };
            }
            None => rest.push(arg.clone()),
        }
    }

    (message, rest)
}

/// Split a short-flag cluster carrying `m` into the flags beside it and any attached value.
///
/// `-m` → `(None, None)`, `-am` → `(Some("-a"), None)`, `-mwip` → `(None, Some("wip"))`. Anything
/// that is not a `git commit` message flag returns `None` and is left alone.
fn split_short_message(arg: &str) -> Option<(Option<String>, Option<String>)> {
    let cluster = arg.strip_prefix('-').filter(|rest| !rest.starts_with('-'))?;
    let at = cluster.find('m')?;
    let (companions, attached) = cluster.split_at(at);
    if !companions.chars().all(|flag| CLUSTERABLE.contains(flag)) {
        return None;
    }
    let attached = attached.strip_prefix('m').filter(|value| !value.is_empty());
    let companions = (!companions.is_empty()).then(|| format!("-{companions}"));
    Some((companions, attached.map(ToOwned::to_owned)))
}

/// Whether these arguments already tell git where the message comes from.
fn carries_its_own_message(args: &[String]) -> bool {
    args.iter().any(|arg| {
        MESSAGE_SOURCES.iter().any(|flag| match flag.strip_prefix("--") {
            // A long flag attaches its value with `=`: `--fixup=HEAD`.
            Some(_) => arg == flag || arg.strip_prefix(flag).is_some_and(|v| v.starts_with('=')),
            // A short one just runs into it: `-CHEAD`. No long flag can be caught here, because a
            // long flag's second character is always `-` and no short flag's is.
            None => arg.starts_with(flag),
        })
    })
}

fn keys_of(value: &Value) -> Vec<String> {
    value.as_object().map(|map| map.keys().cloned().collect()).unwrap_or_default()
}

fn render_keys(keys: &[String]) -> String {
    if keys.is_empty() {
        return "(none)".to_string();
    }
    keys.join(", ")
}

/// The first alias present and non-blank.
///
/// Blank is not a message: git refuses an empty one ("Aborting commit due to empty commit
/// message"), so accepting it here would only move the failure somewhere less legible.
fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .map(ToOwned::to_owned)
}

/// The full `gh` argument list for a `pr` or `issue` call, defaulting to listing them.
///
/// Every argument is forwarded: dropping them would send `pr create --title … --body …` to `gh` as
/// a bare `gh pr create`, which does not fail loudly but *prompts* for the title it was already
/// handed, from behind a TUI that owns the screen.
pub(super) fn gh_args(kind: &str, requested: &[String]) -> Vec<String> {
    let mut args = vec![kind.to_string()];
    if requested.is_empty() {
        args.push("list".to_string());
    } else {
        args.extend(requested.iter().cloned());
    }
    args
}

/// Whether a `pr` call is trying to open a pull request, as opposed to reading about one.
///
/// Only `create` has a precondition the caller can get wrong, so only `create` gets the preflight.
pub(super) fn is_pr_create(requested: &[String]) -> bool {
    requested.first().is_some_and(|subaction| subaction == "create")
}

/// The full `git stash` argument list, defaulting to `stash push`.
///
/// Every argument is forwarded, as in [`gh_args`]: a dropped `-m wip` loses the label that was the
/// entire point of naming the stash, and any way to find the entry again.
pub(super) fn stash_args(requested: &[String]) -> Vec<String> {
    let mut args = vec!["stash".to_string()];
    if requested.is_empty() {
        args.push("push".to_string());
    } else {
        args.extend(requested.iter().cloned());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    /// The shapes a real session produced, all of which were refused.
    ///
    /// Taken from a transcript, not imagined: the model tried `["-m", msg]`, then `[msg]`, got
    /// "'message' argument is required for commit" both times, and went to `bash`.
    #[test]
    fn every_spelling_a_model_reaches_for_becomes_the_same_commit() {
        let expected = owned(&["commit", "-m", "Animate the loading spinner"]);
        let cases = [
            json!({"action": "commit", "message": "Animate the loading spinner"}),
            json!({"action": "commit", "args": ["-m", "Animate the loading spinner"]}),
            json!({"action": "commit", "args": ["Animate the loading spinner"]}),
            json!({"action": "commit", "args": ["--message", "Animate the loading spinner"]}),
            json!({"action": "commit", "args": ["--message=Animate the loading spinner"]}),
            json!({"action": "commit", "args": ["-mAnimate the loading spinner"]}),
        ];
        for call in cases {
            let extra = extra_of(&call);
            assert_eq!(commit_args(&call, &extra).unwrap(), expected, "{call}");
        }
    }

    /// A clustered `-am` keeps the flag it was clustered with.
    #[test]
    fn a_short_cluster_surrenders_the_message_and_keeps_its_flags() {
        let call = json!({"action": "commit", "args": ["-am", "wip"]});
        assert_eq!(
            commit_args(&call, &extra_of(&call)).unwrap(),
            owned(&["commit", "-m", "wip", "-a"])
        );

        // `-F` takes a value, so `-Fm` is the file named `m` and must survive untouched.
        assert_eq!(split_short_message("-Fm"), None);
        assert_eq!(split_short_message("-Sm"), None);
        assert_eq!(split_short_message("--amend"), None);
        assert_eq!(split_short_message("-a"), None);
    }

    /// The advertised spelling wins, and a `-m` in `args` stays put rather than being dropped —
    /// repeated `-m` is how git writes a second paragraph.
    #[test]
    fn the_canonical_spelling_wins_without_discarding_the_other() {
        let call = json!({"action": "commit", "message": "subject", "args": ["-m", "body"]});
        assert_eq!(
            commit_args(&call, &extra_of(&call)).unwrap(),
            owned(&["commit", "-m", "subject", "-m", "body"])
        );
    }

    /// Flags git accepts a message *from* need no message of their own. Every one of these was
    /// unreachable while `message` was unconditionally required.
    #[test]
    fn a_commit_that_names_its_own_message_source_needs_no_message() {
        for args in [
            vec!["--amend", "--no-edit"],
            vec!["-F", "msg.txt"],
            vec!["--file=msg.txt"],
            vec!["-C", "HEAD"],
            vec!["--reuse-message=HEAD"],
            vec!["--fixup=HEAD"],
            vec!["--squash", "HEAD"],
            // Attached to the flag, which is how git spells these as often as not.
            vec!["-Fmsg.txt"],
            vec!["-CHEAD"],
            vec!["-cHEAD"],
        ] {
            let call = json!({"action": "commit", "args": args});
            let extra = extra_of(&call);
            let built = commit_args(&call, &extra).unwrap_or_else(|e| panic!("{args:?}: {e:?}"));
            assert_eq!(built[0], "commit");
            assert!(!built.contains(&"-m".to_string()), "{args:?} got an -m it did not ask for");
        }
    }

    /// `--amend` alone would send git to an editor nobody can reach, so it is refused *here*, where
    /// the reply can name the two ways out.
    #[test]
    fn amending_without_a_message_says_which_two_things_would_work() {
        let call = json!({"action": "commit", "args": ["--amend"]});
        let error = commit_args(&call, &extra_of(&call)).unwrap_err();
        assert_eq!(error, CommitError::AmendNeedsNoEdit);
        let text = error.message();
        assert!(text.contains("--no-edit"), "{text}");
        assert!(text.contains("message"), "{text}");
    }

    /// The refusal that started this: it named the key and never the shape.
    #[test]
    fn a_refusal_shows_the_call_that_would_have_worked() {
        let call = json!({"action": "commit", "args": ["-a"]});
        let error = commit_args(&call, &extra_of(&call)).unwrap_err();
        let text = error.message();

        assert!(text.contains("action, args"), "names what arrived: {text}");
        assert!(text.contains("\"message\""), "and the shape to send: {text}");
        assert!(text.contains("--amend --no-edit"), "and the way out that needs none: {text}");
    }

    #[test]
    fn a_blank_message_is_no_message() {
        for blank in ["", "   ", "\n"] {
            let call = json!({"action": "commit", "message": blank});
            assert!(commit_args(&call, &[]).is_err(), "{blank:?} is not a commit message");
        }
    }

    /// Flags beside the message keep their order and their place after it.
    #[test]
    fn other_flags_survive_in_order() {
        let call = json!({"action": "commit", "message": "m", "args": ["--amend", "--", "a.rs"]});
        assert_eq!(
            commit_args(&call, &extra_of(&call)).unwrap(),
            owned(&["commit", "-m", "m", "--amend", "--", "a.rs"])
        );
    }

    #[test]
    fn an_empty_call_reports_no_keys_rather_than_an_empty_list() {
        let error = commit_args(&json!({}), &[]).unwrap_err();
        assert!(error.message().contains("(none)"), "{}", error.message());
    }

    fn extra_of(call: &Value) -> Vec<String> {
        call.get("args")
            .and_then(|value| value.as_array())
            .map(|values| {
                values.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}
