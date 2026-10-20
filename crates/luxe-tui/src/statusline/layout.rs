//! Statusline layout — fitting the segments to the bar, banding each cluster, and placing them.

use super::{surface_of, Ink, Palette, Segment, SegmentKind, Surface, SEGMENT_GAP, SLAB_GAP};
use crate::theme::styles::Depth;
use crate::widgets::{fade, ramp, Fade, FADE_CELLS};
use luxe_core::config::StatuslineStyle;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// Drops segments by priority until both clusters fit the width, then a hard cap trims anything
/// still over budget so `status_line` never returns a line wider than the bar.
pub(super) fn fit_segments(
    left: &mut Vec<Segment>,
    right: &mut Vec<Segment>,
    width: usize,
    shape: Shape,
    context_replacement: bool,
) {
    // Least actionable first. Token counts are trivia next to the percentage that says whether the
    // window is about to run out; a path repeats the project name; and of the two identities, the
    // project is the one that tells two luxe windows apart, so the branch goes first.
    let removal_order: &[SegmentKind] = if context_replacement {
        &[
            SegmentKind::Cwd,
            SegmentKind::Tokens,
            SegmentKind::Cost,
            SegmentKind::Todo,
            SegmentKind::Changes,
            SegmentKind::PullRequest,
            SegmentKind::Branch,
            SegmentKind::Project,
            SegmentKind::Context,
            SegmentKind::Thinking,
            SegmentKind::Model,
            SegmentKind::Hint,
        ]
    } else {
        &[
            SegmentKind::Cwd,
            SegmentKind::Tokens,
            SegmentKind::Activity,
            SegmentKind::Cost,
            // The plan goes before the change, which is the one order here that is not about
            // importance. This is the only module on the bar that is a *control*, it is short and
            // fixed where the plan is long and variable, and it appears exactly where the dock is
            // absent — so dropping it first would take away the only visible way in to the change
            // on the one layout that has no other.
            SegmentKind::Todo,
            SegmentKind::Changes,
            // Before the branch it belongs to: the branch is where you are, the pull request is
            // news about it, and the dock and `/pr` both still carry it
            SegmentKind::PullRequest,
            SegmentKind::Branch,
            SegmentKind::Project,
            SegmentKind::Context,
            SegmentKind::Thinking,
            SegmentKind::Model,
        ]
    };
    for &kind in removal_order {
        if segments_width(left, right, shape) <= width {
            break;
        }
        remove_kind(left, kind);
        remove_kind(right, kind);
    }

    // Safety net for anything outside the priority list: trim from the right cluster then the left
    // until the line fits, so the composed line can never overflow.
    while segments_width(left, right, shape) > width && right.pop().is_some() {}
    while segments_width(left, right, shape) > width && left.pop().is_some() {}
}

fn remove_kind(segments: &mut Vec<Segment>, kind: SegmentKind) {
    if let Some(index) = segments.iter().position(|segment| segment.kind == kind) {
        segments.remove(index);
    }
}

/// Cells between two adjacent segments that are not in the same band.
fn gap(style: StatuslineStyle) -> usize {
    match style {
        StatuslineStyle::Slab => SLAB_GAP,
        StatuslineStyle::Flat | StatuslineStyle::Unknown => SEGMENT_GAP,
    }
}

/// Cells the two clusters must never come closer than.
///
/// Two, because either end can be a bare run of text: a single cell between the plan and the context
/// meter reads as one sentence, and there is no edge between them to say otherwise.
const CLUSTER_GAP: usize = 2;

/// Total cells the two clusters occupy, including the space between them.
fn segments_width(left: &[Segment], right: &[Segment], shape: Shape) -> usize {
    let between = if left.is_empty() || right.is_empty() { 0 } else { CLUSTER_GAP };
    lay(left, Side::Left, shape).width + lay(right, Side::Right, shape).width + between
}

/// Which end of the bar a cluster is anchored to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Left,
    Right,
}

/// Everything laying a cluster out depends on besides its segments.
#[derive(Debug, Clone, Copy)]
pub(super) struct Shape {
    pub(super) palette: Palette,
    pub(super) style: StatuslineStyle,
}

/// A cluster as drawn: its cells, and where each module landed in them.
pub(super) struct Laid {
    pub(super) spans: Vec<Span<'static>>,
    width: usize,
    /// `(kind, column, cells)` for every segment, pads included
    pub(super) places: Vec<(SegmentKind, usize, usize)>,
}

impl Laid {
    /// Where `kind` sits, as `(column, width)` from the cluster's own left edge.
    pub(super) fn place(&self, kind: SegmentKind) -> Option<(u16, u16)> {
        let (_, column, cells) = self.places.iter().find(|(found, ..)| *found == kind)?;
        Some((u16::try_from(*column).ok()?, u16::try_from(*cells).ok()?))
    }
}

/// Lays one cluster out, which is also how it is measured.
///
/// One function for both, because the fit, the paint and the click region all need the same
/// answer, and three copies of the arithmetic drift by exactly the frame of whatever changed last.
///
/// Consecutive slabs form a **band**. A band's edge against the screen's side is flush, and every
/// other edge fades into the canvas. Inside a band, each module has a pad of its own color either
/// side of its words; neighbors of the same depth are split by a dim `·`, and a change of depth
/// ramps across [`FADE_CELLS`] from one color to the next, so the band reads as a gradient rather
/// than as a row of pills. Every change of color gets the same three steps, the fade into the
/// canvas included, and the fit drops a module before it thins one. A bare module rides the canvas
/// a gap clear of whatever is beside it.
pub(super) fn lay(cluster: &[Segment], side: Side, shape: Shape) -> Laid {
    let Shape { palette, style } = shape;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut places = Vec::with_capacity(cluster.len());
    let mut column = 0usize;
    let push = |spans: &mut Vec<Span<'static>>, span: Span<'static>, column: &mut usize| {
        *column += UnicodeWidthStr::width(span.content.as_ref());
        spans.push(span);
    };
    let surface = |segment: &Segment| match surface_of(segment.kind, style) {
        Surface::Slab(depth) => Some(slab_fill(depth, palette)),
        Surface::Bare => None,
    };

    let mut index = 0;
    while index < cluster.len() {
        if index > 0 {
            push(&mut spans, canvas_cells(gap(style), palette), &mut column);
        }
        let Some(first) = surface(&cluster[index]) else {
            let start = column;
            for span in segment_body(&cluster[index], palette, style) {
                push(&mut spans, span, &mut column);
            }
            places.push((cluster[index].kind, start, column - start));
            index += 1;
            continue;
        };

        // A band: every slab from here to the next bare module
        let end = (index..cluster.len())
            .find(|&at| surface(&cluster[at]).is_none())
            .unwrap_or(cluster.len());
        let flush_start = side == Side::Left && index == 0;
        let flush_end = side == Side::Right && end == cluster.len();
        if !flush_start {
            for span in fade(first, palette.slab.canvas, FADE_CELLS, Fade::In) {
                push(&mut spans, span, &mut column);
            }
        }
        let mut previous: Option<Color> = None;
        for segment in &cluster[index..end] {
            let fill = surface(segment).unwrap_or(first);
            if let Some(previous) = previous {
                let boundary = if previous == fill {
                    vec![Span::styled("\u{b7}", Style::new().fg(palette.muted).bg(fill))]
                } else {
                    ramp(previous, fill, FADE_CELLS)
                };
                for span in boundary {
                    push(&mut spans, span, &mut column);
                }
            }
            let start = column;
            let pad = Span::styled(" ", Style::new().bg(fill));
            push(&mut spans, pad.clone(), &mut column);
            for span in segment_body(segment, palette, style) {
                push(&mut spans, span, &mut column);
            }
            push(&mut spans, pad, &mut column);
            places.push((segment.kind, start, column - start));
            previous = Some(fill);
        }
        if !flush_end {
            let last = previous.unwrap_or(first);
            for span in fade(last, palette.slab.canvas, FADE_CELLS, Fade::Out) {
                push(&mut spans, span, &mut column);
            }
        }
        index = end;
    }
    Laid { spans, width: column, places }
}

fn canvas_cells(cells: usize, palette: Palette) -> Span<'static> {
    Span::styled(" ".repeat(cells), Style::new().bg(palette.background))
}

/// The color a slab at `depth` is filled with.
fn slab_fill(depth: Depth, palette: Palette) -> Color {
    match depth {
        Depth::Hero => palette.slab.hero,
        depth => palette.slab.at(depth),
    }
}

/// Lays the left cluster flush-left and the right cluster flush-right, spacer in between.
pub(super) fn compose_line(
    left: Laid,
    right: Laid,
    width: usize,
    palette: Palette,
) -> Line<'static> {
    let fill = width.saturating_sub(left.width + right.width);
    let mut spans = left.spans;
    spans.push(canvas_cells(fill, palette));
    spans.extend(right.spans);
    Line::from(spans)
}

/// One segment's words, colored for what they sit on: ink on the accent, the foreground on a
/// receded slab, muted on the bar. A signal keeps its own color everywhere except the accent, which
/// is too bright to carry one legibly — and no segment that reaches it has one.
fn segment_body(segment: &Segment, palette: Palette, style: StatuslineStyle) -> Vec<Span<'static>> {
    let emphasis =
        if segment.emphasized { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() };
    let (fill, neutral, force_ink) = match surface_of(segment.kind, style) {
        Surface::Bare => (palette.background, palette.muted, false),
        Surface::Slab(Depth::Hero) => (palette.slab.hero, palette.slab.ink, true),
        Surface::Slab(depth) => (palette.slab.at(depth), palette.text, false),
    };
    segment
        .parts
        .iter()
        .map(|(text, ink)| {
            let color = match ink {
                Ink::Signal(color) if !force_ink => *color,
                Ink::Signal(_) | Ink::Neutral => neutral,
            };
            Span::styled(text.clone(), Style::new().fg(color).bg(fill).patch(emphasis))
        })
        .collect()
}
