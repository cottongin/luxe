//! File listings — which folders the tree wants read, and the reads it accepts, fails and refreshes.

use super::*;

impl FilesTree {
    /// Folders drawn open that want a listing asked for, the project root first.
    pub fn wanted(&self, changes: &[DockChangeItem]) -> Vec<String> {
        if self.changed_only {
            return Vec::new();
        }
        let needs = |folder: &str| {
            self.listings.get(folder).is_none_or(|listing| {
                listing.request.is_none()
                    && (listing.stale || (listing.entries.is_none() && listing.failed.is_none()))
            })
        };
        let mut wanted = Vec::new();
        if needs("") {
            wanted.push(String::new());
        }
        if self.listed("").is_none() {
            return wanted;
        }
        for row in self.rows(changes).iter() {
            if let TreeRow::Folder { path, open: true, .. } = row {
                if needs(path) {
                    wanted.push(path.clone());
                }
            }
        }
        wanted
    }

    /// How many listings have been asked for and not yet answered.
    pub fn in_flight(&self) -> usize {
        self.listings.values().filter(|listing| listing.request.is_some()).count()
    }

    /// Records that `folder`'s listing was asked for under `request`.
    pub fn requested(&mut self, folder: &str, request: ListingRequestId) {
        self.generation += 1;
        let listing = self.listings.entry(folder.to_string()).or_default();
        listing.request = Some(request);
    }

    /// Takes a listing, unless a newer request for the same folder has superseded it.
    pub fn accept(&mut self, request: &ListingRequestId, listing: DirectoryListing) -> bool {
        let Some(entry) = self.listings.get_mut(&listing.path) else { return false };
        if entry.request.as_ref() != Some(request) {
            return false;
        }
        self.generation += 1;
        *entry = Listing { entries: Some(listing), ..Listing::default() };
        true
    }

    /// Takes a failure under the same guard.
    pub fn fail(&mut self, request: &ListingRequestId, folder: &str, error: String) -> bool {
        let Some(entry) = self.listings.get_mut(folder) else { return false };
        if entry.request.as_ref() != Some(request) {
            return false;
        }
        entry.request = None;
        entry.failed = Some(error);
        self.generation += 1;
        true
    }

    /// Marks every listing as overtaken when the changes moved, so open folders are asked again —
    /// the agent creating a file is exactly when the tree must not keep last turn's view of it.
    ///
    /// Answers whether anything was marked.
    pub fn refresh_if_changed(&mut self, changes: &[DockChangeItem]) -> bool {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for item in changes {
            item.path.hash(&mut hasher);
            item.additions.hash(&mut hasher);
            item.deletions.hash(&mut hasher);
        }
        let seen = hasher.finish();
        if seen == self.changes_seen {
            return false;
        }
        self.changes_seen = seen;
        self.generation += 1;
        for listing in self.listings.values_mut() {
            listing.stale = listing.entries.is_some() || listing.failed.is_some();
            listing.failed = None;
        }
        true
    }

    /// Marks every listing as overtaken, whatever the changes did: the reader asked to re-read.
    pub fn refresh_all(&mut self) {
        self.generation += 1;
        for listing in self.listings.values_mut() {
            listing.stale = listing.entries.is_some() || listing.failed.is_some();
            listing.failed = None;
        }
    }

    /// Lists what `.gitignore` ignores too, or stops; every listing is asked again to match.
    pub fn toggle_ignored(&mut self) {
        self.show_ignored = !self.show_ignored;
        self.refresh_all();
    }
}
