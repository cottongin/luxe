//! Nesting — how deep inside other Luxes this one runs, and the ceiling on it.

/// Environment variable carrying how many Luxe processes enclose this one.
///
/// Inherited by every child, so a nested Luxe reads its own depth without anything having to
/// recognize that a command *is* Luxe — which is undecidable from a `bash` argv anyway.
pub const NESTING_DEPTH_VAR: &str = "LUXE_DEPTH";

/// Override for [`DEFAULT_MAX_NESTING_DEPTH`].
pub const MAX_NESTING_DEPTH_VAR: &str = "LUXE_MAX_DEPTH";

/// How deep Luxe may nest before refusing to start.
///
/// Two is enough for the delegation people actually want — a session spawns a subagent, and that
/// subagent may itself consult one — and short enough that a runaway is three processes rather
/// than a machine. The ceiling exists because nesting is *multiplicative*: a session that spawns
/// two subagents per turn is a fork bomb at depth 4, and this repository has already lost a
/// machine to one (a review-gate hook whose acceptance command re-ran the suite that registered
/// it). A guard that costs one environment variable is cheaper than rediscovering that.
pub const DEFAULT_MAX_NESTING_DEPTH: u32 = 2;

/// Why a nested Luxe refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "refusing to start: this Luxe is {depth} levels deep inside another Luxe (limit {max}).\n\
     Nesting multiplies — each level can spawn several of the next — so the limit is a guard \
     against a runaway, not a judgement about your setup.\n\
     Raise it with {var}=<n> if you meant this, or unset {depth_var} if a stale value is \
     lingering in your shell."
)]
pub struct NestingRefused {
    /// How many Luxe processes enclose this one.
    pub depth: u32,
    /// The ceiling that was exceeded.
    pub max: u32,
    /// Name of the variable that raises the ceiling.
    var: &'static str,
    /// Name of the variable carrying the depth.
    depth_var: &'static str,
}

/// Whether a Luxe at `depth` may run under a ceiling of `max`.
///
/// Split out from the environment so the policy is testable without mutating process state.
pub fn check_nesting(depth: u32, max: u32) -> Result<(), NestingRefused> {
    if depth > max {
        return Err(NestingRefused {
            depth,
            max,
            var: MAX_NESTING_DEPTH_VAR,
            depth_var: NESTING_DEPTH_VAR,
        });
    }
    Ok(())
}

/// Take this process's place in the nesting chain, refusing if it is already too deep.
///
/// Reads [`NESTING_DEPTH_VAR`], checks it against the ceiling, then republishes it incremented so
/// every child — a `bash` tool call, a command hook, an MCP server, a delegated headless run —
/// inherits the next depth down. Call once, early, before anything is spawned.
///
/// Returns this process's own depth (0 when a human launched it). A malformed value is treated as
/// 0: it is somebody's stale shell export, and refusing to start over it would be worse than the
/// problem.
pub fn claim_nesting_slot() -> Result<u32, NestingRefused> {
    let depth = std::env::var(NESTING_DEPTH_VAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let max = std::env::var(MAX_NESTING_DEPTH_VAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_NESTING_DEPTH);

    check_nesting(depth, max)?;
    std::env::set_var(NESTING_DEPTH_VAR, (depth + 1).to_string());
    Ok(depth)
}

#[cfg(test)]
mod tests;
