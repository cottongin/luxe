//! Dashboard entries — the rows the list shows: the menu, slash commands, and the sessions and
//! commands a draft matches.

use super::*;

impl DashboardState {
    /// The list under the prompt, for the draft as it stands.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        let query = self.query.trim();
        if query.is_empty() {
            return self.menu();
        }
        if let Some(slash) = query.strip_prefix('/') {
            return self.slash_entries(query, slash);
        }
        if self.prompt {
            return vec![Entry::Ask];
        }
        let mut entries = vec![Entry::Ask];
        let sessions = self.matching_sessions(query);
        if !sessions.is_empty() {
            entries.push(Entry::Blank);
            let note = format!("{} of {}", sessions.len(), self.sessions.len());
            entries.push(Entry::Heading("SESSIONS", Some(note)));
            entries.extend(sessions);
        }
        let mut commands = self.matching_commands(query);
        if mentions_project_context(query) {
            commands.push(Entry::ProjectContext);
        }
        if !commands.is_empty() {
            entries.push(Entry::Blank);
            entries.push(Entry::Heading("COMMANDS", None));
            entries.extend(commands);
        }
        if !self.sessions.is_empty() {
            entries.push(Entry::Blank);
            entries.push(Entry::Heading("SEARCH", None));
            entries.push(Entry::Grep);
        }
        entries
    }

    /// Every door the start screen has, with nothing typed.
    pub(super) fn menu(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        if !self.provider_configured {
            entries.push(Entry::SetUp);
        }
        if self.resume.is_some() {
            entries.push(Entry::Resume);
        }
        entries.push(Entry::NewSession);
        if !self.sessions.is_empty() {
            entries.push(Entry::Sessions);
        }
        if self.briefing.changed_files > 0 {
            entries.push(Entry::Review);
        }
        if self.provider_configured {
            entries.push(Entry::Configuration);
        }
        entries.push(Entry::Quit);
        if !self.recent.is_empty() {
            entries.push(Entry::Blank);
            entries.push(Entry::Heading("RECENT", None));
            entries.extend((0..self.recent.len()).map(Entry::Recent));
        }
        entries
    }

    /// A draft that starts with `/` is a command: the one it spells first, then any it could be.
    fn slash_entries(&self, query: &str, name: &str) -> Vec<Entry> {
        let mut entries = Vec::new();
        if let Some(command) = crate::commands::CommandRegistry::shared().parse_slash(query) {
            entries.push(Entry::Slash(command));
        }
        let commands = self.matching_commands(name.split_whitespace().next().unwrap_or(""));
        if !commands.is_empty() {
            if !entries.is_empty() {
                entries.push(Entry::Blank);
            }
            entries.push(Entry::Heading("COMMANDS", None));
            entries.extend(commands);
        }
        entries
    }

    fn matching_sessions(&self, query: &str) -> Vec<Entry> {
        let names: Vec<&str> = self.sessions.iter().map(|session| session.name.as_str()).collect();
        crate::fuzzy::rank(query, &names)
            .into_iter()
            .take(SESSION_MATCHES)
            .map(|found| Entry::Session(found.idx, found.positions))
            .collect()
    }

    fn matching_commands(&self, query: &str) -> Vec<Entry> {
        if query.is_empty() {
            return Vec::new();
        }
        let commands: Vec<&Command> = self.commands.iter().collect();
        crate::commands::rank(query, &commands)
            .into_iter()
            .take(COMMAND_MATCHES)
            .map(|(index, positions)| Entry::Command(index, positions))
            .collect()
    }
}
