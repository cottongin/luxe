//! Deterministic text-reduction stages for tool output.
//!
//! Each stage is a pure function over lines/text so the pipeline is testable with golden inputs and
//! never depends on an LLM. Order in the generic pipeline: strip ANSI → collapse carriage returns →
//! drop progress noise → dedupe consecutive → collapse blank runs → important-preserving truncate.

/// Strips ANSI/VT escape sequences (CSI `ESC [ … final`, OSC `ESC ] … BEL|ESC\`, and lone escapes).
/// Terminal color/cursor codes are pure noise to a model, so this is always safe to apply.
pub(super) fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: parameters/intermediates until a final byte in 0x40..=0x7e.
            Some('[') => {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            // OSC: string terminated by BEL or ESC\.
            Some(']') => {
                chars.next();
                while let Some(&next) = chars.peek() {
                    if next == '\u{07}' {
                        chars.next();
                        break;
                    }
                    if next == '\u{1b}' {
                        chars.next();
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                    chars.next();
                }
            }
            // Any other two-character escape (e.g. ESC c).
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Keeps the final rendered segment of a line that used carriage returns to overwrite itself
/// (progress bars, spinners). A line without `\r` is returned unchanged.
pub(super) fn collapse_carriage_returns(line: &str) -> &str {
    if !line.contains('\r') {
        return line;
    }
    line.split('\r').rfind(|segment| !segment.is_empty()).unwrap_or("")
}

pub(super) fn collapse_blank_lines(lines: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut prev_blank = false;
    for line in lines {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        prev_blank = blank;
        out.push(line);
    }
    out
}

/// Collapses consecutive identical lines into one, annotating the repeat count (`… (×N)`).
/// Repeated warnings and progress spam collapse to a single line without losing the fact of repeat.
pub(super) fn dedupe_consecutive(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut run: Option<(String, usize)> = None;
    let flush = |out: &mut Vec<String>, line: String, count: usize| {
        if count > 1 {
            out.push(format!("{line}  … (×{count})"));
        } else {
            out.push(line);
        }
    };
    for line in lines {
        match run.take() {
            Some((prev, count)) if prev == line => run = Some((prev, count + 1)),
            Some((prev, count)) => {
                flush(&mut out, prev, count);
                run = Some((line, 1));
            }
            None => run = Some((line, 1)),
        }
    }
    if let Some((prev, count)) = run {
        flush(&mut out, prev, count);
    }
    out
}

/// Drops lines that are pure progress indicators (spinner/braille/block glyphs or percent bars)
/// unless `is_important` keeps them.
pub(super) fn filter_progress(
    lines: Vec<String>,
    is_important: impl Fn(&str) -> bool,
) -> Vec<String> {
    lines.into_iter().filter(|line| is_important(line) || !is_progress_noise(line)).collect()
}

/// Heuristic: a line dominated by spinner/bar glyphs or a bare percentage.
fn is_progress_noise(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    const GLYPHS: &[char] = &[
        '\u{2807}', '\u{2827}', '\u{2837}', '\u{280b}', '\u{280f}', '\u{2819}', '\u{2839}',
        '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}', // braille spinner frames
        '\u{2588}', '\u{2589}', '\u{258a}', '\u{258b}', '\u{258c}', '\u{258d}', '\u{258e}',
        '\u{258f}', '\u{2591}', '\u{2592}', '\u{2593}', // block bar chars
    ];
    if trimmed.chars().any(|c| GLYPHS.contains(&c)) {
        return true;
    }
    // A line that is essentially just "NN%".
    let core: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    core.ends_with('%') && core[..core.len() - 1].chars().all(|c| c.is_ascii_digit())
}

/// When `lines` exceeds `max_lines`, keeps the head, every important line, and the tail, inserting
/// `… N lines omitted …` markers for the gaps. Signal (errors, file:line, failures) survives
/// regardless of where it sits in a long log — unlike a blind head/tail cut.
pub(super) fn truncate_preserving(
    lines: Vec<String>,
    is_important: impl Fn(&str) -> bool,
    head: usize,
    tail: usize,
    max_lines: usize,
) -> Vec<String> {
    if lines.len() <= max_lines {
        return lines;
    }
    let total = lines.len();
    let head_end = head.min(total);
    let tail_start = total.saturating_sub(tail).max(head_end);

    let mut keep = vec![false; total];
    for k in keep.iter_mut().take(head_end) {
        *k = true;
    }
    for (index, k) in keep.iter_mut().enumerate().take(tail_start).skip(head_end) {
        if is_important(&lines[index]) {
            *k = true;
        }
    }
    for k in keep.iter_mut().take(total).skip(tail_start) {
        *k = true;
    }

    let mut out = Vec::new();
    let mut omitted = 0usize;
    for (index, line) in lines.into_iter().enumerate() {
        if keep[index] {
            if omitted > 0 {
                out.push(format!("… {omitted} lines omitted …"));
                omitted = 0;
            }
            out.push(line);
        } else {
            omitted += 1;
        }
    }
    if omitted > 0 {
        out.push(format!("… {omitted} lines omitted …"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_and_osc() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m here"), "error here");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{07}body"), "body");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn carriage_returns_keep_final_frame() {
        assert_eq!(collapse_carriage_returns("10%\r50%\r100%"), "100%");
        assert_eq!(collapse_carriage_returns("no cr"), "no cr");
    }

    #[test]
    fn collapses_blank_runs() {
        let input = vec!["a".into(), "".into(), "".into(), "".into(), "b".into()];
        assert_eq!(collapse_blank_lines(input), vec!["a", "", "b"]);
    }

    #[test]
    fn dedupes_consecutive_with_count() {
        let input = vec!["warn".into(), "warn".into(), "warn".into(), "ok".into()];
        assert_eq!(dedupe_consecutive(input), vec!["warn  … (×3)", "ok"]);
    }

    #[test]
    fn drops_progress_keeps_signal() {
        let input = vec!["\u{2588}\u{2588}\u{2588} 40%".into(), "error: boom".into(), "75%".into()];
        let out = filter_progress(input, |l| l.contains("error"));
        assert_eq!(out, vec!["error: boom"]);
    }

    #[test]
    fn truncate_preserves_head_tail_and_important() {
        let mut lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        lines[50] = "error: important".into();
        let out = truncate_preserving(lines, |l| l.contains("error"), 5, 5, 20);
        let joined = out.join("\n");
        assert!(joined.contains("line 0"), "head kept");
        assert!(joined.contains("line 99"), "tail kept");
        assert!(joined.contains("error: important"), "mid-log signal kept");
        assert!(joined.contains("lines omitted"), "omission marker present");
    }
}
