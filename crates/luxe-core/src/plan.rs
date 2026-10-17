//! Plan — reading one phase out of a project's `PLAN.md`.
//!
//! A phased plan is the loop's spine: the developer works one phase at a time and the reviewer
//! judges that phase against the same text. Both need the *slice*, not the document — a reviewer
//! handed all four phases judges phase one against a description of four, and a developer handed
//! them builds all four in the first turn and then has nothing left to claim.
//!
//! Owned here rather than in either caller because they must agree on which lines belong to a
//! phase; two readings of one plan is two different standards.

/// The plan's section for `phase`, when the plan is written in phase headings.
///
/// Matches a heading that names the number — `## Phase 2`, `## Phase 2 — the CLI`, `### phase 2:` —
/// and runs to the next heading of the same or higher level. Returns `None` for a plan with no such
/// heading, which is most projects' and is why callers fall back to the whole document.
pub fn phase_section(plan: &str, phase: u32) -> Option<String> {
    let wanted = format!("phase {phase}");
    let mut lines = plan.lines().enumerate();
    let (start, hashes) = loop {
        let (index, line) = lines.next()?;
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes == 0 {
            continue;
        }
        let heading = trimmed[hashes..].trim().to_ascii_lowercase();
        // `phase 2` must be the number itself, not the start of `phase 20`.
        let matches = heading
            .strip_prefix(&wanted)
            .is_some_and(|rest| rest.is_empty() || !rest.starts_with(|c: char| c.is_ascii_digit()));
        if matches {
            break (index, hashes);
        }
    };

    let mut body = Vec::new();
    for (index, line) in plan.lines().enumerate().skip(start) {
        let trimmed = line.trim_start();
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        // The next heading at this level or above ends the section; a deeper one is part of it.
        if index > start && level > 0 && level <= hashes {
            break;
        }
        body.push(line);
    }
    Some(body.join("\n"))
}

/// The heading line of `phase`'s section, with its `#`s stripped.
///
/// What a one-line reminder says when the whole section has already been sent.
pub fn phase_heading(plan: &str, phase: u32) -> Option<String> {
    let section = phase_section(plan, phase)?;
    let first = section.lines().next()?.trim_start();
    Some(first.trim_start_matches('#').trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "# Plan\n\n## Phase 0 — Counting\n\nCount them.\n\n### Tests\n\nA \
                        deeper heading belongs to the phase.\n\n## Phase 1 — Ranking\n\nRank \
                        them.\n\n## Phase 20 — Much later\n\nNot phase 2.\n";

    /// A phase is its own heading through to the next one at the same level.
    #[test]
    fn a_phase_is_read_from_its_heading_to_the_next_one() {
        let section = phase_section(PLAN, 0).expect("phase 0 exists");
        assert!(section.contains("Count them."), "{section}");
        assert!(section.contains("deeper heading"), "a deeper heading is part of it: {section}");
        assert!(!section.contains("Rank them."), "the next phase is not: {section}");
    }

    /// `Phase 2` is not `Phase 20`.
    #[test]
    fn a_phase_heading_matches_its_own_number() {
        assert!(phase_section(PLAN, 2).is_none(), "there is no phase 2 in this plan");
        let twenty = phase_section(PLAN, 20).expect("phase 20 exists");
        assert!(twenty.contains("Not phase 2."), "{twenty}");
    }

    /// A plan with no phase headings has no sections to read.
    #[test]
    fn a_plan_without_phases_has_no_section() {
        assert!(phase_section("# Plan\n\nJust do the thing.\n", 0).is_none());
    }

    /// The heading is what a one-line reminder names.
    #[test]
    fn the_heading_comes_back_without_its_hashes() {
        assert_eq!(phase_heading(PLAN, 1).as_deref(), Some("Phase 1 — Ranking"));
        assert_eq!(phase_heading(PLAN, 9), None);
    }
}
