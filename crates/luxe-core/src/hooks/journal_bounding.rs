//! Journal bounding hook — moves the loop journal's entries to an archive past a size cap.
//!
//! Keeps the journal small enough to be read whole by bounding it, and keeps what it displaced by
//! **appending** to the archive. The first version renamed the journal over the archive, so every
//! archive destroyed the one before it and only the most recent survived — for a file whose whole
//! job is to be the loop's durable memory.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use async_trait::async_trait;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Default maximum journal size in KB before archiving.
const DEFAULT_JOURNAL_MAX_KB: u64 = 24;

/// A hook that auto-archives PROGRESS.md when it exceeds the size cap.
///
/// Fires on `HookPoint::AgentEnd`. If the journal exceeds `journal_max_kb`, moves it to
/// `PROGRESS.archive.md`, keeping its newest section and a pointer to the archive.
pub struct JournalBoundingHook {
    /// Path to the journal file (`.luxe/loop/PROGRESS.md`; see `loop_hooks::journal_paths`).
    journal_path: PathBuf,
    /// Path to the archive file (`.luxe/loop/PROGRESS.archive.md`).
    archive_path: PathBuf,
    /// Maximum journal size in KB before archiving.
    max_kb: u64,
}

impl std::fmt::Debug for JournalBoundingHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalBoundingHook").field("max_kb", &self.max_kb).finish()
    }
}

impl JournalBoundingHook {
    /// Create a new journal bounding hook.
    pub fn new(journal_path: PathBuf, archive_path: PathBuf, max_kb: u64) -> Self {
        Self { journal_path, archive_path, max_kb }
    }

    /// Create with default max size (24 KB).
    pub fn with_defaults(journal_path: PathBuf, archive_path: PathBuf) -> Self {
        Self::new(journal_path, archive_path, DEFAULT_JOURNAL_MAX_KB)
    }

    /// Check if the journal exceeds the size cap.
    async fn should_archive(&self) -> bool {
        match tokio::fs::metadata(&self.journal_path).await {
            Ok(metadata) => metadata.len() / 1024 >= self.max_kb,
            Err(_) => false,
        }
    }

    /// Append the journal's older entries to the archive, keeping the newest section in place.
    ///
    /// Append rather than replace: the archive is the older half of one continuous record, so a
    /// rename over it throws away everything before the last cap crossing.
    /// Both files are opened, read and written through `tokio::fs`, because this runs on the
    /// runtime's thread at the end of every turn. The files are small enough that the blocking
    /// version never showed up, which is exactly the kind of stall that is discovered later, on a
    /// slower disk, as a loop that pauses between iterations for no visible reason.
    async fn archive_journal(&self) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;

        debug!(
            journal = ?self.journal_path,
            archive = ?self.archive_path,
            "archiving journal"
        );
        let journal = tokio::fs::read_to_string(&self.journal_path).await?;
        let archive_name = self.archive_path.file_name().unwrap_or_default().to_string_lossy();
        let (entries, kept) = split_for_archive(&journal, &archive_name);
        let mut archive = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.archive_path)
            .await?;
        // A separator, so two runs' entries do not fuse into one unreadable line.
        if archive.metadata().await.map(|meta| meta.len() > 0).unwrap_or(false) {
            archive.write_all(b"\n").await?;
        }
        archive.write_all(entries.as_bytes()).await?;
        archive.sync_all().await?;
        // Rewritten rather than removed: the loop appends to this path without re-creating it
        tokio::fs::write(&self.journal_path, kept).await?;
        Ok(())
    }
}

/// Splits a journal into what moves to the archive and what stays: the preamble, a line pointing
/// at the archive, and the newest `## ` section.
///
/// An empty journal after archiving told the next turn nothing — not where the history went, and
/// not what the run had just done. A journal with no sections moves whole.
fn split_for_archive(journal: &str, archive_name: &str) -> (String, String) {
    let starts: Vec<usize> = journal
        .match_indices("## ")
        .map(|(at, _)| at)
        .filter(|&at| at == 0 || journal[..at].ends_with('\n'))
        .collect();
    let Some(&first) = starts.first() else {
        return (journal.to_string(), pointer_header("", archive_name));
    };
    let newest = starts[starts.len() - 1];
    let header = pointer_header(&journal[..first], archive_name);
    match starts.len() {
        1 => (journal[first..].to_string(), header),
        _ => (journal[first..newest].to_string(), header + &journal[newest..]),
    }
}

/// `preamble`, or a title when there is none, with a line naming the archive unless it already
/// does.
fn pointer_header(preamble: &str, archive_name: &str) -> String {
    let preamble = match preamble.trim() {
        "" => "# Loop journal",
        text => text,
    };
    match preamble.contains(archive_name) {
        true => format!("{preamble}\n\n"),
        false => {
            format!("{preamble}\n\nEarlier entries are in `{archive_name}` beside this file.\n\n")
        }
    }
}

#[async_trait]
impl Hook for JournalBoundingHook {
    fn name(&self) -> &'static str {
        "journal-bounding"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::AgentEnd]
    }

    async fn on(&self, _point: HookPoint, _ev: &HookEvent, cx: &HookCtx) -> HookOutcome {
        if self.should_archive().await {
            info!(
                journal = ?self.journal_path,
                max_kb = self.max_kb,
                "archiving the journal at its size cap"
            );
            match self.archive_journal().await {
                Ok(()) => cx.notify(format!(
                    "the journal passed {} KB — its entries moved to the archive beside it",
                    self.max_kb
                )),
                Err(error) => warn!(%error, "failed to archive journal"),
            }
        }
        HookOutcome::Continue
    }
}

#[cfg(test)]
mod tests;
