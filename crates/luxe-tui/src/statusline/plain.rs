//! Plain — the status line's right-hand facts as words, for a surface that isn't a terminal.
//!
//! The phone app shows what the bar shows, so it takes the bar's own wording from here rather than
//! keeping a copy that could drift: the context reading and its ramp, the cost slot's choice
//! between a plan's allowance and the session's spend, the thinking level's short name. No slabs
//! and no truncation, which are the terminal's problems; glyphs are the Nerd Font marks, which the
//! app's font always has.

use super::segments::{context_text, cost_text};
use super::{context_percent, thinking_label};
use crate::glyphs::{GlyphTier, Icon};
use crate::theme::styles::ContextFill;
use luxe_core::provider::{PlanUsage, ThinkingLevel};
use luxe_core::usage::SessionUsage;
use luxe_core::ModelRef;

/// What a session last reported, each part absent until its event arrives.
#[derive(Debug, Default, Clone, Copy)]
pub struct Facts<'a> {
    /// The model answering, from `state_changed`
    pub model: Option<&'a ModelRef>,
    /// Its thinking level, from `state_changed`
    pub thinking: Option<ThinkingLevel>,
    /// Tokens in the window and the window's size, from `context_usage`
    pub context: Option<(u32, u32)>,
    /// A subscription's allowance, from `plan_usage`
    pub plan: Option<&'a PlanUsage>,
    /// The session's spend, from `session_usage`
    pub session: Option<&'a SessionUsage>,
}

/// One fact as the bar words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Which fact this is
    pub kind: Kind,
    /// The mark in front of it, if the bar draws one
    pub glyph: Option<&'static str>,
    /// The words
    pub label: String,
    /// How loudly to color it
    pub tone: Tone,
}

/// The facts, in the bar's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// How full the context window is
    Context,
    /// A plan's allowance or the session's spend
    Cost,
    /// The thinking level
    Thinking,
    /// The model
    Model,
}

/// How loudly a segment is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Body text
    Plain,
    /// The theme's warning color
    Warning,
    /// The theme's error color
    Error,
}

/// The facts the session has reported, worded and ordered as the bar's right-hand side.
pub fn segments(facts: &Facts<'_>) -> Vec<Segment> {
    [context(facts), cost(facts), thinking(facts), model(facts)].into_iter().flatten().collect()
}

fn context(facts: &Facts<'_>) -> Option<Segment> {
    let (used, window) = facts.context.filter(|(_, window)| *window > 0)?;
    let percent = context_percent(used, window);
    let tone = match ContextFill::at(percent) {
        ContextFill::Roomy => Tone::Plain,
        ContextFill::Filling => Tone::Warning,
        ContextFill::Full => Tone::Error,
    };
    Some(Segment {
        kind: Kind::Context,
        glyph: Some(Icon::Context.at(GlyphTier::Nerd)),
        label: context_text(percent, window),
        tone,
    })
}

fn cost(facts: &Facts<'_>) -> Option<Segment> {
    let label = cost_text(facts.plan, facts.session.unwrap_or(&SessionUsage::default()))?;
    Some(Segment { kind: Kind::Cost, glyph: None, label, tone: Tone::Plain })
}

fn thinking(facts: &Facts<'_>) -> Option<Segment> {
    let level = facts.thinking?;
    facts.model.filter(|model| !model.model.is_empty())?;
    Some(Segment {
        kind: Kind::Thinking,
        glyph: Some(Icon::Thinking.at(GlyphTier::Nerd)),
        label: thinking_label(&level).to_string(),
        tone: Tone::Plain,
    })
}

// Always there, as on the bar: with no model, saying so explains a refused turn
fn model(facts: &Facts<'_>) -> Option<Segment> {
    let (label, tone) = match facts.model.filter(|model| !model.model.is_empty()) {
        Some(model) => (model.display().to_string(), Tone::Plain),
        None => ("no model".to_string(), Tone::Warning),
    };
    Some(Segment { kind: Kind::Model, glyph: Some(Icon::Model.at(GlyphTier::Nerd)), label, tone })
}

#[cfg(test)]
mod tests;
