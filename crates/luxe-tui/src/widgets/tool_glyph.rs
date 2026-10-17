//! tool_glyph — which icon a tool card wears.
//!
//! A card's outcome is already said twice over — the tool name takes the success/error color and the
//! band behind it takes a wash of the same hue — so the leading glyph is free to say something the
//! card doesn't otherwise carry: *which* tool ran. A reader scanning a long transcript picks a `bash`
//! out of a run of `read`s by shape, before reading a word.
//!
//! The shapes themselves live in [`crate::glyphs`], with a rendering per fidelity tier; this module
//! only decides which icon a tool *name* maps to. Aliases share their primary's icon
//! (`patch`/`apply_patch` are `edit`; `webfetch` is `web_fetch`), because a card should read by what
//! the call did, not by which spelling the model reached for.

use crate::glyphs::Icon;

/// The icon for a tool with no bespoke shape of its own — also the icon for a mixed run of calls.
pub const GENERIC_TOOL: Icon = Icon::ToolGeneric;

/// The icon `tool` is carded with, falling back to [`GENERIC_TOOL`] for a name this map has never
/// heard of — a newly registered tool, or one proxied under an unfamiliar name.
pub fn tool_icon(tool: &str) -> Icon {
    match tool {
        "read" => Icon::ToolRead,
        "write" => Icon::ToolWrite,
        "edit" | "patch" | "apply_patch" => Icon::ToolEdit,
        "bash" => Icon::ToolBash,
        "grep" => Icon::ToolGrep,
        "glob" => Icon::ToolGlob,
        "ast_grep" => Icon::ToolAstGrep,
        "git" => Icon::ToolGit,
        "test" => Icon::ToolTest,
        "todo" => Icon::ToolTodo,
        "skill" => Icon::ToolSkill,
        // The agent's question to the user is the same job the question overlay does, so it wears the
        // same icon.
        "question" => Icon::Question,
        "web_fetch" | "webfetch" => Icon::ToolWebFetch,
        "web_search" | "websearch" => Icon::ToolWebSearch,
        "screenshot" => Icon::ToolScreenshot,
        "loop_control" => Icon::ToolLoopControl,
        "lsp_diagnostics" => Icon::ToolDiagnostics,
        "lsp_navigation" => Icon::ToolNavigation,
        "symbol_search" => Icon::ToolSymbolSearch,
        "module_report" => Icon::ToolModuleReport,
        "read_symbol" => Icon::ToolReadSymbol,
        "impact" => Icon::ToolImpact,
        "debug" => Icon::ToolDebug,
        "mcp" => Icon::ToolMcp,
        "activate_tools" | "activate" => Icon::ToolActivate,
        _ => GENERIC_TOOL,
    }
}

/// The glyph for `tool` at the resolved fidelity tier.
pub fn tool_glyph(tool: &str) -> &'static str {
    tool_icon(tool).glyph()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registered tool — the built-ins, the LSP/graph funnel, the debugger, the MCP gateway. A
    /// new tool without an entry here still renders (the generic wrench), but the point of the map is
    /// that the ones a session actually hits are recognizable by shape.
    const REGISTERED_TOOLS: &[&str] = &[
        "read",
        "write",
        "edit",
        "bash",
        "grep",
        "glob",
        "ast_grep",
        "git",
        "test",
        "todo",
        "skill",
        "question",
        "web_fetch",
        "web_search",
        "screenshot",
        "loop_control",
        "lsp_diagnostics",
        "lsp_navigation",
        "symbol_search",
        "module_report",
        "read_symbol",
        "impact",
        "debug",
        "mcp",
        "activate_tools",
    ];

    #[test]
    fn every_registered_tool_has_an_icon_of_its_own() {
        for tool in REGISTERED_TOOLS {
            assert_ne!(tool_icon(tool), GENERIC_TOOL, "{tool} falls back to the generic icon");
        }
    }

    /// Two tools sharing an icon would defeat the whole point: telling calls apart at a glance. The
    /// shapes are checked in [`crate::glyphs`]; what this guards is the *mapping* — two names landing
    /// on one icon.
    #[test]
    fn no_two_tools_map_to_one_icon() {
        let mut seen = std::collections::HashMap::new();
        for tool in REGISTERED_TOOLS {
            if let Some(other) = seen.insert(tool_icon(tool), *tool) {
                panic!("{tool} and {other} share an icon");
            }
        }
    }

    /// Spelling variants describe the same call, so they can't look like different tools.
    #[test]
    fn aliases_share_their_primary_icon() {
        assert_eq!(tool_icon("patch"), tool_icon("edit"));
        assert_eq!(tool_icon("apply_patch"), tool_icon("edit"));
        assert_eq!(tool_icon("webfetch"), tool_icon("web_fetch"));
        assert_eq!(tool_icon("websearch"), tool_icon("web_search"));
        assert_eq!(tool_icon("activate"), tool_icon("activate_tools"));
    }

    /// A name nobody registered still cards, at every fidelity.
    #[test]
    fn an_unknown_tool_falls_back_at_every_tier() {
        use crate::glyphs::GlyphTier;
        for tier in [GlyphTier::Nerd, GlyphTier::Unicode, GlyphTier::Ascii] {
            assert_eq!(
                tool_icon("definitely_not_a_tool").at(tier),
                GENERIC_TOOL.at(tier),
                "{tier:?}"
            );
        }
    }
}
