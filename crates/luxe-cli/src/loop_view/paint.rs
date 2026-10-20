//! Paint — how a loop line looks: the guides that lead it, and its colors when there are any.
//!
//! The guides work like an editor's indent guides. A phase draws a heavy rail down the left, in
//! a color of its own; an iteration draws a light one inside it, opened with `┌` and closed with
//! `└`. Scrolled back through a long run, or seen in a minimap, each phase is a band of one color
//! and each iteration a bracket within it. Colors come from the terminal's own sixteen, so its
//! theme picks the shades, and the guides are plain characters, so a log without color keeps them.

use crossterm::style::{Color, Stylize};

/// Phase rail colors, cycled by phase number so two neighboring phases never share one.
const PHASE_COLORS: [Color; 4] = [Color::Cyan, Color::Magenta, Color::Blue, Color::Yellow];

/// Where a line sits in the run, which decides the guides leading it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Depth {
    /// Before the run has a phase
    Run,
    /// Inside a phase, between iterations
    Phase(u32),
    /// Inside an iteration of a phase.
    ///
    /// Args: the phase, and which edge of the iteration the line is.
    Iteration(u32, Edge),
}

/// Which part of an iteration a line is: its opening, its body, or its closing line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Edge {
    /// `┌`, the iteration's first line
    Open,
    /// `│`, every line between
    Within,
    /// `└`, the turn's summary
    Close,
}

/// How a tool call has gone so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CallState {
    /// Started, no result yet
    Running,
    /// Ended without an error
    Succeeded,
    /// Ended in an error
    Failed,
}

/// Draws loop lines, in color or not.
#[derive(Clone, Copy, Debug)]
pub(super) struct Paint {
    color: bool,
}

impl Paint {
    /// Color when stdout is a terminal that wants it: not `NO_COLOR`, not `TERM=dumb`.
    pub(super) fn detect() -> Self {
        use std::io::IsTerminal;
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let dumb = std::env::var("TERM").is_ok_and(|term| term == "dumb");
        Self { color: std::io::stdout().is_terminal() && !no_color && !dumb }
    }

    /// No color at all, as a pipe or a log gets.
    #[cfg(test)]
    pub(super) fn plain() -> Self {
        Self { color: false }
    }

    /// Whether this paints in color.
    pub(super) fn in_color(self) -> bool {
        self.color
    }

    /// The guides for a line at `depth`, each followed by a space.
    pub(super) fn guides(self, depth: Depth) -> String {
        match depth {
            Depth::Run => String::new(),
            Depth::Phase(phase) => format!("{} ", self.rail(phase)),
            Depth::Iteration(phase, edge) => {
                format!("{} {} ", self.rail(phase), self.dim(edge_glyph(edge)))
            }
        }
    }

    /// The line that opens a phase: its number in the phase's color.
    pub(super) fn phase_heading(self, phase: u32) -> String {
        let text = format!("phase {phase}");
        match self.color {
            true => text.with(phase_color(phase)).bold().to_string(),
            false => text,
        }
    }

    /// A tool call: a mark and the tool's name colored by how it went, then what it was asked.
    /// A failure says so in words too, so the color never carries it alone.
    pub(super) fn tool(self, name: &str, label: &str, state: CallState) -> String {
        let (mark, color) = match state {
            CallState::Running => ("○", None),
            CallState::Succeeded => ("✓", Some(Color::Green)),
            CallState::Failed => ("✗", Some(Color::Red)),
        };
        let head = format!("{mark} {name}");
        let head = match (self.color, color) {
            (true, Some(color)) => head.with(color).bold().to_string(),
            (true, None) => head.bold().to_string(),
            (false, _) => head,
        };
        let failed = match state {
            CallState::Failed => format!(" · {}", self.colored("failed", Color::Red)),
            _ => String::new(),
        };
        format!("{head} {label}").trim_end().to_string() + &failed
    }

    /// Secondary text: a turn's summary, the iteration rail.
    pub(super) fn dim(self, text: &str) -> String {
        match self.color {
            true => text.dim().to_string(),
            false => text.to_string(),
        }
    }

    fn colored(self, text: &str, color: Color) -> String {
        match self.color {
            true => text.with(color).to_string(),
            false => text.to_string(),
        }
    }

    fn rail(self, phase: u32) -> String {
        self.colored("┃", phase_color(phase))
    }
}

fn phase_color(phase: u32) -> Color {
    PHASE_COLORS[phase as usize % PHASE_COLORS.len()]
}

fn edge_glyph(edge: Edge) -> &'static str {
    match edge {
        Edge::Open => "┌",
        Edge::Within => "│",
        Edge::Close => "└",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_depth_draws_its_own_guides() {
        let paint = Paint::plain();
        assert_eq!(paint.guides(Depth::Run), "");
        assert_eq!(paint.guides(Depth::Phase(2)), "┃ ");
        assert_eq!(paint.guides(Depth::Iteration(2, Edge::Open)), "┃ ┌ ");
        assert_eq!(paint.guides(Depth::Iteration(2, Edge::Within)), "┃ │ ");
        assert_eq!(paint.guides(Depth::Iteration(2, Edge::Close)), "┃ └ ");
    }

    /// Without color a failure still reads as one, and a success has no word to need.
    #[test]
    fn a_failed_call_says_so_in_words() {
        let paint = Paint::plain();
        assert_eq!(paint.tool("read", "a.rs", CallState::Running), "○ read a.rs");
        assert_eq!(paint.tool("read", "a.rs", CallState::Succeeded), "✓ read a.rs");
        assert_eq!(
            paint.tool("bash", "cargo test", CallState::Failed),
            "✗ bash cargo test · failed"
        );
        assert_eq!(paint.tool("todo", "", CallState::Succeeded), "✓ todo", "no trailing space");
    }

    /// The tool's name takes the result's color; a plain view has no escapes at all.
    #[test]
    fn the_name_is_colored_by_the_result() {
        let paint = Paint { color: true };
        let ok = paint.tool("read", "a.rs", CallState::Succeeded);
        let failed = paint.tool("read", "a.rs", CallState::Failed);
        assert!(ok.contains(&"✓ read".with(Color::Green).bold().to_string()), "{ok:?}");
        assert!(failed.contains(&"✗ read".with(Color::Red).bold().to_string()), "{failed:?}");
        assert!(!Paint::plain().tool("read", "a.rs", CallState::Failed).contains('\x1b'));
    }

    /// Neighboring phases never share a rail color, so a phase boundary shows in a minimap.
    #[test]
    fn neighboring_phases_differ() {
        for phase in 0..8 {
            assert_ne!(phase_color(phase), phase_color(phase + 1), "{phase}");
        }
    }
}
