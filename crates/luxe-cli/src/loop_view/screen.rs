//! Screen — where loop lines go, and taking one back to redraw it.
//!
//! A tool call is printed when it starts, because "is it still going?" is the first question about
//! an unattended run, and its result only exists once it ends. On a terminal the start line is
//! redrawn in place with the result. That's only safe when every line is exactly one row, so on a
//! terminal each line is cut to the width rather than left to wrap, and a resize since the line
//! was printed means it's left alone. A pipe or a file never gets a redraw.

use crate::output::{out, outln};
use unicode_width::UnicodeWidthChar;

/// A line already printed, and what's needed to find it again.
#[derive(Clone, Copy, Debug)]
pub(super) struct Printed {
    /// How many lines came before it
    index: u64,
    /// The terminal's width when it was printed
    width: Option<u16>,
}

/// The terminal or pipe the view prints to.
pub(super) struct Screen {
    /// Whether lines may be cut to the width and redrawn
    terminal: bool,
    /// Lines printed so far
    printed: u64,
    /// Where a test's lines go instead of stdout
    recorded: Option<Vec<String>>,
}

impl Screen {
    /// A terminal when stdout is one.
    pub(super) fn detect() -> Self {
        use std::io::IsTerminal;
        Self { terminal: std::io::stdout().is_terminal(), printed: 0, recorded: None }
    }

    /// A pipe that keeps its lines for a test to read.
    #[cfg(test)]
    pub(super) fn recording() -> Self {
        Self { terminal: false, printed: 0, recorded: Some(Vec::new()) }
    }

    /// What a [`Screen::recording`] was given.
    #[cfg(test)]
    pub(super) fn lines(&self) -> &[String] {
        self.recorded.as_deref().unwrap_or_default()
    }

    /// Prints `line` as one row, and says where it went.
    pub(super) fn print(&mut self, line: &str) -> Printed {
        let width = self.width();
        let printed = Printed { index: self.printed, width };
        self.printed += 1;
        if let Some(recorded) = &mut self.recorded {
            recorded.push(line.to_string());
            return printed;
        }
        match width {
            Some(cells) => outln!("{}", fit(line, usize::from(cells))),
            None => outln!("{line}"),
        }
        printed
    }

    /// Redraws an earlier line in place. False, and nothing written, when it can't be trusted to
    /// land on the same row: not a terminal, a resize since, or scrolled past the top.
    pub(super) fn rewrite(&mut self, at: Printed, line: &str) -> bool {
        let Some(cells) = self.width().filter(|now| Some(*now) == at.width) else { return false };
        let up = self.printed - at.index;
        let rows = crossterm::terminal::size().map_or(0, |(_, rows)| u64::from(rows));
        if up == 0 || up >= rows {
            return false;
        }
        // Up to the line, clear it, draw, then back down to where the next line goes
        out!("\x1b[{up}A\r\x1b[2K{}\x1b[{up}B\r", fit(line, usize::from(cells)));
        true
    }

    fn width(&self) -> Option<u16> {
        match self.terminal {
            true => crossterm::terminal::size().ok().map(|(cols, _)| cols),
            false => None,
        }
    }
}

/// `line` cut to fit in `width` cells, so it never wraps. Escape sequences pass through uncounted,
/// and a cut line ends in `…` and, if it had color, a reset.
pub(super) fn fit(line: &str, width: usize) -> String {
    if visible_width(line) <= width {
        return line.to_string();
    }
    let (cut, _) = scan(line, width.saturating_sub(1));
    let reset = if line.contains('\x1b') { "\x1b[0m" } else { "" };
    format!("{cut}…{reset}")
}

/// Where the scanner is: in text, just past an escape, or inside a `CSI` sequence.
#[derive(Clone, Copy)]
enum Scan {
    Text,
    Escape,
    Csi,
}

/// `line` up to `budget` visible cells, with every escape sequence kept, and the cells it used.
fn scan(line: &str, budget: usize) -> (String, usize) {
    let mut kept = String::with_capacity(line.len());
    let mut used = 0;
    let mut state = Scan::Text;
    for ch in line.chars() {
        state = match (state, ch) {
            (Scan::Text, '\x1b') => Scan::Escape,
            (Scan::Escape, '[') => Scan::Csi,
            (Scan::Escape, _) => Scan::Text,
            (Scan::Csi, '@'..='~') => Scan::Text,
            (Scan::Csi, _) => Scan::Csi,
            (Scan::Text, text) => {
                let cells = text.width().unwrap_or(0);
                if used + cells > budget {
                    break;
                }
                used += cells;
                Scan::Text
            }
        };
        kept.push(ch);
    }
    (kept, used)
}

fn visible_width(line: &str) -> usize {
    scan(line, usize::MAX).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_fits_is_left_alone() {
        assert_eq!(fit("○ read a.rs", 40), "○ read a.rs");
        assert_eq!(fit("exactly", 7), "exactly");
    }

    /// One cell short of the width plus the ellipsis is the width, so the row never wraps.
    #[test]
    fn a_long_line_is_cut_to_the_width() {
        assert_eq!(fit("abcdefghij", 6), "abcde…");
        assert_eq!(visible_width(&fit("abcdefghij", 6)), 6);
    }

    /// Escapes take no cells: a colored line that fits isn't cut, and a cut one is reset.
    #[test]
    fn escapes_are_not_counted() {
        let green = "\x1b[32m✓ read\x1b[39m a.rs";
        assert_eq!(visible_width(green), "✓ read a.rs".chars().count());
        assert_eq!(fit(green, 11), green);
        assert_eq!(fit(green, 5), "\x1b[32m✓ re…\x1b[0m");
    }

    /// A wide character counts as the two cells it takes.
    #[test]
    fn wide_characters_count_double() {
        assert_eq!(visible_width("日本"), 4);
        assert_eq!(fit("日本語", 5), "日本…");
    }

    /// A pipe never redraws: there's no row to go back to.
    #[test]
    fn a_pipe_never_redraws() {
        let mut screen = Screen::recording();
        let line = screen.print("○ read a.rs");
        assert!(!screen.rewrite(line, "✓ read a.rs"));
    }
}
