//! Permission — judging a call, asking about it, remembering the answer, and refusing a loop.

use super::*;

impl Engine {
    /// Evaluate the permission engine for a tool call, running the ask round-trip when a front-end
    /// can prompt. Returns `Some(reason)` when the call is denied (by policy or the user).
    pub(super) async fn check_permission(
        &self,
        tool: &str,
        args: &serde_json::Value,
        hook_ask: Option<&str>,
        event_tx: &mpsc::Sender<Event>,
        hook_cx: &HookCtx,
    ) -> Option<String> {
        let root = self.project_root();
        let mut request = crate::permission::build_request(tool, args, &root);
        if self.bump_doom_loop(tool, args).await {
            request
                .checks
                .push(Check { permission: DOOM_LOOP.to_string(), input: tool.to_string() });
        }
        // A hook escalation arrives as one more check rather than a prompt of its own, so it folds
        // in by the same strictest-wins rule and resolves the same way everywhere else does.
        //
        // Nothing to remember, though. A hook judges each call afresh, so an "always" answered here
        // approved the tool's own rule — and wrote it to the global file — while the hook went on
        // asking. Silencing a hook is `hook_ask` in `permissions.json`, which says so in the open.
        if let Some(reason) = hook_ask {
            request
                .checks
                .push(Check { permission: HOOK_ASK.to_string(), input: tool.to_string() });
            request.summary = format!("{} — {reason}", request.summary);
            request.always.clear();
        }
        let decision = self.permission.lock().await.decide(&request);
        // The runtime auto-approve toggle forces `ask` → allow (deny still wins above).
        let ask_resolution = self.ask_resolution_now();
        match decision {
            PermissionAction::Allow => None,
            PermissionAction::Deny => Some(deny_message(&request)),
            PermissionAction::Ask => match ask_resolution {
                // Auto-allow is where nobody is watching, so the few acts that need a person stop
                // here rather than being approved on nobody's behalf
                AskResolution::AutoAllow => crate::permission::defaults::needs_a_person(&request)
                    .map(|act| {
                        format!(
                            "Tool call denied: {}. {act} needs someone to approve it, and nobody \
                             can in this run. Leave it for a person, or report that it is ready \
                             for them.",
                            request.summary
                        )
                    }),
                // Why, not just no: this run has nobody to approve it, so the useful next move is
                // a different route — and a model told only "denied" tries the same call again.
                AskResolution::AutoDeny => Some(format!(
                    "Tool call denied: {} needs approval, and this run is unattended (--on-ask \
                     deny), so there is nobody to give it. Take a route that does not need \
                     approval, or report what you would have needed.",
                    request.summary
                )),
                AskResolution::Prompt => {
                    match self.prompt_permission(&request, event_tx, hook_cx).await {
                        // No rule to record, so a remembering reply is a single approval and
                        // must not report something saved
                        PermissionReply::Always | PermissionReply::AlwaysPersist
                            if request.always.is_empty() =>
                        {
                            None
                        }
                        PermissionReply::Once => None,
                        PermissionReply::Always => {
                            self.permission.lock().await.approve(request.always.clone());
                            None
                        }
                        PermissionReply::AlwaysPersist => {
                            self.permission.lock().await.approve(request.always.clone());
                            self.persist_approval(&request, event_tx).await;
                            None
                        }
                        PermissionReply::Reject => Some(rejected_message(&request)),
                    }
                }
            },
        }
    }

    /// Write a "remember this" approval to the global permissions file so it outlives the session.
    ///
    /// Always the *global* file: a project-local ruleset is tighten-only, so an approval written
    /// there would be silently ignored. A write failure is reported to the user — the call still
    /// proceeds, it just won't be remembered next time.
    async fn persist_approval(
        &self,
        request: &crate::permission::PermissionRequest,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let Some(path) = self.config.permissions_file.clone() else {
            let _ = event_tx
                .send(Event::Notice(
                    "approved for this session only — no permissions file is configured to save to"
                        .to_string(),
                ))
                .await;
            return;
        };
        let rules = request.always.clone();
        let write = tokio::task::spawn_blocking(move || {
            crate::permission::persist_rules(&path, &rules).map(|()| path)
        })
        .await;
        match write {
            Ok(Ok(path)) => {
                let _ = event_tx
                    .send(Event::Notice(format!("approval saved to {}", path.display())))
                    .await;
            }
            Ok(Err(error)) => {
                warn!(%error, "could not persist the permission approval");
                let _ = event_tx
                    .send(Event::Notice(format!(
                        "approved for this session — saving it failed: {error}"
                    )))
                    .await;
            }
            Err(error) => warn!(%error, "permission persist task failed"),
        }
    }

    /// Emit a `PermissionRequested` event and await the user's reply, unblocking on cancel.
    ///
    /// Brackets the wait with the `PermissionRequested`/`PermissionResolved` hook points so
    /// observers (e.g. the Herdr integration) can mark the turn `blocked` while a human decides and
    /// resume it afterward. Those hooks are observational — their outcome is ignored; the decision
    /// is owned by the permission engine and the user's reply. Only reached on an interactive
    /// `ask`, so headless/loop auto-allow/deny never fires them.
    async fn prompt_permission(
        &self,
        request: &crate::permission::PermissionRequest,
        event_tx: &mpsc::Sender<Event>,
        hook_cx: &HookCtx,
    ) -> PermissionReply {
        let (id, rx) = self.permission_broker.register();
        let session_id = self.current_session_id().await.to_string();
        let always_patterns =
            request.always.iter().map(|r| format!("{} {}", r.permission, r.pattern)).collect();
        let _ = event_tx
            .send(Event::PermissionRequested {
                id: id.clone(),
                session_id,
                tool: request.tool.clone(),
                permission: request.permission.clone(),
                summary: request.summary.clone(),
                always_patterns,
            })
            .await;

        // Notify observers the turn is now blocked on a human decision (and later, unblocked). The
        // payload carries only the tool + permission key — not the summary — to avoid piping a
        // command string to a command hook's stdin.
        let perm_event = |phase: &str| {
            HookEvent::Other(serde_json::json!({
                "event": "permission_prompt",
                "phase": phase,
                "tool": request.tool,
                "permission": request.permission,
            }))
        };
        self.config
            .hook_registry
            .invoke(HookPoint::PermissionRequested, &perm_event("requested"), hook_cx)
            .await;

        tracing::info!(tool = %request.tool, permission = %request.permission, ?id, "waiting on a permission answer");
        let asked = std::time::Instant::now();
        let mut cancel = self.cancel_tx.subscribe();
        let reply = tokio::select! {
            reply = rx => reply.unwrap_or(PermissionReply::Reject),
            _ = wait_until_cancelled(&mut cancel) => {
                self.permission_broker.forget(&id);
                PermissionReply::Reject
            }
        };
        tracing::info!(?reply, waited_ms = asked.elapsed().as_millis(), "permission prompt closed");
        // Every front-end showing it takes it down, whichever one answered
        let _ = event_tx.send(Event::PromptClosed { id: id.as_str().to_string() }).await;

        self.config
            .hook_registry
            .invoke(HookPoint::PermissionResolved, &perm_event("resolved"), hook_cx)
            .await;
        reply
    }

    /// Doom-loop guard: `true` once the same `(tool, args)` call has repeated ≥3× in a row.
    ///
    /// Only *consecutive* repeats count — any different call resets the counter — so an identical
    /// command run again after real work in between is not a loop.
    async fn bump_doom_loop(&self, tool: &str, args: &serde_json::Value) -> bool {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        tool.hash(&mut hasher);
        args.to_string().hash(&mut hasher);
        let key = hasher.finish();
        let mut guard = self.recent_tool_call.lock().await;
        let count = match &*guard {
            Some((k, c)) if *k == key => c + 1,
            _ => 1,
        };
        *guard = Some((key, count));
        count >= 3
    }
}

/// The message a denied call hands back to the model.
///
/// A doom-loop denial gets its own wording. "Denied by policy" invites a retry, which is the one
/// thing that cannot help — the model needs to be told the call already ran and produced that
/// result, so it stops and does something else.
///
/// Running as root gets its own too, because "denied by policy" reads as an obstacle to route
/// around, and the two routes a model reaches for are worse: rewording the command past the rule,
/// or stopping. What it should do is hand the command to the person.
pub(super) fn deny_message(request: &crate::permission::PermissionRequest) -> String {
    if crate::permission::defaults::runs_as_root(request) {
        return "Blocked: Luxe does not run commands as root. A command that needs root needs the \
                user's attention and consent, so they run it: give them the exact command in a \
                fenced code block, ask them to run it in their own terminal, and continue once \
                they say it is done. Do not look for another way to run it here."
            .to_string();
    }
    if request.checks.iter().any(|check| check.permission == DOOM_LOOP) {
        return format!(
            "Blocked: `{}` was already called with these exact arguments twice in a row and \
             returned the same result. Repeating it cannot help — use the result you already have, \
             or take a different approach.",
            request.tool
        );
    }
    format!("{}: {}", crate::tool::denial::BY_POLICY, request.summary)
}

/// The message a call the reader rejected hands back to the model.
pub(super) fn rejected_message(request: &crate::permission::PermissionRequest) -> String {
    format!("{}: {}", crate::tool::denial::BY_USER, request.summary)
}
