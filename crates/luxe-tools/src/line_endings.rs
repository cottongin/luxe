//! line_endings — rewrite a file the way it was already written.
//!
//! A model writes `\n`. A Windows working tree, checked out with git's default `core.autocrlf=true`,
//! is `\r\n`. Nothing warns about the mismatch: the `edit` tool's whole-line relaxation trims the
//! stray `\r` and matches happily, then writes the replacement back with bare `\n` — so the four
//! lines it touched are now the only LF lines in a CRLF file, and `git diff` reports a change to
//! every line the user did not ask about.
//!
//! So the rule is: **the file decides.** We detect what it already uses and convert the payload to
//! match. A new file gets `\n`, which is what the model wrote and what git normalizes to on commit.

/// The line ending a file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineEnding {
    /// `\n` — unix, and the default for anything new
    Lf,
    /// `\r\n` — a Windows checkout
    Crlf,
}

/// Which line ending `content` predominantly uses.
///
/// Majority rule rather than "any CRLF wins", so one stray `\r\n` in a generated header does not
/// convert an entire unix file. A file with no newline at all is [`LineEnding::Lf`].
pub(crate) fn detect(content: &str) -> LineEnding {
    let total = content.matches('\n').count();
    if total == 0 {
        return LineEnding::Lf;
    }
    let carriage = content.matches("\r\n").count();
    if carriage * 2 >= total {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

/// Rewrite `text` so every line ends with `ending`, whatever it ends with now.
pub(crate) fn normalize(text: &str, ending: LineEnding) -> String {
    let unix = text.replace("\r\n", "\n");
    match ending {
        LineEnding::Lf => unix,
        LineEnding::Crlf => unix.replace('\n', "\r\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_dominant_ending_is_what_counts() {
        assert_eq!(detect("a\nb\nc\n"), LineEnding::Lf);
        assert_eq!(detect("a\r\nb\r\nc\r\n"), LineEnding::Crlf);
        assert_eq!(detect("no newline at all"), LineEnding::Lf);
        assert_eq!(detect(""), LineEnding::Lf);
    }

    /// One generated `\r\n` in an otherwise unix file must not convert the file.
    #[test]
    fn a_single_stray_carriage_return_does_not_win() {
        assert_eq!(detect("gen\r\na\nb\nc\nd\n"), LineEnding::Lf);
        assert_eq!(detect("a\r\nb\r\nc\n"), LineEnding::Crlf, "but a majority does");
    }

    #[test]
    fn normalizing_is_idempotent_and_lossless_in_both_directions() {
        let unix = "one\ntwo\nthree";
        let windows = "one\r\ntwo\r\nthree";
        assert_eq!(normalize(unix, LineEnding::Crlf), windows);
        assert_eq!(normalize(windows, LineEnding::Lf), unix);
        assert_eq!(normalize(windows, LineEnding::Crlf), windows, "already right, left alone");
        assert_eq!(normalize(unix, LineEnding::Lf), unix);
    }

    /// A lone `\r` is a classic-Mac ending nobody has produced this century, and treating it as a
    /// newline would corrupt any file that uses it as data.
    #[test]
    fn a_bare_carriage_return_is_data_not_a_line_ending() {
        assert_eq!(normalize("a\rb", LineEnding::Crlf), "a\rb");
    }
}
