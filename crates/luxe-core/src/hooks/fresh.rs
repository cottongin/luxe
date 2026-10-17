//! Fresh hook — re-anchors a loop session from the repository's own memory.
//!
//! A continuous loop outlives its sessions: it restarts, compacts, and picks the work back up with
//! a fresh context window. The task store carries what one *session* knew; the loop's journal is
//! what carries across runs, and without it the loop opens every session with a generic steer and
//! no idea what the last one did.
//!
//! Fires once per process on `BeforeAgentStart` and returns [`HookOutcome::InjectContext`], so the
//! anchor is appended to the system prompt rather than submitted as a turn. That is the same
//! treatment one-time context already gets everywhere else: appended, never prepended, so the
//! cached prefix above it survives.
//!
//! # Bounded, and honest about it
//!
//! The journal is append-only and the spec grows without limit, to hundreds of kilobytes. Reading
//! either whole is not a briefing, it is the context window. So the journal contributes its
//! **tail** (recent entries are the ones that carry), the plan contributes a bounded head, and the
//! spec contributes a *pointer*, because an agent with `read` does not need to be handed a document
//! it can open.
//!
//! When there is nothing to anchor from, the hook says nothing. Announcing a re-anchor that loaded
//! no memory is worse than silence: it tells the model it has been briefed.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info};

/// Characters of the journal's tail to carry.
const MAX_JOURNAL_CHARS: usize = 4_000;
/// Characters of the current plan to carry.
const MAX_PLAN_CHARS: usize = 4_000;

/// Re-anchors a fresh loop session from the repository's durable files.
pub struct FreshHook {
    /// Repository root — where the journal, the plan and the spec live.
    repo_root: PathBuf,
    /// Whether the anchor has already been delivered this process.
    delivered: AtomicBool,
}

impl std::fmt::Debug for FreshHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FreshHook").field("repo_root", &self.repo_root).finish()
    }
}

impl FreshHook {
    /// Create a new fresh hook rooted at `repo_root`.
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root, delivered: AtomicBool::new(false) }
    }

    /// Read a file under the repository root, or `None` when it is absent or empty.
    async fn read(&self, relative: impl AsRef<std::path::Path>) -> Option<String> {
        let path = self.repo_root.join(relative);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) if !content.trim().is_empty() => Some(content),
            Ok(_) => None,
            Err(error) => {
                debug!(path = %path.display(), %error, "nothing to read");
                None
            }
        }
    }

    /// The anchor for a fresh session, or `None` when the repository has nothing to say.
    pub async fn anchor(&self) -> Option<String> {
        let (journal_path, _) = crate::loop_hooks::journal_paths(&self.repo_root);
        let journal_relative =
            journal_path.strip_prefix(&self.repo_root).unwrap_or(&journal_path).to_path_buf();

        let plan = self.read("PLAN.md").await.map(|plan| crate::hooks::head(&plan, MAX_PLAN_CHARS));
        let journal = self
            .read(&journal_relative)
            .await
            .map(|journal| crate::hooks::tail(&journal, MAX_JOURNAL_CHARS));
        let has_spec = self.repo_root.join("PROJECT.md").is_file();

        if plan.is_none() && journal.is_none() {
            debug!("no plan and no journal, so nothing to anchor from");
            return None;
        }

        let mut out = String::from(
            "## Picking up where the loop left off\n\nThis session starts with a fresh context \
             window. What the repository remembers:",
        );
        if let Some(plan) = plan {
            out.push_str(&format!("\n\n### The current slice (`PLAN.md`)\n\n{plan}"));
        }
        if let Some(journal) = journal {
            out.push_str(&format!(
                "\n\n### Most recent journal entries (`{}`, tail)\n\n{journal}",
                crate::paths::display(&journal_relative)
            ));
        }
        if has_spec {
            out.push_str(
                "\n\nThe full spec is in `PROJECT.md`. Read it when you need it rather than \
                 assuming what it says.",
            );
        }
        Some(out)
    }
}

#[async_trait]
impl Hook for FreshHook {
    fn name(&self) -> &'static str {
        "fresh"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::BeforeAgentStart]
    }

    async fn on(&self, _point: HookPoint, _ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        // Once per process: `BeforeAgentStart` fires every turn, and re-anchoring on each one would
        // rewrite the system prompt's tail with the same text forever.
        if self.delivered.swap(true, Ordering::Relaxed) {
            return HookOutcome::Continue;
        }
        match self.anchor().await {
            Some(anchor) => {
                info!(chars = anchor.chars().count(), "anchoring from repo memory");
                HookOutcome::InjectContext(anchor)
            }
            None => HookOutcome::Continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ctx() -> HookCtx {
        HookCtx {
            session_id: "test".to_string(),
            event_tx: tokio::sync::mpsc::channel(1).0,
            hook_registry: crate::hook::HookRegistry::new(),
        }
    }

    async fn fire(hook: &FreshHook) -> HookOutcome {
        hook.on(HookPoint::BeforeAgentStart, &HookEvent::Other(serde_json::json!({})), &ctx()).await
    }

    /// A repository with nothing to say gets to say nothing. Announcing a re-anchor that loaded no
    /// memory tells the model it has been briefed when it has not.
    #[tokio::test]
    async fn an_empty_repository_produces_no_anchor() {
        let dir = tempdir().unwrap();
        let hook = FreshHook::new(dir.path().to_path_buf());
        assert!(hook.anchor().await.is_none());
        assert!(matches!(fire(&hook).await, HookOutcome::Continue));
    }

    #[tokio::test]
    async fn the_plan_and_the_journal_tail_are_carried_and_the_spec_is_pointed_at() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".luxe").join("loop")).unwrap();
        std::fs::write(root.join("PLAN.md"), "# Slice\n\nFix the thing.").unwrap();
        std::fs::write(
            root.join(".luxe").join("loop").join("PROGRESS.md"),
            "- old entry\n- newest entry\n",
        )
        .unwrap();
        std::fs::write(root.join("PROJECT.md"), "the whole spec").unwrap();

        let anchor = FreshHook::new(root.to_path_buf()).anchor().await.expect("an anchor");

        assert!(anchor.contains("Fix the thing"), "the plan rides along: {anchor}");
        assert!(anchor.contains("newest entry"), "so does the journal: {anchor}");
        assert!(anchor.contains("PROJECT.md"), "and the spec is named: {anchor}");
        assert!(
            !anchor.contains("the whole spec"),
            "...but not injected — the agent can open it: {anchor}"
        );
    }

    /// The failure this hook shipped with: a 372 KB journal read whole is not a briefing.
    #[tokio::test]
    async fn a_huge_journal_contributes_only_its_tail() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".luxe").join("loop")).unwrap();
        let journal = format!(
            "{}\nLAST LINE OF THE JOURNAL\n",
            "an old entry that scrolled away\n".repeat(20_000)
        );
        std::fs::write(root.join(".luxe").join("loop").join("PROGRESS.md"), &journal).unwrap();

        let anchor = FreshHook::new(root.to_path_buf()).anchor().await.expect("an anchor");

        assert!(anchor.chars().count() < MAX_JOURNAL_CHARS + 600, "bounded: {}", anchor.len());
        assert!(anchor.contains("LAST LINE OF THE JOURNAL"), "and it kept the recent end");
        assert!(anchor.contains("earlier entries omitted"), "and said so");
    }

    /// `BeforeAgentStart` fires every turn; the anchor is a one-time briefing.
    #[tokio::test]
    async fn the_anchor_is_delivered_once() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("PLAN.md"), "# Slice").unwrap();
        let hook = FreshHook::new(dir.path().to_path_buf());

        assert!(matches!(fire(&hook).await, HookOutcome::InjectContext(_)));
        assert!(matches!(fire(&hook).await, HookOutcome::Continue), "not again");
        assert!(matches!(fire(&hook).await, HookOutcome::Continue));
    }
}
