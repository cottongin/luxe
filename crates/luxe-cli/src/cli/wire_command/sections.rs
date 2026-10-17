//! Report sections — sizes, composition, duplication and streaming, one per question the log answers.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::call::Call;

/// Characters per token, measured from the run: total request characters over reported prompt
/// tokens. Falls back to the usual ~4 when nothing reported usage.
pub(super) fn chars_per_token(calls: &[Call]) -> f64 {
    let (chars, tokens) =
        calls.iter().fold((0usize, 0u64), |(chars, tokens), call| match call.prompt_tokens() {
            Some(reported) if reported > 0 => (chars + call.context_chars(), tokens + reported),
            _ => (chars, tokens),
        });
    if tokens == 0 {
        return 4.0;
    }
    (chars as f64 / tokens as f64).max(1.0)
}

pub(super) fn tokens(chars: usize, ratio: f64) -> u64 {
    (chars as f64 / ratio).round() as u64
}

pub(super) fn size(chars: usize, ratio: f64, total: usize) -> String {
    let share = if total == 0 { 0.0 } else { chars as f64 / total as f64 * 100.0 };
    format!("{:>7} ch  ~{:>6} tok  {:>5.1}%", chars, tokens(chars, ratio), share)
}

pub(super) fn overview(out: &mut String, calls: &[Call], ratio: f64, verdict: &[String]) {
    let _ = writeln!(out, "# Wire audit\n");
    let _ = writeln!(out, "{}\n", verdict.join("\n"));
    let _ = writeln!(
        out,
        "{} provider call(s) · {} · model {} · ~{:.2} chars/token (measured)\n{}\n",
        calls.len(),
        calls[0].provider,
        calls[0].model(),
        ratio,
        calls[0].url,
    );
    // `written` sits beside `cached` because the pair is the whole story and either alone misleads:
    // a prefix rewritten every turn reads most of itself straight back, so a healthy-looking
    // `cached` column can sit on top of a bill that is almost entirely writes.
    let _ = writeln!(
        out,
        "{:<5} {:>8} {:>9} {:>8} {:>8} {:>8} {:>9} {:>8}",
        "call", "req ch", "prompt", "cached", "written", "output", "first ms", "total ms"
    );
    for call in calls {
        let usage = call.response.as_ref().and_then(|r| r.get("usage"));
        let field = |name: &str| {
            usage.and_then(|u| u.get(name)).and_then(serde_json::Value::as_u64).unwrap_or(0)
        };
        let first = call
            .response
            .as_ref()
            .and_then(|r| r.get("first_delta_ms"))
            .and_then(serde_json::Value::as_u64);
        let elapsed = call
            .response
            .as_ref()
            .and_then(|r| r.get("elapsed_ms"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let _ = writeln!(
            out,
            "{:<5} {:>8} {:>9} {:>8} {:>8} {:>8} {:>9} {:>8}",
            call.index,
            call.body_chars(),
            field("prompt_tokens"),
            field("cache_read_tokens"),
            field("cache_write_tokens"),
            field("completion_tokens"),
            first.map(|ms| ms.to_string()).unwrap_or_else(|| "-".into()),
            elapsed,
        );
    }
    let _ = writeln!(out);
}

/// Where the request's characters actually go, for the first call (the leanest one) and the last.
pub(super) fn composition(out: &mut String, calls: &[Call], ratio: f64, per_tool: bool) {
    for (label, call) in [("first", calls.first()), ("last", calls.last())] {
        let Some(call) = call else { continue };
        if calls.len() == 1 && label == "last" {
            continue;
        }
        let total = call.body_chars();
        let _ = writeln!(out, "## Composition — {label} call (#{}) · {total} chars\n", call.index);

        let system = call.system().unwrap_or_default();
        let system_chars = system.chars().count();
        let tool_entries = call.tool_entries();
        let tool_chars: usize = tool_entries.iter().map(|(_, _, json)| json.chars().count()).sum();
        let deferred_entries = call.deferred_entries();
        let deferred_chars: usize =
            deferred_entries.iter().map(|(_, _, json)| json.chars().count()).sum();
        let message_chars: usize = call
            .messages()
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
            .map(|m| serde_json::to_string(m).map(|s| s.chars().count()).unwrap_or(0))
            .sum();
        let other =
            total.saturating_sub(system_chars + tool_chars + deferred_chars + message_chars);

        let _ = writeln!(out, "  system prompt   {}", size(system_chars, ratio, total));
        for (heading, chars) in system_sections(system) {
            let _ = writeln!(out, "    {:<22} {}", heading, size(chars, ratio, total));
        }
        let _ = writeln!(
            out,
            "  tool schemas    {}  ({} tools)",
            size(tool_chars, ratio, total),
            tool_entries.len()
        );
        if per_tool {
            let mut sorted = tool_entries.clone();
            sorted.sort_by_key(|(_, _, json)| std::cmp::Reverse(json.chars().count()));
            for (name, _, json) in sorted {
                let _ =
                    writeln!(out, "    {:<22} {}", name, size(json.chars().count(), ratio, total));
            }
        }
        // On the wire but not in the context: the provider bills none of it until a
        // `tool_reference` loads one, so it's counted apart rather than as overhead
        if !deferred_entries.is_empty() {
            let _ = writeln!(
                out,
                "  deferred tools  {:>7} ch  ({} tools, outside the context until loaded)",
                deferred_chars,
                deferred_entries.len()
            );
        }
        let _ = writeln!(out, "  conversation    {}", size(message_chars, ratio, total));
        for message in call.messages() {
            let role = message.get("role").and_then(|r| r.as_str()).unwrap_or("item");
            if role == "system" {
                continue;
            }
            let chars = serde_json::to_string(&message).map(|s| s.chars().count()).unwrap_or(0);
            let _ = writeln!(out, "    {:<22} {}", role, size(chars, ratio, total));
        }
        let _ = writeln!(out, "  wire overhead   {}\n", size(other, ratio, total));
    }
}

/// Split a system prompt into its `## Heading` sections with their sizes.
pub(super) fn system_sections(system: &str) -> Vec<(String, usize)> {
    let mut sections: Vec<(String, usize)> = Vec::new();
    let mut current = String::from("(preamble)");
    let mut chars = 0usize;
    for line in system.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if chars > 0 {
                sections.push((current.clone(), chars));
            }
            current = heading.trim().to_string();
            chars = line.chars().count() + 1;
        } else {
            chars += line.chars().count() + 1;
        }
    }
    if chars > 0 {
        sections.push((current, chars));
    }
    sections
}

/// Content sent more than once in a single request.
pub(super) fn duplication(out: &mut String, calls: &[Call]) {
    let Some(call) = calls.last() else { return };
    let _ = writeln!(out, "## Duplication — last call (#{})\n", call.index);
    let system = call.system().unwrap_or_default();
    let mut findings = Vec::new();

    // A tool's description repeated in the prompt is the classic double-send. Scoped to the
    // `## Tools` section for the re-listing check: `**git**:` also appears in the environment
    // line and in any AGENTS.md that uses bold labels, and flagging those is noise.
    let tools_section = call.tools_section();
    for (name, description, _) in call.tool_entries() {
        let head: String = description.chars().take(60).collect();
        if head.chars().count() > 20 && system.contains(head.trim()) {
            findings.push(format!("tool `{name}` description also appears in the system prompt"));
        }
        if tools_section.contains(&format!("**{name}**:")) {
            findings.push(format!("tool `{name}` is re-listed in the system prompt"));
        }
    }

    // The same tool result twice means the model paid for the same file/output twice.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for message in call.messages() {
        let text = serde_json::to_string(&message).unwrap_or_default();
        if text.chars().count() < 200 {
            continue;
        }
        *seen.entry(text).or_default() += 1;
    }
    for (text, count) in seen.iter().filter(|(_, count)| **count > 1) {
        let head: String = text.chars().take(70).collect();
        findings
            .push(format!("a {}-char message is sent {count}× — {head}…", text.chars().count()));
    }

    if findings.is_empty() {
        let _ = writeln!(out, "  none found\n");
    } else {
        for finding in findings {
            let _ = writeln!(out, "  ▲ {finding}");
        }
        let _ = writeln!(out);
    }
}

/// Did text actually arrive incrementally, or all at once at the end?
pub(super) fn streaming(out: &mut String, calls: &[Call]) {
    let _ = writeln!(out, "## Streaming\n");
    for call in calls {
        let Some(response) = &call.response else { continue };
        let arrival: Vec<(u128, usize)> = response
            .get("arrival")
            .and_then(|a| a.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|entry| {
                        let pair = entry.as_array()?;
                        Some((pair.first()?.as_u64()? as u128, pair.get(1)?.as_u64()? as usize))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if arrival.is_empty() {
            let _ = writeln!(out, "  #{}: no deltas (tool-only or empty reply)", call.index);
            continue;
        }
        let first = arrival.first().map(|(ms, _)| *ms).unwrap_or(0);
        let last = arrival.last().map(|(ms, _)| *ms).unwrap_or(0);
        let span = last.saturating_sub(first).max(1);
        let gaps: Vec<u128> =
            arrival.windows(2).map(|pair| pair[1].0.saturating_sub(pair[0].0)).collect();
        let max_gap = gaps.iter().copied().max().unwrap_or(0);
        let _ = writeln!(
            out,
            "  #{}: {} deltas over {} ms (first at {} ms, max gap {} ms) — {}",
            call.index,
            arrival.len(),
            span,
            first,
            max_gap,
            if arrival.len() > 1 && span > 5 { "incremental" } else { "arrived as one chunk" }
        );
    }
    let _ = writeln!(out);
}
