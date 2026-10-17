//! Hook configuration — parse hook definitions and lower them into runner specifications.

use std::collections::HashMap;

use serde::Deserialize;
use tracing::warn;

use super::{CommandHookSpec, HookFailureAction, HookMatcher};
use crate::hook::HookPoint;

/// One hook definition as written in `hooks.json` (the hook point is the map key it sits under).
#[derive(Debug, Clone, Deserialize)]
pub struct CommandHookDef {
    /// Which tool calls to run on. Omit to run on all of them.
    #[serde(default)]
    pub matcher: Option<HookMatcher>,
    /// Program + args (first element is the program).
    pub command: Vec<String>,
    /// Timeout in milliseconds (default 5000).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Fail-safe when the command times out/crashes/returns junk (default `allow`).
    #[serde(default)]
    pub on_timeout: HookFailureAction,
}

fn default_timeout_ms() -> u64 {
    5000
}

/// Points `hooks.json` accepts that the loop never raises.
const RESERVED_POINTS: [HookPoint; 3] =
    [HookPoint::MessageStart, HookPoint::MessageEnd, HookPoint::Notification];

/// Parsed `hooks.json`: a map of hook point → the hooks to run there.
///
/// ```json
/// { "BeforeToolUse": [ { "command": ["./guard.sh"], "on_timeout": "deny" } ] }
/// ```
///
/// A key naming no hook point is collected into [`HooksFile::unknown_points`] rather than failing
/// the parse. Deserializing straight into a `HashMap<HookPoint, _>` made one stale or misspelled
/// key reject the whole document, and the caller's fail-safe is an empty file — so a single typo,
/// or a hook point renamed by an update, silently switched off *every* hook the user had. Losing
/// one hook is a mistake; losing all of them because of one is a different and much worse one.
#[derive(Debug, Clone, Default)]
pub struct HooksFile {
    /// Hooks that resolved to a known point.
    pub hooks: HashMap<HookPoint, Vec<CommandHookDef>>,
    /// Keys that named no hook point, in the order they appeared. The caller reports these; they
    /// are kept rather than logged here because only the caller knows which file they came from.
    pub unknown_points: Vec<String>,
}

impl<'de> Deserialize<'de> for HooksFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = HashMap::<String, Vec<CommandHookDef>>::deserialize(deserializer)?;
        let mut file = HooksFile::default();
        for (key, defs) in raw {
            match HookPoint::parse(&key) {
                Some(point) => file.hooks.entry(point).or_default().extend(defs),
                None => file.unknown_points.push(key),
            }
        }
        file.unknown_points.sort();
        Ok(file)
    }
}

impl HooksFile {
    /// Parse from JSON text.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Lower into the point-keyed [`CommandHookSpec`] map the runner consumes.
    pub fn into_specs(self) -> HashMap<HookPoint, Vec<CommandHookSpec>> {
        self.hooks
            .into_iter()
            .map(|(point, defs)| {
                // Accepted so a config naming one still loads, but nothing raises it. Silence here
                // reads exactly like a hook that works and has not been needed yet.
                if RESERVED_POINTS.contains(&point) {
                    warn!(
                        ?point,
                        "this hook point is reserved and never fires; its hooks will not run"
                    );
                }
                let specs = defs
                    .into_iter()
                    .map(|d| {
                        // Only a tool call carries arguments, and only `BeforeToolUse` is handed
                        // them. Saying so beats the two silent alternatives: a matcher that never
                        // fires, or one that quietly widens to every call at that point.
                        if point != HookPoint::BeforeToolUse
                            && d.matcher.as_ref().is_some_and(HookMatcher::needs_arguments)
                        {
                            warn!(
                                ?point,
                                command = ?d.command,
                                "an argument pattern only matches at BeforeToolUse; \
                                 matching on the tool name alone here"
                            );
                        }
                        // Kept rather than dropped: at run time it fails like a program that
                        // will not spawn, so a fail-closed guard stays closed.
                        if d.command.is_empty() {
                            warn!(?point, "a hook has an empty `command`; it will use on_timeout");
                        }
                        CommandHookSpec {
                            point,
                            matcher: d.matcher,
                            command: d.command,
                            timeout_ms: d.timeout_ms,
                            on_timeout: d.on_timeout,
                        }
                    })
                    .collect();
                (point, specs)
            })
            .collect()
    }
}
