//! Structured verdict types for the review gate.
//!
//! These types define the schema for review gate outputs: individual item verdicts
//! (PASS/FAIL/INVALID) with evidence, and an overall verdict (CONTINUE/FIX).

use serde::{Deserialize, Serialize};

/// Status of a single reviewed item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReviewItemStatus {
    /// Item meets spec intent + gate green.
    Pass,
    /// Real defect — with file:line + smallest fix.
    Fail,
    /// Could not be measured as specified.
    Invalid,
}

impl std::fmt::Display for ReviewItemStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => write!(f, "PASS"),
            Self::Fail => write!(f, "FAIL"),
            Self::Invalid => write!(f, "INVALID"),
        }
    }
}

/// A single reviewed item: label, status, evidence, optional fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReviewItem {
    /// Human-readable label (e.g., "acceptance gate commands", "architecture invariants").
    pub(crate) label: String,
    pub(crate) status: ReviewItemStatus,
    /// File:line evidence (if FAIL).
    pub(crate) evidence: Option<String>,
    /// Smallest fix (if FAIL).
    pub(crate) fix: Option<String>,
}

/// Overall verdict: continue to next phase or fix issues first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Overall {
    /// All items PASS — proceed.
    Continue,
    /// Some items FAIL — fix list provided.
    Fix(Vec<String>),
}

impl std::fmt::Display for Overall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Continue => write!(f, "CONTINUE"),
            Self::Fix(fixes) => {
                write!(f, "FIX:")?;
                for fix in fixes {
                    write!(f, "\n  - {}", fix)?;
                }
                Ok(())
            }
        }
    }
}

/// A complete review verdict: individual items + overall status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReviewVerdict {
    pub(crate) items: Vec<ReviewItem>,
    pub(crate) overall: Overall,
}

impl ReviewVerdict {
    /// Create a new verdict with the given items and derive overall status.
    pub(crate) fn new(items: Vec<ReviewItem>) -> Self {
        let has_failures = items.iter().any(|item| item.status == ReviewItemStatus::Fail);
        let fix_list: Vec<String> = items
            .iter()
            .filter(|item| item.status == ReviewItemStatus::Fail)
            .filter_map(|item| item.fix.clone())
            .collect();

        Self {
            items,
            overall: if has_failures { Overall::Fix(fix_list) } else { Overall::Continue },
        }
    }

    #[cfg(test)]
    fn is_pass(&self) -> bool {
        matches!(self.overall, Overall::Continue)
    }

    #[cfg(test)]
    fn fix_list(&self) -> Vec<&str> {
        match &self.overall {
            Overall::Fix(fixes) => fixes.iter().map(|s| s.as_str()).collect(),
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_item_status_display() {
        assert_eq!(ReviewItemStatus::Pass.to_string(), "PASS");
        assert_eq!(ReviewItemStatus::Fail.to_string(), "FAIL");
        assert_eq!(ReviewItemStatus::Invalid.to_string(), "INVALID");
    }

    #[test]
    fn test_overall_display_continue() {
        assert_eq!(Overall::Continue.to_string(), "CONTINUE");
    }

    #[test]
    fn test_overall_display_fix() {
        let fix = Overall::Fix(vec!["fix1".into(), "fix2".into()]);
        let display = format!("{}", fix);
        assert!(display.starts_with("FIX:"));
        assert!(display.contains("fix1"));
        assert!(display.contains("fix2"));
    }

    #[test]
    fn test_review_verdict_pass() {
        let items = vec![
            ReviewItem {
                label: "gate".into(),
                status: ReviewItemStatus::Pass,
                evidence: None,
                fix: None,
            },
            ReviewItem {
                label: "tests".into(),
                status: ReviewItemStatus::Pass,
                evidence: None,
                fix: None,
            },
        ];
        let verdict = ReviewVerdict::new(items);
        assert!(verdict.is_pass());
        assert_eq!(verdict.fix_list().len(), 0);
    }

    #[test]
    fn test_review_verdict_fail() {
        let items = vec![
            ReviewItem {
                label: "gate".into(),
                status: ReviewItemStatus::Pass,
                evidence: None,
                fix: None,
            },
            ReviewItem {
                label: "invariants".into(),
                status: ReviewItemStatus::Fail,
                evidence: Some("engine.rs:42".into()),
                fix: Some("add modes to EngineConfig".into()),
            },
        ];
        let verdict = ReviewVerdict::new(items);
        assert!(!verdict.is_pass());
        assert_eq!(verdict.fix_list().len(), 1);
        assert_eq!(verdict.fix_list()[0], "add modes to EngineConfig");
    }

    #[test]
    fn test_review_verdict_serialization() {
        let items = vec![ReviewItem {
            label: "test".into(),
            status: ReviewItemStatus::Pass,
            evidence: None,
            fix: None,
        }];
        let verdict = ReviewVerdict::new(items);
        let json = serde_json::to_string(&verdict).unwrap();
        let deserialized: ReviewVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.items.len(), 1);
        assert_eq!(deserialized.items[0].label, "test");
    }
}
