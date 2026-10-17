//! Diagnostics — opening documents, and settling on what the server has to say about them.

use super::*;

/// Hash document content to a `u64` so an unchanged warm read can skip a resync.
pub(super) fn hash_content(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Default budget for a cold `get_diagnostics` settle wait.
pub(super) const DEFAULT_DIAGNOSTICS_WAIT: Duration = Duration::from_secs(10);
/// Minimum time to give the server to react to a document open/change before settling.
pub(super) const SETTLE_MIN_WAIT: Duration = Duration::from_millis(150);
/// How long the diagnostics cache must be unchanged (and progress idle) to count as settled.
pub(super) const SETTLE_QUIESCENCE: Duration = Duration::from_millis(250);
/// After this long idle + quiescent with no publish at all, conclude "clean, nothing to report"
/// rather than waiting the whole budget. A server still working holds `$/progress` tokens
/// (progress_active > 0), so this only fires for a genuinely silent server.
pub(super) const SETTLE_NO_PUBLISH_GRACE: Duration = Duration::from_millis(1500);

impl LspClient {
    /// Ensure a document is open and synced; returns `true` when a fresh publish should be awaited.
    ///
    /// A brand-new document is opened (`didOpen`); a changed one is synced (`didChange`); an
    /// unchanged, already-open document returns `false` so a warm read can use the cache directly.
    pub(super) async fn ensure_document(
        &mut self,
        uri: &str,
        content: &str,
    ) -> Result<bool, LspError> {
        let hash = hash_content(content);
        match self.open_docs.get(uri).copied() {
            Some((_, old_hash)) if old_hash == hash => return Ok(false),
            Some((version, _)) => {
                let version = version + 1;
                self.send_notification(
                    "textDocument/didChange",
                    serde_json::json!({
                        "textDocument": { "uri": uri, "version": version },
                        "contentChanges": [{ "text": content }]
                    }),
                )
                .await?;
                self.open_docs.insert(uri.to_string(), (version, hash));
            }
            None => {
                self.send_notification(
                    "textDocument/didOpen",
                    serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": self.language_id_for(uri),
                            "version": 1,
                            "text": content
                        }
                    }),
                )
                .await?;
                self.open_docs.insert(uri.to_string(), (1, hash));
            }
        }
        // Reset quiescence from our own action so a silent-clean server still settles cleanly.
        self.shared.mark_change();
        Ok(true)
    }

    /// Ensure a document is open (synced from the given content) so document-scoped requests
    /// (`documentSymbol`, navigation) resolve. Idempotent for unchanged content.
    pub async fn open_document(&mut self, uri: &str, content: &str) -> Result<(), LspError> {
        self.ensure_document(uri, content).await.map(|_| ())
    }

    /// Wait until the server is ready to answer navigation/symbol requests.
    ///
    /// Prefers rust-analyzer's `experimental/serverStatus { quiescent: true }`; for servers that
    /// don't send it, falls back to "no in-flight `$/progress` and quiet for a settle window".
    /// Returns immediately for a warm, already-quiescent server.
    pub async fn wait_until_idle(&self, wait: Duration) {
        const FALLBACK_QUIESCENCE: Duration = Duration::from_millis(1200);
        let start = Instant::now();
        loop {
            let (progress_active, quiescent, since_change) = {
                let inner = self.shared.inner.lock().expect("diagnostics state poisoned");
                (inner.progress_active, inner.quiescent, inner.since_change())
            };
            if quiescent || (progress_active == 0 && since_change >= FALLBACK_QUIESCENCE) {
                break;
            }
            if start.elapsed() >= wait {
                break;
            }
            tokio::select! {
                _ = self.shared.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(80)) => {}
            }
        }
    }

    /// Get diagnostics for a document, waiting up to the default budget for the server to settle.
    pub async fn get_diagnostics(
        &mut self,
        uri: &str,
        content: &str,
    ) -> Result<Vec<Diagnostic>, LspError> {
        self.get_diagnostics_with_wait(uri, content, DEFAULT_DIAGNOSTICS_WAIT).await
    }

    /// Get diagnostics for a document with an explicit settle budget.
    ///
    /// Opens/syncs the document, then waits until the server is idle (no in-flight `$/progress`),
    /// the cache has been quiescent, and at least one publish arrived — or the budget elapses.
    /// A warm, unchanged document returns the cached result immediately. A server that has never
    /// published about this document and can be asked is asked, rather than reported as clean.
    pub async fn get_diagnostics_with_wait(
        &mut self,
        uri: &str,
        content: &str,
        wait: Duration,
    ) -> Result<Vec<Diagnostic>, LspError> {
        if self.state != ClientState::Initialized {
            return Err(LspError::NotInitialized);
        }
        if !self.ensure_document(uri, content).await? {
            return Ok(self.cached_diagnostics(uri));
        }

        let base_publish = self.publish_count();
        let start = Instant::now();
        loop {
            let (progress_active, since_change, publish_count) = {
                let inner = self.shared.inner.lock().expect("diagnostics state poisoned");
                (inner.progress_active, inner.since_change(), inner.publish_count)
            };
            let idle_quiescent = start.elapsed() >= SETTLE_MIN_WAIT
                && progress_active == 0
                && since_change >= SETTLE_QUIESCENCE;
            // Waiting out the no-publish grace is pointless when the answer is going to be asked
            // for: the request itself is the synchronization point, since the server holds it
            // until it can answer. Only the idle check still has to pass, so a server that is
            // still loading the project is not interrupted mid-index.
            let settled = idle_quiescent
                && (publish_count > base_publish
                    || self.pulls_diagnostics_for(uri)
                    || start.elapsed() >= SETTLE_NO_PUBLISH_GRACE);
            if settled || start.elapsed() >= wait {
                break;
            }
            tokio::select! {
                _ = self.shared.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(80)) => {}
            }
        }
        if self.pulls_diagnostics_for(uri) {
            // What is left of the caller's budget: the request is where a pull server does its
            // thinking, so it has to answer inside the window the settle loop would have spent.
            self.pull_document_diagnostics(uri, wait.saturating_sub(start.elapsed())).await;
        }
        Ok(self.cached_diagnostics(uri))
    }

    /// Whether this document's diagnostics have to be asked for rather than waited on.
    ///
    /// Push wins wherever it is offered: a server that has published about this file has answered,
    /// and asking again would only replace a settled answer with a second opinion — rust-analyzer
    /// advertises both and its pushed set is cargo's, which is the one a reader means. What is left
    /// is the file the server has said nothing about, which is where a pull-only server lives.
    pub(super) fn pulls_diagnostics_for(&self, uri: &str) -> bool {
        self.pull_diagnostics.is_some() && !self.has_published(uri)
    }

    /// Ask for one document's diagnostics and cache the answer.
    ///
    /// Best-effort: a refusal, or a server that sits on the request past `budget`, is logged and
    /// leaves the previous answer in place — the caller is a diagnostics read that still has a
    /// pushed cache to fall back on.
    pub(super) async fn pull_document_diagnostics(&mut self, uri: &str, budget: Duration) {
        let mut params = serde_json::json!({ "textDocument": { "uri": uri } });
        if let Some(identifier) = self.pull_diagnostics.as_ref().and_then(|p| p.identifier.clone())
        {
            params["identifier"] = Value::String(identifier);
        }
        if let Some(result_id) = self.pulled.get(uri).and_then(|report| report.result_id.clone()) {
            params["previousResultId"] = Value::String(result_id);
        }
        let request = self.send_request("textDocument/diagnostic", params);
        let response = match tokio::time::timeout(budget, request).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                debug!(uri, %error, "lsp: pull diagnostics refused");
                return;
            }
            Err(_) => {
                debug!(uri, ?budget, "lsp: pull diagnostics did not answer in the budget");
                return;
            }
        };
        let result_id = response.get("resultId").and_then(Value::as_str).map(str::to_string);
        // `unchanged` means the previous answer still stands; only a `full` report carries items.
        if response.get("kind").and_then(Value::as_str) == Some("unchanged") {
            if let Some(report) = self.pulled.get_mut(uri) {
                report.result_id = result_id;
            }
            return;
        }
        let items = response
            .get("items")
            .and_then(Value::as_array)
            .map(|raw| {
                raw.iter()
                    .filter_map(|item| match serde_json::from_value::<Diagnostic>(item.clone()) {
                        Ok(diagnostic) => Some(diagnostic),
                        Err(error) => {
                            debug!(uri, %error, "lsp: dropping unparseable pulled diagnostic");
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.pulled.insert(uri.to_string(), PulledReport { result_id, items });
    }

    /// Recent server-reported Error/Warning notices, formatted, newest last. Explains an empty
    /// diagnostics result when a server is misconfigured or can't drive its toolchain.
    pub fn server_notices(&self) -> Vec<String> {
        self.shared
            .inner
            .lock()
            .map(|inner| {
                inner
                    .server_notices
                    .iter()
                    .map(|notice| {
                        let label = if notice.severity == 1 { "error" } else { "warning" };
                        format!("[{label}] {}", notice.text)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn cached_diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        let published = {
            let inner = self.shared.inner.lock().expect("diagnostics state poisoned");
            inner.diagnostics.get(uri).cloned()
        };
        published.unwrap_or_else(|| {
            self.pulled.get(uri).map(|report| report.items.clone()).unwrap_or_default()
        })
    }

    /// Whether the server has published about this document, even to say it is clean.
    pub(super) fn has_published(&self, uri: &str) -> bool {
        self.shared.inner.lock().expect("diagnostics state poisoned").diagnostics.contains_key(uri)
    }

    pub(super) fn publish_count(&self) -> u64 {
        self.shared.inner.lock().expect("diagnostics state poisoned").publish_count
    }
}
