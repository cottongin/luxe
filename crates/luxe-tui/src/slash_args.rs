//! slash_args — a slash command's argument forms, read as a small grammar.
//!
//! Every argument-taking command already lists its forms for the palette's detail pane
//! ([`crate::commands::Command::usage`]): `/diff pr <number or URL>`, `/dock left`. Read as words —
//! literals to type as written, `<placeholders>` for a value — those same lines answer the two
//! questions the composer asks while a command is being typed: which words of the draft are the
//! command's own, so they can be lit ([`marks`]), and which forms the draft could still become, so
//! the picker can list them ([`fit`], [`fill`]). No second description of the grammar exists to
//! drift from the first: a form added to `usage` lights and lists the moment it is written.

use std::ops::Range;

use crate::commands::{Command, CommandRegistry};

/// One word of an argument form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Word<'a> {
    /// Typed as written: `pr`, `left`
    Literal(&'a str),
    /// Stands for a value: `<number or URL>`
    Placeholder(&'a str),
}

/// The words of a usage form after its trigger. A placeholder may hold spaces (`<number or URL>`)
/// and runs on to the next space after its closing `>`, so `<path>:<line>` is one placeholder.
pub(crate) fn words(form: &str) -> Vec<Word<'_>> {
    let Some((_, rest)) = form.trim_start().split_once(char::is_whitespace) else {
        return Vec::new();
    };
    fn word(text: &str) -> Word<'_> {
        match text.starts_with('<') {
            true => Word::Placeholder(text),
            false => Word::Literal(text),
        }
    }
    let mut words = Vec::new();
    let mut start = None;
    let mut bracketed = false;
    for (at, character) in rest.char_indices() {
        match (start, character) {
            (None, character) if character.is_whitespace() => {}
            (None, character) => {
                start = Some(at);
                bracketed = character == '<';
            }
            (Some(_), '<') => bracketed = true,
            (Some(_), '>') => bracketed = false,
            (Some(begun), character) if character.is_whitespace() && !bracketed => {
                words.push(word(&rest[begun..at]));
                start = None;
            }
            (Some(_), _) => {}
        }
    }
    if let Some(begun) = start {
        words.push(word(&rest[begun..]));
    }
    words
}

/// How typed arguments sit against a form, when they could still become it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fit {
    /// Every word of the form has something typed for it, so it can run as typed
    pub(crate) complete: bool,
}

/// Whether `typed` — what follows the trigger — could still become the form `words`, and if so
/// whether it already is.
///
/// A literal must match as written, except the word still being typed, which only has to start it;
/// a placeholder takes any word, and the last one takes all the words after it, since `/compact
/// <focus>` means a sentence.
pub(crate) fn fit(words: &[Word<'_>], typed: &str) -> Option<Fit> {
    let open = !typed.is_empty() && !typed.ends_with(char::is_whitespace);
    let tokens: Vec<&str> = typed.split_whitespace().collect();
    let mut at = 0;
    let mut last_open_literal_whole = true;
    for (index, token) in tokens.iter().enumerate() {
        let still_typing = open && index + 1 == tokens.len();
        match words.get(at) {
            Some(Word::Literal(literal)) => {
                let fits = match still_typing {
                    true => literal.starts_with(token),
                    false => literal == token,
                };
                if !fits {
                    return None;
                }
                if still_typing {
                    last_open_literal_whole = literal == token;
                }
                at += 1;
            }
            Some(Word::Placeholder(_)) => at += 1,
            None if matches!(words.last(), Some(Word::Placeholder(_))) => {}
            None => return None,
        }
    }
    Some(Fit { complete: at >= words.len() && last_open_literal_whole })
}

/// The draft Tab writes for a form: the trigger and the form's leading literals, ready for the value
/// that comes next. `None` when what was typed already goes past them, so there is nothing to add.
///
/// Args: `(trigger, words, typed)`.
pub(crate) fn fill(trigger: &str, words: &[Word<'_>], typed: &str) -> Option<String> {
    let literals: Vec<&str> = words
        .iter()
        .map_while(|word| match word {
            Word::Literal(literal) => Some(*literal),
            Word::Placeholder(_) => None,
        })
        .collect();
    if typed.split_whitespace().count() > literals.len() {
        return None;
    }
    let mut draft = trigger.to_string();
    for literal in &literals {
        draft.push(' ');
        draft.push_str(literal);
    }
    // A value comes next: leave the space it goes after
    if literals.len() < words.len() {
        draft.push(' ');
    }
    Some(draft)
}

/// What a lit stretch of the composer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// A slash trigger luxe answers to: `/diff`
    Command,
    /// A word one of its forms spells out: the `pr` in `/diff pr 12`
    Keyword,
}

/// The stretches of `draft` to light: a known trigger at its start, and the words after it that one
/// of the command's forms spells out. A trigger luxe does not know lights nothing, which is the cue
/// that it was mistyped; values (`12`, a path) stay in the text's own color.
///
/// Only the first line: a command is one line, and whatever follows a newline is prose.
pub(crate) fn marks(draft: &str, registry: &CommandRegistry) -> Vec<(Range<usize>, Mark)> {
    let line = draft.split('\n').next().unwrap_or_default();
    if !line.starts_with('/') {
        return Vec::new();
    }
    let trigger_end = line.find(char::is_whitespace).unwrap_or(line.len());
    let trigger = &line[..trigger_end];
    let Some(command) = registry.iter().find(|command| command.slash_name() == Some(trigger))
    else {
        return Vec::new();
    };
    let mut marks = vec![(0..trigger_end, Mark::Command)];
    let tokens = tokens_with_offsets(line, trigger_end);
    let mut keywords: Vec<Range<usize>> = Vec::new();
    for form in forms(command) {
        let words = words(form);
        for (at, (range, token)) in tokens.iter().enumerate() {
            match words.get(at) {
                Some(Word::Literal(literal)) if literal == token => {
                    if !keywords.contains(range) {
                        keywords.push(range.clone());
                    }
                }
                Some(Word::Placeholder(_)) => {}
                _ => break,
            }
        }
    }
    keywords.sort_by_key(|range| range.start);
    marks.extend(keywords.into_iter().map(|range| (range, Mark::Keyword)));
    marks
}

/// The command's usage forms that take arguments: everything but the bare trigger.
pub(crate) fn forms(command: &Command) -> impl Iterator<Item = &str> {
    command.usage.iter().map(|(form, _)| form.as_ref())
}

/// Each whitespace-separated word of `line` from `from` on, with where it sits.
fn tokens_with_offsets(line: &str, from: usize) -> Vec<(Range<usize>, &str)> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (offset, character) in line[from..].char_indices().map(|(at, c)| (at + from, c)) {
        match (character.is_whitespace(), start) {
            (true, Some(begun)) => {
                tokens.push((begun..offset, &line[begun..offset]));
                start = None;
            }
            (false, None) => start = Some(offset),
            _ => {}
        }
    }
    if let Some(begun) = start {
        tokens.push((begun..line.len(), &line[begun..]));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_reads_as_literals_and_placeholders() {
        assert_eq!(
            words("/diff pr <number or URL>"),
            [Word::Literal("pr"), Word::Placeholder("<number or URL>")]
        );
        assert_eq!(words("/editor <path>:<line>"), [Word::Placeholder("<path>:<line>")]);
        assert_eq!(words("/dock left"), [Word::Literal("left")]);
        assert_eq!(
            words("/x  a   <two words>  b"),
            [Word::Literal("a"), Word::Placeholder("<two words>"), Word::Literal("b")],
            "however many spaces sit between"
        );
        assert!(words("/diff").is_empty());
    }

    #[test]
    fn typed_arguments_fit_the_forms_they_could_still_become() {
        let pr = words("/diff pr <number or URL>");
        assert_eq!(fit(&pr, ""), Some(Fit { complete: false }));
        assert_eq!(fit(&pr, "p"), Some(Fit { complete: false }), "still typing `pr`");
        assert_eq!(fit(&pr, "pr "), Some(Fit { complete: false }), "waiting on the number");
        assert_eq!(fit(&pr, "pr 12"), Some(Fit { complete: true }));
        assert_eq!(fit(&pr, "base main"), None, "a different form");
        assert_eq!(fit(&pr, "pr 12 13"), Some(Fit { complete: true }), "the last value runs on");

        let turn = words("/diff turn");
        assert_eq!(fit(&turn, "tu"), Some(Fit { complete: false }));
        assert_eq!(fit(&turn, "turn"), Some(Fit { complete: true }));
        assert_eq!(fit(&turn, "turn now"), None, "nothing takes a second word");
        assert_eq!(fit(&turn, "tu "), None, "a finished word has to be the whole literal");

        let compact = words("/compact <focus>");
        assert_eq!(
            fit(&compact, "keep the API decisions"),
            Some(Fit { complete: true }),
            "the last placeholder takes the sentence"
        );
        assert_eq!(fit(&[], ""), Some(Fit { complete: true }), "the bare form");
        assert_eq!(fit(&[], "x"), None);
    }

    #[test]
    fn tab_writes_the_forms_literals_and_leaves_room_for_the_value() {
        let pr = words("/diff pr <number or URL>");
        assert_eq!(fill("/diff", &pr, "p").as_deref(), Some("/diff pr "));
        assert_eq!(fill("/diff", &pr, "pr 12"), None, "already past them");
        let turn = words("/diff turn");
        assert_eq!(fill("/diff", &turn, "t").as_deref(), Some("/diff turn"));
        let path = words("/preview <path>");
        assert_eq!(fill("/preview", &path, "").as_deref(), Some("/preview "));
    }

    #[test]
    fn a_known_trigger_and_its_spelled_out_words_are_lit() {
        let registry = CommandRegistry::all();
        let marks = marks("/diff pr 12", &registry);
        assert_eq!(marks, vec![(0..5, Mark::Command), (6..8, Mark::Keyword)]);
        assert_eq!(
            marks_of("/dock left", &registry),
            [(0..5, Mark::Command), (6..10, Mark::Keyword)]
        );
        assert_eq!(
            marks_of("/diff 12", &registry),
            [(0..5, Mark::Command)],
            "a value is not a word of it"
        );
        assert!(marks_of("/dif pr", &registry).is_empty(), "a mistyped trigger lights nothing");
        assert!(marks_of("look at /diff", &registry).is_empty(), "only at the start");
        assert_eq!(
            marks_of("/compact keep\n/diff pr", &registry),
            [(0..8, Mark::Command)],
            "the first line only"
        );
    }

    /// A word a form spells out after a value lights too: the value is skipped, not the end.
    #[test]
    fn a_word_after_a_value_is_lit() {
        use std::borrow::Cow;
        let command = Command {
            id: Cow::Borrowed("cmd.test"),
            name: Cow::Borrowed("Test"),
            slash: Some(Cow::Borrowed("/move")),
            summary: Cow::Borrowed(""),
            detail: Cow::Borrowed(""),
            usage: vec![(Cow::Borrowed("/move <file> to <folder>"), Cow::Borrowed(""))],
            keywords: Vec::new(),
            kind: crate::commands::CommandKind::Slash,
            keys: None,
            needs: Cow::Borrowed(&[]),
            args: None,
        };
        let registry = CommandRegistry::from_sources([vec![command]]);
        assert_eq!(
            marks_of("/move a.rs to src", &registry),
            [(0..5, Mark::Command), (11..13, Mark::Keyword)]
        );
    }

    fn marks_of(draft: &str, registry: &CommandRegistry) -> Vec<(Range<usize>, Mark)> {
        marks(draft, registry)
    }
}
