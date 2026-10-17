//! Entry — one node of a session's tree, as it is written to disk and read back.

use super::*;

/// What kind of entry is stored.
///
/// Not `Eq`: a usage entry carries a floating-point cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum EntryKind {
    /// Session header: cwd + creation timestamp.
    Header { cwd: String, created: i64 },
    /// A conversation message.
    Message(Message),
    /// Model switch.
    ModelChange(ModelRef),
    /// Compaction summary.
    Compaction { summary: String, first_kept: EntryId, tokens_before: u64 },
    /// Branch point summary.
    BranchSummary { summary: String, from: EntryId },
    /// What one provider call cost.
    ///
    /// Written for every call the process makes, so resuming a session continues its tally instead
    /// of restarting at zero, and a disputed figure has line-by-line evidence behind it. Inert for
    /// context assembly: only [`EntryKind::Message`] reaches the model.
    Usage {
        /// Which part of the harness made the call.
        source: crate::usage::UsageSource,
        /// The model that served it, for a per-model breakdown.
        model: String,
        /// The call's tokens and cost.
        totals: crate::usage::UsageTotals,
    },
    /// What one cache-warming call cost ([`crate::cache_warming`]).
    ///
    /// A kind of its own rather than a [`EntryKind::Usage`] with a new source, because a source an
    /// older Luxe can't parse fails that entry as corrupt and takes the whole session load with it,
    /// while an unknown *kind* degrades to [`EntryKind::Unknown`] and is skipped. Inert for context
    /// assembly, and counted under [`crate::usage::UsageSource::CacheWarm`] on resume.
    CacheWarm {
        /// The model whose cache was kept warm
        model: String,
        /// The call's tokens and cost
        totals: crate::usage::UsageTotals,
    },
    /// This session worked on a pull request: opened it, pushed to it, or answered its threads.
    ///
    /// Inert for context assembly, like [`EntryKind::Usage`]; it is what `luxe --from-pr` finds.
    PullRequestLink {
        /// Number within its repository.
        number: u64,
        /// Its web address, which names the repository.
        url: String,
    },
    /// This session took up an issue as its brief.
    ///
    /// Inert for context assembly; what a pull request opened from the session closes.
    IssueLink {
        /// Number within its repository.
        number: u64,
        /// Its web address.
        url: String,
    },
    /// This session works in a worktree of its own, on this branch.
    ///
    /// Inert for context assembly. Resuming the session re-enters the worktree — made again from
    /// the branch if it was deleted — before the engine starts.
    Worktree {
        /// The branch
        branch: String,
        /// Where the worktree was, relative to the project
        path: String,
    },
    /// An entry written by a newer Luxe than this one.
    ///
    /// A session log is append-only and shared across versions, so an unrecognized kind must not
    /// make the whole transcript unreadable. It is kept (never rewritten) so the parent chain —
    /// and therefore resume — survives, and ignored everywhere else.
    #[serde(other)]
    Unknown,
}

/// A single node in the session tree.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    /// Unique ID for this entry.
    pub id: EntryId,
    /// Parent entry ID (None for the root/header).
    pub parent_id: Option<EntryId>,
    /// Unix timestamp (seconds) when this entry was created.
    pub ts: i64,
    /// The entry's payload.
    pub kind: EntryKind,
}

/// Reads an entry, degrading an unrecognized `kind` to [`EntryKind::Unknown`] instead of failing.
///
/// Hand-written because the derive cannot deliver what [`EntryKind::Unknown`] promises.
/// `#[serde(other)]` may only sit on a *unit* variant, and in an adjacently tagged enum serde will
/// only match one when the content field is absent — so `{"kind":"future","data":{…}}`, which is
/// what a newer Luxe actually writes, failed the whole load and took every earlier entry with it.
///
/// The retry below is what separates "a kind we have never heard of" from "a kind we know, written
/// wrong". Every known variant carries fields, so stripping `data` and trying again can only
/// succeed for an unknown tag; a corrupt known entry fails both attempts and reports its original
/// error, which keeps real corruption loud.
impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            id: EntryId,
            parent_id: Option<EntryId>,
            ts: i64,
            kind: serde_json::Value,
        }

        let raw = Raw::deserialize(deserializer)?;
        let kind = match serde_json::from_value::<EntryKind>(raw.kind.clone()) {
            Ok(kind) => kind,
            Err(known_shape_error) => {
                let mut bare = raw.kind;
                if let Some(object) = bare.as_object_mut() {
                    object.remove("data");
                }
                serde_json::from_value::<EntryKind>(bare)
                    .map_err(|_| serde::de::Error::custom(known_shape_error))?
            }
        };
        Ok(Entry { id: raw.id, parent_id: raw.parent_id, ts: raw.ts, kind })
    }
}

/// The entry a resume should chain from: the newest one nothing else claims as its parent.
///
/// Not simply the last line written. A session log is append-only and holds *every* branch, and it
/// now also holds inert usage records, so "last written" and "tip of the conversation" stopped
/// being the same question. Resuming from a non-leaf walks a path that stops early — and an entry
/// whose parent chain is shorter than the transcript silently discards the work after it.
pub fn latest_leaf(entries: &[Entry]) -> Option<EntryId> {
    let claimed: std::collections::HashSet<&EntryId> =
        entries.iter().filter_map(|entry| entry.parent_id.as_ref()).collect();
    entries
        .iter()
        .rev()
        .find(|entry| !claimed.contains(&entry.id))
        .or_else(|| entries.last())
        .map(|entry| entry.id.clone())
}
