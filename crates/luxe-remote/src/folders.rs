//! Folders — where a device can start a session.
//!
//! The folders Luxe is running in now, the ones herdr's panes are in, and the ones the bridge has
//! seen Luxe running in before, remembered in a small file. Never a scan of the home folder: the
//! list is where you've already worked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use luxe_core::json_file::{FileError, FileMode, JsonFile};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How many remembered folders are kept, the longest unseen dropped first.
const REMEMBERED: usize = 50;

/// How stale a sighting gets before it's written again. The list is read every few seconds, and
/// rewriting the file each time would only refresh the order.
const RESIGHTING_SECS: u64 = 60 * 60;

/// One place a session can start, as a device sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Folder {
    pub(crate) path: String,
    pub(crate) name: String,
    /// Sessions running there now
    pub(crate) sessions: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Remembered {
    #[serde(default)]
    folders: Vec<Sighting>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sighting {
    path: String,
    last_seen: u64,
}

/// The folders the bridge has seen Luxe running in.
#[derive(Debug, Clone)]
pub(crate) struct FolderBook {
    file: JsonFile,
}

impl FolderBook {
    /// The book in `path`, created on first sighting.
    pub(crate) fn at(path: PathBuf) -> Self {
        Self { file: JsonFile::new(path, FileMode::OwnerOnly) }
    }

    /// Where it lives unless told otherwise: Luxe's state folder, since it's a record of use.
    pub(crate) fn default_path() -> PathBuf {
        luxe_core::user_dirs::luxe_state_dir().join("remote-folders.json")
    }

    /// Notes that Luxe is running in each of `running` now.
    pub(crate) fn saw(&self, running: &[String]) -> Result<(), FileError> {
        let now = now();
        let since = now.saturating_sub(RESIGHTING_SECS);
        if all_seen_since(&self.read().folders, running, since) {
            return Ok(());
        }
        self.file.modify(|map| {
            let mut remembered = parse(std::mem::take(map));
            remember(&mut remembered.folders, running, now);
            *map = into_map(&remembered);
            Ok(())
        })
    }

    /// Paths remembered, most recently seen first.
    fn remembered(&self) -> Vec<String> {
        let mut folders = self.read().folders;
        folders.sort_by_key(|sighting| std::cmp::Reverse(sighting.last_seen));
        folders.into_iter().map(|sighting| sighting.path).collect()
    }

    fn read(&self) -> Remembered {
        self.file.read().map(parse).unwrap_or_default()
    }
}

fn all_seen_since(folders: &[Sighting], running: &[String], since: u64) -> bool {
    running.iter().all(|path| {
        folders.iter().any(|sighting| sighting.path == *path && sighting.last_seen >= since)
    })
}

/// Every folder a session could start in: running ones first, then herdr's, then remembered ones,
/// each once and only while it's still a folder.
pub(crate) fn listing(running: &[String], herdr: &[String], book: &FolderBook) -> Vec<Folder> {
    let mut sessions: HashMap<&str, usize> = HashMap::new();
    for path in running {
        *sessions.entry(path).or_default() += 1;
    }
    let remembered = book.remembered();
    let mut listed: Vec<Folder> = Vec::new();
    for path in running.iter().chain(herdr).chain(&remembered) {
        if path.is_empty() || listed.iter().any(|folder| folder.path == *path) {
            continue;
        }
        if !Path::new(path).is_dir() {
            continue;
        }
        let name = luxe_core::paths::dir_name(Path::new(path));
        let count = sessions.get(path.as_str()).copied().unwrap_or(0);
        listed.push(Folder { path: path.clone(), name, sessions: count });
    }
    listed
}

/// Brings each of `running` to `now`, keeping the [`REMEMBERED`] most recent.
fn remember(folders: &mut Vec<Sighting>, running: &[String], now: u64) {
    for path in running {
        match folders.iter_mut().find(|sighting| sighting.path == *path) {
            Some(sighting) => sighting.last_seen = now,
            None => folders.push(Sighting { path: path.clone(), last_seen: now }),
        }
    }
    folders.sort_by_key(|sighting| std::cmp::Reverse(sighting.last_seen));
    folders.truncate(REMEMBERED);
}

/// The remembered folders, or none when the file doesn't read: it's a convenience, rebuilt by use.
fn parse(map: Map<String, Value>) -> Remembered {
    serde_json::from_value(Value::Object(map))
        .inspect_err(|error| tracing::warn!(%error, "the remembered-folder file doesn't read"))
        .unwrap_or_default()
}

fn into_map(remembered: &Remembered) -> Map<String, Value> {
    match serde_json::to_value(remembered) {
        Ok(Value::Object(map)) => map,
        _ => unreachable!("a struct of vectors serializes to an object"),
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests;
