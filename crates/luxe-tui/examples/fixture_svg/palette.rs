//! Palette pictures — one scenario drawn once for every theme, its colors as CSS variables.

use super::capture::render;
use super::color::hex;
use super::fail;
use super::svg::{canvas_color, svg};
use luxe_tui::fixture::FixtureScenario;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The first byte of a placeholder color. Only placeholders are ever in the buffer that gets
/// drawn, so nothing real can be mistaken for one.
const SENTINEL: u8 = 0x0f;

/// The scenario drawn once for every theme in `themes`: each color a `var(--pN, first)`, plus what
/// every `N` is in every theme.
///
/// Twelve themes lay the same cells out identically, so what differs between their pictures is
/// only which color goes where. Each cell's colors are keyed on what they are across *all* the
/// themes at once, and the key becomes a placeholder color in one buffer that the ordinary
/// exporter then draws. That is also what keeps the background runs right: two neighbors merge into
/// one rectangle only when they match in every theme, where merging per theme and comparing the
/// results would have given different shapes to the same picture.
pub(super) fn palette_svg(
    themes: &[String],
    scenario: FixtureScenario,
    cols: u16,
    rows: u16,
) -> (String, BTreeMap<String, BTreeMap<String, String>>) {
    if themes.is_empty() {
        fail("--palette wants at least one theme");
    }
    let drawn: Vec<(Buffer, String)> =
        themes.iter().map(|theme| render(theme, scenario, None, cols, rows)).collect();
    let (first, _) = &drawn[0];
    for (theme, (buffer, _)) in themes.iter().zip(&drawn).skip(1) {
        let same = first.content().iter().zip(buffer.content()).all(|(left, right)| {
            left.symbol() == right.symbol() && left.modifier == right.modifier
        });
        if !same {
            fail(&format!("{theme} lays {scenario:?} out differently, so no one picture fits it"));
        }
    }
    let canvases: Vec<String> = drawn.iter().map(|(buffer, _)| canvas_color(buffer)).collect();

    // One key per distinct "this color in every theme", numbered in the order the cells meet them
    let mut keys: BTreeMap<Vec<String>, u16> = BTreeMap::new();
    let mut order: Vec<Vec<String>> = Vec::new();
    let mut key_of = |key: Vec<String>| -> u16 {
        if let Some(index) = keys.get(&key) {
            return *index;
        }
        let index = u16::try_from(order.len()).unwrap_or_else(|_| fail("too many colors"));
        keys.insert(key.clone(), index);
        order.push(key);
        index
    };
    let placeholder = |index: u16| {
        let [high, low] = index.to_be_bytes();
        Color::Rgb(SENTINEL, high, low)
    };
    let mut merged = first.clone();
    for (position, cell) in merged.content.iter_mut().enumerate() {
        let fg: Vec<String> = drawn
            .iter()
            .map(|(buffer, ink)| hex(buffer.content[position].fg).unwrap_or_else(|| ink.clone()))
            .collect();
        let bg: Vec<String> = drawn
            .iter()
            .zip(&canvases)
            .map(|((buffer, _), canvas)| {
                hex(buffer.content[position].bg).unwrap_or_else(|| canvas.clone())
            })
            .collect();
        cell.set_fg(placeholder(key_of(fg)));
        cell.set_bg(placeholder(key_of(bg)));
    }

    let first_values: Vec<&str> = order.iter().map(|key| key[0].as_str()).collect();
    let document = with_variables(&svg(&merged), &first_values);
    let mut values = BTreeMap::new();
    for (column, theme) in themes.iter().enumerate() {
        let table =
            order.iter().enumerate().map(|(index, key)| (format!("p{index}"), key[column].clone()));
        values.insert(theme.clone(), table.collect());
    }
    (document, values)
}

/// Every placeholder in an attribute swapped for `var(--pN, #first)`.
///
/// Only attribute values are touched (`="#0f…"`), because a picture of a terminal can have a hex
/// color in its *text*, and that one is somebody's code.
pub(super) fn with_variables(document: &str, first: &[&str]) -> String {
    let marker = format!("=\"#{SENTINEL:02x}");
    let mut out = String::with_capacity(document.len() + document.len() / 4);
    let mut rest = document;
    while let Some(at) = rest.find(&marker) {
        let (before, tail) = rest.split_at(at);
        out.push_str(before);
        let digits = &tail[marker.len()..];
        let index = digits
            .get(..4)
            .filter(|_| digits.as_bytes().get(4) == Some(&b'"'))
            .and_then(|hex| usize::from_str_radix(hex, 16).ok());
        match index {
            Some(index) => {
                let fallback = first.get(index).copied().unwrap_or("#000000");
                let _ = write!(out, "=\"var(--p{index}, {fallback})\"");
                rest = &digits[5..];
            }
            None => {
                out.push_str(&marker);
                rest = digits;
            }
        }
    }
    out.push_str(rest);
    out
}
