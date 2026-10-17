//! Cache stability — whether each request appends to the last or rewrites what a cache holds.

use std::fmt::Write as _;

use super::call::Call;

/// What changed from each request to the next, in the order of the log — one entry per
/// transition, empty where the prefix held.
pub(super) fn transitions(calls: &[Call]) -> Vec<Vec<String>> {
    calls.windows(2).map(|pair| prefix_changes(&pair[0], &pair[1])).collect()
}

/// Does the cacheable prefix actually stay identical between turns?
///
/// Every provider that caches does it by prefix: the tools and system prompt come first, so one
/// changed byte in either throws away the whole cached prefix — and cache pricing means that is a
/// silent bill, not an error. This is the check that catches volatile state (a git dirty flag, a
/// todo counter, a timestamp) sitting inside the cached region.
pub(super) fn cache_stability(out: &mut String, calls: &[Call], transitions: &[Vec<String>]) {
    if calls.len() < 2 {
        return;
    }
    let _ = writeln!(out, "## Cache prefix stability\n");
    let mut unstable = 0usize;
    for (pair, reasons) in calls.windows(2).zip(transitions) {
        let (before, after) = (&pair[0], &pair[1]);
        let verdict = if reasons.is_empty() {
            "stable".to_string()
        } else {
            unstable += 1;
            reasons.join(" · ")
        };
        let _ = writeln!(out, "  #{} → #{}: {}", before.index, after.index, verdict);
    }
    if unstable > 0 {
        let _ = writeln!(
            out,
            "\n  ▲ {unstable} transition(s) invalidate the cached prefix. Everything after the\n    \
             first changed byte has to be re-read (and re-billed) by the provider."
        );
    }
    let _ = writeln!(out);
}

/// Everything about `after` that a cache cannot reuse from `before`, in prefix order.
///
/// The conversation is checked as well as the prefix above it: a transcript is only cacheable if
/// each request *appends* to the last one, and a harness that edits history in flight —
/// re-truncating an old tool result, dropping a stale image — re-bills every token after the edit
/// at the cache-write rate with nothing to show for it.
pub(super) fn prefix_changes(before: &Call, after: &Call) -> Vec<String> {
    let mut reasons = Vec::new();
    if before.system() != after.system() {
        reasons.push(format!(
            "system CHANGED{}",
            first_difference(
                before.system().unwrap_or_default(),
                after.system().unwrap_or_default()
            )
        ));
    }
    // Only the loaded tools: a deferred definition sits outside the cached prefix, so adding or
    // changing one costs nothing
    if before.loaded_tools() != after.loaded_tools() {
        reasons.push("tools CHANGED".to_string());
    }
    let (was, now) = (before.cached_messages(), after.cached_messages());
    if let Some(at) = was.iter().zip(now.iter()).position(|(a, b)| a != b) {
        let resent: usize = was[at..].iter().map(|m| m.chars().count()).sum();
        reasons.push(format!(
            "conversation REWRITTEN at message {at}/{} (~{} tok re-sent){}",
            was.len(),
            resent / 4,
            first_difference(&was[at], &now[at])
        ));
    }
    reasons
}

/// Drop every `cache_control` marker from a message, however deeply nested.
pub(super) fn strip_cache_control(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("cache_control");
            map.values_mut().for_each(strip_cache_control);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_cache_control),
        _ => {}
    }
}

/// Where two strings first differ, as a short quoted excerpt.
pub(super) fn first_difference(before: &str, after: &str) -> String {
    let common = before.char_indices().zip(after.chars()).take_while(|((_, a), b)| a == b).count();
    let excerpt = |text: &str| -> String {
        text.chars().skip(common).take(48).collect::<String>().replace('\n', "⏎")
    };
    format!(" at char {common}: {:?} → {:?}", excerpt(before), excerpt(after))
}
