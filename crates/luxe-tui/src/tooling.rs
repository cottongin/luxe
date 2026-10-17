//! Tooling views — plain-text renderers for the `/lsp`, `/debug`, `/mcp` and `/tools` status
//! commands.
//!
//! The data ([`luxe_core::ToolingSummary`]) is rebuilt by the runtime collector each workspace
//! snapshot, so it already reflects config edits, binaries installed mid-session, warm servers, and
//! live debug sessions. These functions only format it; there's no I/O here. Rendered as a system
//! block, not markdown: the one markup it reads is `` `quoted` ``, drawn as inline code.

use luxe_core::{
    DebugSessionStatus, McpServerStatus, ToolAvailability, ToolStatus, ToolingSummary,
};

/// Render the `/tools` status view.
///
/// Answers the question the other views cannot: *why can it not do that?* A tool the model will
/// not reach for is either gone or merely deferred, and from the outside those look identical — so
/// a user reads "it won't search the web" and goes hunting through config for something that was
/// only ever one activation away.
pub(crate) fn render_tools(summary: &ToolingSummary) -> String {
    let mut out = String::from("Tools · what the agent can reach right now");
    if summary.tools.is_empty() {
        out.push_str("\n\n  (not ready yet) — try `/tools` again in a moment.");
        return out;
    }

    let group = |wanted: fn(&ToolStatus) -> bool| -> Vec<&ToolStatus> {
        summary.tools.iter().filter(|status| wanted(status)).collect()
    };
    let active = group(|s| matches!(s.availability, ToolAvailability::Active));
    let deferred = group(|s| matches!(s.availability, ToolAvailability::Deferred));
    let disabled = group(|s| matches!(s.availability, ToolAvailability::Disabled { .. }));

    if !active.is_empty() {
        out.push_str(&format!("\n\n  In every request ({})\n    ", active.len()));
        out.push_str(&names(&active).join("  "));
    }
    if !deferred.is_empty() {
        out.push_str(&format!(
            "\n\n  Available on demand ({}) — loaded when the agent calls activate_tools\n    ",
            deferred.len()
        ));
        out.push_str(&names(&deferred).join("  "));
    }
    if !disabled.is_empty() {
        out.push_str(&format!("\n\n  Disabled ({})\n", disabled.len()));
        for status in &disabled {
            let by = match &status.availability {
                ToolAvailability::Disabled { source } => source.as_str(),
                _ => "",
            };
            out.push_str(&format!("    {} · by {by}\n", status.name));
        }
        out = out.trim_end().to_string();
    }

    // Which tools cannot be turned off is the config file's and the guide's to say: it is the
    // answer to a question nobody asks before they have tried
    out.push_str(
        "\n\nTurn the agent's tools on or off in `/settings`, on the Tools page.\nUse \
         `luxe --with-tool <name>` or `luxe --no-tool <name>` to turn one on or off for a single \
         session.",
    );
    out
}

fn names(statuses: &[&ToolStatus]) -> Vec<String> {
    statuses.iter().map(|status| status.name.clone()).collect()
}

pub(crate) fn render_lsp(summary: &ToolingSummary) -> String {
    let mut out =
        String::from("Language servers · used by lsp_diagnostics, lsp_navigation, symbol_search");
    if summary.servers.is_empty() {
        out.push_str("\n\n  (none configured)");
    }
    for server in &summary.servers {
        out.push_str(&format!(
            "\n  {} {} · {} · {} · {}{}",
            mark(server.installed),
            server.language,
            server.command,
            exts(&server.extensions),
            source(server.from_config),
            if server.running { " · running" } else { "" },
        ));
        if !server.installed {
            match &server.install_hint {
                Some(hint) => out.push_str(&format!("\n      not installed — {hint}")),
                None => out.push_str("\n      not installed"),
            }
        }
    }
    if let Some(path) = &summary.lsp_config_path {
        out.push_str(&format!(
            "\n\nAdd or override a language in {} (language → server).",
            luxe_core::paths::home_relative(path)
        ));
    }
    out
}

pub(crate) fn render_debug(summary: &ToolingSummary) -> String {
    let mut out = String::from("Debug adapters · used by the debug tool (DAP)");
    if summary.adapters.is_empty() {
        out.push_str("\n\n  (none configured)");
    }
    for adapter in &summary.adapters {
        let routing = if adapter.extensions.is_empty() {
            "compiled binaries".to_string()
        } else {
            exts(&adapter.extensions)
        };
        out.push_str(&format!(
            "\n  {} {} · {} · {} · {}",
            mark(adapter.installed),
            adapter.id,
            adapter.command,
            routing,
            source(adapter.from_config),
        ));
        if !adapter.installed {
            out.push_str("\n      not installed");
        }
    }

    out.push_str("\n\nActive debug sessions:");
    if summary.debug_sessions.is_empty() {
        out.push_str("\n  (none)");
    } else {
        for session in &summary.debug_sessions {
            out.push_str(&format!("\n  • {}", session_line(session)));
        }
    }

    if let Some(path) = &summary.adapters_config_path {
        out.push_str(&format!(
            "\n\nAdd an adapter in {} (see the README for the schema).",
            luxe_core::paths::home_relative(path)
        ));
    }
    out
}

/// Render the `/mcp` status view: configured servers, their transport/auth, and cached tool counts.
pub(crate) fn render_mcp(summary: &ToolingSummary) -> String {
    let mut out = String::from("MCP servers · reached on demand via the `mcp` proxy tool");
    if summary.mcp_servers.is_empty() {
        out.push_str(
            "\n\n  (none configured) — add servers to .mcp.json or .luxe/mcp.json, or import an \
             existing config.",
        );
        return out;
    }
    for server in &summary.mcp_servers {
        out.push_str(&format!("\n  {}", mcp_line(server)));
    }
    out.push_str(
        "\n\nThe agent discovers tools with mcp({ search: \"...\" }); servers connect lazily. \
         Authorize an OAuth server with `luxe mcp login --id <server>`.",
    );
    out
}

fn mcp_line(server: &McpServerStatus) -> String {
    let tools = match server.tool_count {
        Some(n) => format!("{n} tools"),
        None => "not yet discovered".to_string(),
    };
    let state = if server.enabled { "" } else { " · disabled" };
    format!("{} · {} · auth={} · {}{}", server.id, server.transport, server.auth, tools, state)
}

fn session_line(session: &DebugSessionStatus) -> String {
    let state = if session.terminated { "ended" } else { "live" };
    format!("{} · {} · {} · {}", session.id, session.adapter, session.program, state)
}

fn mark(installed: bool) -> &'static str {
    if installed {
        crate::glyphs::Mark::Yes.glyph()
    } else {
        crate::glyphs::Mark::No.glyph()
    }
}

fn source(from_config: bool) -> &'static str {
    if from_config {
        "config"
    } else {
        "built-in"
    }
}

/// Format extensions as a dotted, space-separated list (`.rs .toml`), capped for readability.
fn exts(extensions: &[String]) -> String {
    if extensions.is_empty() {
        return "—".to_string();
    }
    let shown: Vec<String> = extensions.iter().take(6).map(|e| format!(".{e}")).collect();
    let mut joined = shown.join(" ");
    if extensions.len() > 6 {
        joined.push_str(" …");
    }
    joined
}

/// Render the `/images` status view.
///
/// Exists because the honest fallback is quiet by design: an image the terminal cannot draw shows a
/// single line naming it, and repeating *why* on every one of them would be noise. The reader who
/// wants to know asks once, here, and gets the reason and the lever in the same breath.
pub(crate) fn render_images(
    tier: crate::graphics::ImageTier,
    reason: &str,
    max_rows: u16,
) -> String {
    use crate::graphics::ImageTier;

    let (state, detail) = match tier {
        ImageTier::Pixels => (
            "on",
            format!(
                "Screenshots and image files are drawn in the transcript, up to {max_rows} rows \
                 tall (and never more than half the height of the conversation)."
            ),
        ),
        ImageTier::Blocks => (
            "low fidelity",
            "Images are drawn as half-block cells: two pixels per cell, so a picture is \
             recognizable rather than legible. Never chosen automatically."
                .to_string(),
        ),
        ImageTier::Text => (
            "off",
            "Images are named in a line you can click to open them in this desktop's image \
             viewer. Nothing is drawn in the terminal."
                .to_string(),
        ),
    };

    let mut out = String::from("Inline images · what this terminal can draw");
    out.push_str(&format!("\n\n  {state} — {reason}"));
    out.push_str(&format!("\n  {detail}"));
    out.push_str(
        "\n\nReal pixels need the kitty graphics protocol with Unicode placeholders, which kitty \
         and Ghostty support and other terminals do not yet — including inside tmux, which cannot \
         keep track of an image it does not model.",
    );
    out.push_str(
        "\n\nSet \"images\" in config to override: \"pixels\" if your terminal has since learned \
         the protocol, \"blocks\" for a low-fidelity preview anywhere, \"off\" for text only.",
    );
    out
}

#[cfg(test)]
mod tests;
