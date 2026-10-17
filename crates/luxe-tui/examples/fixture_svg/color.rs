//! Color — a ratatui color as hex, through the ANSI and indexed palettes, and text attributes.

use ratatui::style::{Color, Modifier};

/// A ratatui color as `#rrggbb`, or `None` when the terminal would have chosen it.
pub(super) fn hex(color: Color) -> Option<String> {
    let (red, green, blue) = match color {
        Color::Rgb(red, green, blue) => (red, green, blue),
        Color::Indexed(index) => indexed_rgb(index),
        Color::Reset => return None,
        // The sixteen named colors belong to the terminal's palette. Luxe's own themes are pure
        // RGB, so this is only reached by a user theme; xterm's defaults are the least surprising
        // stand-in available.
        named => indexed_rgb(named_index(named)?),
    };
    Some(format!("#{red:02x}{green:02x}{blue:02x}"))
}

fn named_index(color: Color) -> Option<u8> {
    Some(match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        _ => return None,
    })
}

/// The RGB an xterm-256 index stands for: sixteen system colors, a 6×6×6 cube, then a gray ramp.
fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    const SYSTEM: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    if index < 16 {
        return SYSTEM[index as usize];
    }
    if index >= 232 {
        let level = 8 + (index - 232) * 10;
        return (level, level, level);
    }
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let index = index - 16;
    (LEVELS[(index / 36) as usize], LEVELS[(index % 36 / 6) as usize], LEVELS[(index % 6) as usize])
}

/// Ratatui modifiers as SVG text attributes.
pub(super) fn attributes(modifier: Modifier) -> String {
    let mut out = String::new();
    if modifier.contains(Modifier::BOLD) {
        out.push_str(" font-weight=\"700\"");
    }
    if modifier.contains(Modifier::ITALIC) {
        out.push_str(" font-style=\"italic\"");
    }
    if modifier.contains(Modifier::DIM) {
        out.push_str(" opacity=\"0.6\"");
    }
    match (modifier.contains(Modifier::UNDERLINED), modifier.contains(Modifier::CROSSED_OUT)) {
        (true, true) => out.push_str(" text-decoration=\"underline line-through\""),
        (true, false) => out.push_str(" text-decoration=\"underline\""),
        (false, true) => out.push_str(" text-decoration=\"line-through\""),
        (false, false) => {}
    }
    out
}
