//! Cells — the vocabulary the two bars share.
//!
//! The activity indicator and the fill gauge are the same bar, one moving and one standing still,
//! so both ask the same two questions: which step of a ramp does this cell get, and how does a run
//! of cells become spans? Both answers live here so neither widget owns them.

use ratatui::style::Style;
use ratatui::text::Span;

/// Where `offset` cells into a `span`-cell run falls on a ramp of `len` steps.
///
/// The ramp is stretched over the run, so offset 0 always gets step 0 and the run's last cell always
/// the final step, however short the run had to be.
pub(super) fn ramp_step(span: usize, offset: usize, len: usize) -> usize {
    let last = len.saturating_sub(1);
    if span <= 1 || last == 0 {
        return 0;
    }
    let position = offset as f32 / (span - 1) as f32;
    ((position * last as f32).round() as usize).min(last)
}

/// Appends one cell, extending the previous span when it wears the same style.
pub(super) fn push_cell(spans: &mut Vec<Span<'static>>, glyph: char, style: Style) {
    match spans.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push(glyph),
        _ => spans.push(Span::styled(glyph.to_string(), style)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn a_ramp_spans_its_run_end_to_end() {
        assert_eq!(ramp_step(5, 0, 10), 0, "the first cell takes the first step");
        assert_eq!(ramp_step(5, 4, 10), 9, "the last cell takes the last");
        // Monotonic in between, however the run and the ramp divide.
        for span in 1..12usize {
            let steps: Vec<usize> = (0..span).map(|at| ramp_step(span, at, 7)).collect();
            assert!(steps.windows(2).all(|pair| pair[0] <= pair[1]), "span {span}: {steps:?}");
            assert!(steps.iter().all(|&step| step < 7), "span {span}: {steps:?}");
        }
    }

    /// Degenerate runs and ramps resolve to the first step rather than dividing by zero.
    #[test]
    fn a_run_or_ramp_of_one_takes_the_first_step() {
        assert_eq!(ramp_step(1, 0, 10), 0);
        assert_eq!(ramp_step(0, 0, 10), 0);
        assert_eq!(ramp_step(8, 3, 1), 0);
        assert_eq!(ramp_step(8, 3, 0), 0);
    }

    #[test]
    fn like_styled_cells_share_a_span() {
        let (one, two) = (Style::default().fg(Color::Red), Style::default().fg(Color::Blue));
        let mut spans = Vec::new();
        for (glyph, style) in [('a', one), ('b', one), ('c', two), ('d', one)] {
            push_cell(&mut spans, glyph, style);
        }
        assert_eq!(spans.len(), 3, "{spans:?}");
        assert_eq!(spans[0].content.as_ref(), "ab");
        assert_eq!(spans[2].content.as_ref(), "d");
    }
}
