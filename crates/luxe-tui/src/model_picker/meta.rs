//! Model row metadata — the context window, reasoning and source notes after a model's name.

use super::ModelEntry;

/// The parenthesized metadata suffix for a model row (`  (200k ctx · reasoning · catalog)`),
/// or empty when the model carries none. Shared by the row renderer and the width calculation
/// so the fitted modal width matches what is drawn.
pub(super) fn meta_suffix(entry: &ModelEntry) -> String {
    let mut meta = Vec::new();
    if entry.info.context_window > 0 {
        meta.push(format!("{} ctx", human_tokens(entry.info.context_window)));
    }
    if entry.info.reasoning {
        meta.push("reasoning".to_string());
    }
    match (entry.configured, entry.unverified) {
        (_, true) => meta.push("unverified".to_string()),
        (false, false) => meta.push("catalog".to_string()),
        (true, false) => {}
    }
    if meta.is_empty() {
        String::new()
    } else {
        format!("  ({})", meta.join(" · "))
    }
}

/// `1.0M` / `200k` / `8192` — million-token windows read badly as `1000k`.
fn human_tokens(tokens: u32) -> String {
    match tokens {
        t if t >= 1_000_000 => format!("{:.1}M", t as f64 / 1_000_000.0),
        t if t >= 10_000 => format!("{}k", t / 1_000),
        t => t.to_string(),
    }
}
