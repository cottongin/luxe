//! Cache — the session a store writes, parsed once and then read on from where it left off.
//!
//! A long session's file runs to tens of megabytes, and the engine reads its active path on every
//! model call, so parsing it whole each time cost ~60ms a call on a 68 MB session. Only the session
//! this store writes is kept: the picker and the GitHub overview read every session, and keeping
//! those would hold all of them in memory.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use super::super::{Entry, SessionError, SessionId};

/// The entries parsed so far and where in the file the parse stopped.
#[derive(Default)]
struct Parsed {
    entries: Vec<Entry>,
    bytes: u64,
    lines: usize,
}

#[derive(Default)]
struct CacheState {
    writing: Option<SessionId>,
    parsed: Parsed,
}

/// The parsed entries of the session the store writes, refreshed from the file on each read.
#[derive(Default)]
pub(super) struct SessionCache {
    state: Mutex<CacheState>,
}

impl SessionCache {
    /// Marks `session` as the one this store writes, dropping another session's entries.
    pub(super) fn writing(&self, session: &SessionId) {
        let mut state = self.lock();
        if state.writing.as_ref() != Some(session) {
            *state = CacheState { writing: Some(session.clone()), parsed: Parsed::default() };
        }
    }

    /// Calls `read` with every entry in `path`, parsing only what was added since the last call
    /// when `session` is the one this store writes, and the whole file otherwise.
    pub(super) fn read<T>(
        &self,
        session: &SessionId,
        path: &Path,
        read: impl FnOnce(&[Entry]) -> Result<T, SessionError>,
    ) -> Result<T, SessionError> {
        let mut state = self.lock();
        if state.writing.as_ref() != Some(session) {
            drop(state);
            let mut fresh = Parsed::default();
            catch_up(&mut fresh, path)?;
            return read(&fresh.entries);
        }
        catch_up(&mut state.parsed, path)?;
        read(&state.parsed.entries)
    }

    fn lock(&self) -> MutexGuard<'_, CacheState> {
        // the state is rebuilt from the file on the next read, so a panicked reader left nothing
        // that matters
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Parses whatever `path` gained since `parsed` last read it. A file shorter than what was read
/// was replaced, so it's read again from the start.
fn catch_up(parsed: &mut Parsed, path: &Path) -> Result<(), SessionError> {
    let length = fs::metadata(path)?.len();
    if length < parsed.bytes {
        *parsed = Parsed::default();
    }
    if length == parsed.bytes {
        return Ok(());
    }
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(parsed.bytes))?;
    let mut tail = String::new();
    file.read_to_string(&mut tail)?;
    let (entries, lines) = parse_lines(&tail, parsed.lines)?;
    parsed.entries.extend(entries);
    parsed.lines += lines;
    parsed.bytes += tail.len() as u64;
    Ok(())
}

/// The entries in `text`, numbering lines on from `before` so an error names the file's line.
/// Returns them with how many lines `text` held.
fn parse_lines(text: &str, before: usize) -> Result<(Vec<Entry>, usize), SessionError> {
    let mut entries = Vec::new();
    let mut lines = 0;
    for line in text.lines() {
        lines += 1;
        if line.is_empty() {
            continue;
        }
        let entry = serde_json::from_str(line)
            .map_err(|error| SessionError::JsonParse { line: before + lines, error })?;
        entries.push(entry);
    }
    Ok((entries, lines))
}

#[cfg(test)]
mod tests;
