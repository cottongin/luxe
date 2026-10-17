//! JSONL — the store that keeps each session as one JSON object per line.

use super::*;

/// In-memory index of sessions for fast lookup.
#[derive(Debug, Default)]
struct SessionIndex {
    /// session_id -> (path to .jsonl file, last entry id)
    sessions: HashMap<SessionId, (PathBuf, Option<EntryId>)>,
}

/// JSONL-backed session store with an injectable root directory.
pub struct JsonlSessionStore {
    /// Root directory for session storage.
    root: PathBuf,
    /// Index of known sessions.
    index: SessionIndex,
    /// Project hash for directory naming (derived from cwd).
    project_hash: String,
}

impl JsonlSessionStore {
    /// Create a new store rooted at `root`, scanning for existing sessions.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let mut store = Self { root, index: SessionIndex::default(), project_hash: String::new() };
        store.scan_for_sessions();
        store
    }

    /// Set the project hash (derived from cwd in production).
    pub fn set_project_hash(&mut self, hash: &str) {
        self.project_hash = hash.to_string();
        // Rescan with the new project hash.
        self.scan_for_sessions();
    }

    /// Scan the filesystem for existing session files and rebuild the index.
    fn scan_for_sessions(&mut self) {
        let project_dir = self.root.join(&self.project_hash);
        if !project_dir.exists() {
            return;
        }

        if let Ok(entries) = fs::read_dir(&project_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "jsonl") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        let session_id = SessionId(stem.to_string());
                        self.index.sessions.insert(session_id, (path, None));
                    }
                }
            }
        }
    }

    /// Get the directory for a session.
    pub(super) fn session_dir(&self, session_id: &SessionId) -> PathBuf {
        self.root.join(&self.project_hash).join(format!("{}.jsonl", session_id.0))
    }

    /// Get current timestamp in seconds.
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// List all known session IDs (for the session picker).
    pub fn list_sessions(&self) -> Vec<SessionId> {
        self.index.sessions.keys().cloned().collect()
    }

    /// Path to a session's `.jsonl` file, if the store knows the session.
    pub fn session_path(&self, session: &SessionId) -> Option<PathBuf> {
        self.index.sessions.get(session).map(|(path, _)| path.clone())
    }

    /// The most recently modified session for the active project, if any.
    ///
    /// Used by `--continue` to resume where the user left off.
    pub fn most_recent_session(&self) -> Option<SessionId> {
        self.modified_sessions().max_by_key(|(_, modified)| *modified).map(|(id, _)| id)
    }

    /// Every known session with when its file last changed, skipping one whose file is gone.
    fn modified_sessions(&self) -> impl Iterator<Item = (SessionId, SystemTime)> + '_ {
        self.index.sessions.iter().filter_map(|(id, (path, _))| {
            let modified = path.metadata().ok()?.modified().ok()?;
            Some((id.clone(), modified))
        })
    }
}

impl SessionStore for JsonlSessionStore {
    fn create(&mut self, cwd: &str) -> Result<SessionId, SessionError> {
        let session_id = SessionId::new();
        let dir = self.session_dir(&session_id);

        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }

        let header = Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: self.now(),
            kind: EntryKind::Header { cwd: cwd.to_string(), created: self.now() },
        };

        let file = fs::File::create(&dir)?;
        let mut writer = io::BufWriter::new(file);
        writeln!(
            writer,
            "{}",
            serde_json::to_string(&header).map_err(SessionError::JsonSerialize)?
        )?;
        writer.flush()?;

        self.index.sessions.insert(session_id.clone(), (dir, Some(header.id.clone())));

        Ok(session_id)
    }

    fn append(
        &mut self,
        session: &SessionId,
        parent: Option<EntryId>,
        kind: EntryKind,
    ) -> Result<EntryId, SessionError> {
        let (path, _) =
            self.index.sessions.get(session).ok_or(SessionError::NotFound(session.clone()))?;

        let entry = Entry { id: EntryId::new(), parent_id: parent, ts: self.now(), kind };

        let file = fs::OpenOptions::new().append(true).open(path)?;
        let mut writer = io::BufWriter::new(file);
        writeln!(
            writer,
            "{}",
            serde_json::to_string(&entry).map_err(SessionError::JsonSerialize)?
        )?;
        writer.flush()?;

        if let Some(entry_info) = self.index.sessions.get_mut(session) {
            entry_info.1 = Some(entry.id.clone());
        }

        Ok(entry.id)
    }

    fn load(&self, session: &SessionId) -> Result<Vec<Entry>, SessionError> {
        let (path, _) =
            self.index.sessions.get(session).ok_or(SessionError::NotFound(session.clone()))?;

        let file = fs::File::open(path)?;
        let reader = io::BufReader::new(file);

        let mut entries = Vec::new();
        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            let entry: Entry = serde_json::from_str(&line)
                .map_err(|e| SessionError::JsonParse { line: line_num + 1, error: e })?;
            entries.push(entry);
        }

        Ok(entries)
    }

    fn active_path(&self, session: &SessionId, leaf: &EntryId) -> Result<Vec<Entry>, SessionError> {
        let entries = self.load(session)?;

        let by_id: HashMap<&EntryId, &Entry> = entries.iter().map(|e| (&e.id, e)).collect();

        let mut path = Vec::new();
        let mut current_id: Option<&EntryId> = Some(leaf);

        while let Some(id) = current_id {
            let entry = by_id.get(id).ok_or(SessionError::EntryNotFound(id.clone()))?;
            path.push((*entry).clone());
            current_id = entry.parent_id.as_ref();
        }

        // Reverse to get root-to-leaf order.
        path.reverse();
        Ok(path)
    }

    fn list_sessions(&self) -> Vec<SessionId> {
        JsonlSessionStore::list_sessions(self)
    }

    fn most_recent_session(&self) -> Option<SessionId> {
        JsonlSessionStore::most_recent_session(self)
    }

    fn recent_sessions(&self, limit: usize) -> Vec<(SessionId, SystemTime)> {
        let mut sorted: Vec<_> = self.modified_sessions().collect();
        sorted.sort_by_key(|b| std::cmp::Reverse(b.1));
        sorted.truncate(limit);
        sorted
    }
}
