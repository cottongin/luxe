//! Test baseline — what was failing last time, so this run can name what changed.
//!
//! A pass/fail count answers "is it green?" and hides the question that actually matters after an
//! edit: *did I break something that used to work?* Agents introduce regressions at a rate the
//! benchmarks mostly don't measure, because a suite that was already red stays red and a newly-red
//! test looks identical to an old one in a raw runner dump.
//!
//! So each unfiltered run records the set of failing test names, and the next run of the same
//! command diffs against it. Filtered runs read the baseline but never write it — a subset that
//! passes is not evidence the suite is green, and letting it overwrite the record would erase the
//! very failures it skipped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What one recorded run remembers.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Baseline {
    /// Failing test names from the last full run of this command.
    failing: BTreeSet<String>,
    passed: usize,
    failed: usize,
}

/// The whole store: one baseline per command.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    runs: std::collections::BTreeMap<String, Baseline>,
}

/// What changed between the previous run and this one.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Delta {
    /// Tests failing now that passed before — regressions, the thing worth interrupting for.
    pub(crate) broke: Vec<String>,
    /// Tests passing now that failed before.
    pub(crate) fixed: Vec<String>,
    /// Whether a previous run of this command was on record at all.
    pub(crate) had_baseline: bool,
}

impl Delta {
    /// Whether there is anything worth saying.
    pub(crate) fn is_interesting(&self) -> bool {
        !self.broke.is_empty() || !self.fixed.is_empty()
    }

    /// The delta as a line for the model, or `None` when nothing moved.
    ///
    /// Regressions lead, because they are the finding: everything else in the reply is about the
    /// change that was intended, and this is the part that wasn't.
    pub(crate) fn render(&self) -> Option<String> {
        if !self.is_interesting() {
            return None;
        }
        let mut out = String::new();
        if !self.broke.is_empty() {
            out.push_str(&format!(
                "REGRESSION — {} test(s) passed before this change and fail now:\n",
                self.broke.len()
            ));
            for name in self.broke.iter().take(MAX_LISTED) {
                out.push_str(&format!("  - {name}\n"));
            }
            if self.broke.len() > MAX_LISTED {
                out.push_str(&format!("  … and {} more\n", self.broke.len() - MAX_LISTED));
            }
        }
        if !self.fixed.is_empty() {
            out.push_str(&format!("Fixed since the last run: {}\n", self.fixed.len()));
        }
        Some(out)
    }
}

/// Most test names listed before the rest is counted.
const MAX_LISTED: usize = 10;

/// Compare this run against the recorded one, and record it when the run was unfiltered.
///
/// `filtered` runs are read-only against the store: a passing subset says nothing about the tests
/// it never executed.
pub(crate) fn diff_and_record(
    cwd: &Path,
    command: &str,
    failing: &BTreeSet<String>,
    passed: usize,
    failed: usize,
    filtered: bool,
) -> Delta {
    let path = store_path(cwd);
    let mut store = load(&path);

    let delta = match store.runs.get(command) {
        Some(previous) => Delta {
            broke: failing.difference(&previous.failing).cloned().collect(),
            fixed: previous.failing.difference(failing).cloned().collect(),
            had_baseline: true,
        },
        None => Delta::default(),
    };

    if !filtered {
        store
            .runs
            .insert(command.to_string(), Baseline { failing: failing.clone(), passed, failed });
        save(&path, &store);
    }

    delta
}

fn store_path(cwd: &Path) -> PathBuf {
    cwd.join(".luxe").join("test-baseline.json")
}

fn load(path: &Path) -> Store {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Best-effort persistence: losing a baseline costs one run's delta, never the run itself.
fn save(path: &Path, store: &Store) {
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(encoded) = serde_json::to_string_pretty(store) {
        let _ = std::fs::write(path, encoded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_newly_failing_test_is_reported_as_a_regression() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cmd = "pytest";

        diff_and_record(dir.path(), cmd, &names(&["test_old"]), 5, 1, false);
        let delta =
            diff_and_record(dir.path(), cmd, &names(&["test_old", "test_new"]), 4, 2, false);

        assert_eq!(delta.broke, vec!["test_new".to_string()], "only the new one is a regression");
        assert!(delta.fixed.is_empty());
        let rendered = delta.render().expect("something to say");
        assert!(rendered.starts_with("REGRESSION"), "{rendered}");
        assert!(rendered.contains("test_new"));
    }

    #[test]
    fn a_test_that_starts_passing_is_reported_as_fixed() {
        let dir = tempfile::tempdir().expect("temp dir");
        diff_and_record(dir.path(), "pytest", &names(&["a", "b"]), 0, 2, false);
        let delta = diff_and_record(dir.path(), "pytest", &names(&["a"]), 1, 1, false);

        assert_eq!(delta.fixed, vec!["b".to_string()]);
        assert!(delta.broke.is_empty());
        assert!(delta.render().expect("something to say").contains("Fixed since the last run: 1"));
    }

    /// A green subset is not evidence the suite is green, so it must not overwrite the record of
    /// the failures it never ran.
    #[test]
    fn a_filtered_run_reads_the_baseline_but_never_replaces_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        diff_and_record(dir.path(), "pytest", &names(&["broken_one"]), 3, 1, false);

        // A filtered run that touched only the healthy tests.
        diff_and_record(dir.path(), "pytest", &BTreeSet::new(), 1, 0, true);

        // The known failure is still on record, so it is not reported as "fixed" next time.
        let delta = diff_and_record(dir.path(), "pytest", &names(&["broken_one"]), 3, 1, false);
        assert!(delta.broke.is_empty(), "still failing, not a new break");
        assert!(delta.fixed.is_empty(), "and never wrongly credited as fixed");
    }

    #[test]
    fn different_commands_keep_separate_baselines() {
        let dir = tempfile::tempdir().expect("temp dir");
        diff_and_record(dir.path(), "pytest unit", &names(&["u1"]), 1, 1, false);
        let delta = diff_and_record(dir.path(), "pytest integration", &names(&["i1"]), 1, 1, false);

        assert!(!delta.had_baseline, "a different suite is not a baseline for this one");
        assert!(delta.broke.is_empty());
    }

    #[test]
    fn the_first_run_reports_nothing_to_compare() {
        let dir = tempfile::tempdir().expect("temp dir");
        let delta = diff_and_record(dir.path(), "pytest", &names(&["a"]), 0, 1, false);

        assert!(!delta.had_baseline);
        assert!(delta.render().is_none(), "nothing moved because nothing came before");
    }
}
