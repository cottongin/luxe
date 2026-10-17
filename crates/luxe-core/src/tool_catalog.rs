//! Tool catalog — every built-in tool Luxe can register, with what a person needs to know to
//! decide whether they want it.
//!
//! One list for every surface that names tools without a registry in hand: `luxe init`, the setup
//! surface, the Tools settings page, the deny-list's typo check. The registry is the authority on
//! what a run *has*; this is the authority on what Luxe *offers*, whatever this machine happens to
//! have installed. A CLI test holds the two together, so a tool cannot ship without an entry.

/// How a settings surface groups a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolGroup {
    /// The irreducible core: always registered, never offered as a toggle
    Essential,
    /// Searching, reading and understanding code
    Code,
    /// Reaching outside the machine
    Web,
    /// Planning, version control, and talking to the person
    Workflow,
    /// Machinery the harness registers for itself, never offered as a toggle
    Internal,
}

impl ToolGroup {
    /// The groups a person chooses tools from, in the order they are offered.
    pub const OFFERED: [Self; 3] = [Self::Code, Self::Web, Self::Workflow];

    /// The heading a settings surface prints over the group.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Essential => "ALWAYS ON",
            Self::Code => "CODE",
            Self::Web => "WEB",
            Self::Workflow => "WORKFLOW",
            Self::Internal => "INTERNAL",
        }
    }
}

/// One built-in tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogTool {
    /// The name the model calls it by, and the one `tools.disabled` takes
    pub name: &'static str,
    /// One line on what it does, for a checklist
    pub summary: &'static str,
    /// A sentence or two more, for the pane beside a checklist
    pub detail: &'static str,
    /// Where it is offered
    pub group: ToolGroup,
    /// What has to be on the machine or in the config before it registers at all
    pub needs: Option<&'static str>,
}

impl CatalogTool {
    /// Whether a person may turn this tool off.
    pub fn toggleable(&self) -> bool {
        !matches!(self.group, ToolGroup::Essential | ToolGroup::Internal)
    }
}

const fn tool(
    name: &'static str,
    group: ToolGroup,
    summary: &'static str,
    detail: &'static str,
) -> CatalogTool {
    CatalogTool { name, summary, detail, group, needs: None }
}

const fn needing(mut tool: CatalogTool, needs: &'static str) -> CatalogTool {
    tool.needs = Some(needs);
    tool
}

use ToolGroup::{Code, Essential, Internal, Web, Workflow};

/// Every built-in tool, sorted by name.
pub const CATALOG: &[CatalogTool] = &[
    tool(
        "activate_tools",
        Internal,
        "Load a deferred tool",
        "Lets the model pull in a tool that is not in every request, so rarely used ones cost \
         nothing until they are needed.",
    ),
    needing(
        tool(
            "ast_grep",
            Code,
            "Search and replace by code structure",
            "Matches syntax rather than text, so a rename or a pattern change touches the code \
             and not the comments or strings that look like it.",
        ),
        "the ast-grep program",
    ),
    tool(
        "bash",
        Essential,
        "Run a shell command",
        "Long output is trimmed, and the untouched text saved (each stream whole up to 8 MB).",
    ),
    needing(
        tool(
            "debug",
            Code,
            "Step through code with a debugger",
            "Breakpoints, stepping and inspecting variables, through a debug adapter.",
        ),
        "a debug adapter for the language",
    ),
    tool(
        "edit",
        Essential,
        "Change part of a file",
        "A file must be read before it is edited, and the card shows the diff.",
    ),
    tool(
        "git",
        Workflow,
        "Status, diffs, commits and pull requests",
        "History, branches and commits, and GitHub pull requests, issues and checks through gh.",
    ),
    tool(
        "glob",
        Code,
        "Find files by name",
        "Matches paths by pattern, with capped output and the project's ignore rules applied.",
    ),
    tool(
        "grep",
        Code,
        "Search file contents",
        "Searches the project's text, with capped output and the project's ignore rules applied.",
    ),
    tool(
        "impact",
        Code,
        "What else a change affects",
        "The callers, dependents and covering tests of a symbol or file, from an offline code \
         graph.",
    ),
    tool(
        "loop_control",
        Internal,
        "Report progress in a loop",
        "How the agent says a phase is done when it runs under luxe loop.",
    ),
    tool(
        "lsp_diagnostics",
        Code,
        "Errors and warnings from a language server",
        "Asks the project's language server what is wrong, the way an editor does.",
    ),
    tool(
        "lsp_navigation",
        Code,
        "Definitions, references and types",
        "Goes to a definition, finds references and reads types through a language server.",
    ),
    needing(
        tool(
            "mcp",
            Workflow,
            "Call tools on your MCP servers",
            "One gateway to every MCP server you configured; each call still goes through your \
             permission rules.",
        ),
        "an MCP server in mcp.json",
    ),
    tool(
        "module_report",
        Code,
        "Outline a file's symbols",
        "A file's functions and types with their line ranges, without reading the whole file.",
    ),
    tool(
        "question",
        Workflow,
        "Ask you a question",
        "Asks one question at a time with choices in a picker, and marks the one it recommends.",
    ),
    tool(
        "read",
        Essential,
        "Read files",
        "Several at once, with numbered lines; an image is read as a picture for models that can \
         see.",
    ),
    tool(
        "read_symbol",
        Code,
        "Read one function or type",
        "The source of one named symbol, without the rest of its file.",
    ),
    needing(
        tool(
            "screenshot",
            Workflow,
            "Capture a web page or your dev server",
            "Takes a picture of a page so the agent can check what it built.",
        ),
        "a Chromium-based browser",
    ),
    tool(
        "skill",
        Workflow,
        "Load a skill",
        "Reads a SKILL.md playbook when a task matches what it describes.",
    ),
    tool(
        "symbol_search",
        Code,
        "Find symbols by name",
        "Ranked search over the project's functions and types, from an offline index.",
    ),
    tool(
        "test",
        Code,
        "Run the tests and read the failures",
        "Runs cargo, pytest, go or jest and reports each failure with its file and line.",
    ),
    tool(
        "todo",
        Workflow,
        "Keep a checklist",
        "The agent's plan for the current task, shown in the dock's PLAN.",
    ),
    tool(
        "web_fetch",
        Web,
        "Read a page or a repository",
        "Fetches a URL as text; a GitHub repository is cloned so its files can be searched.",
    ),
    tool(
        "web_search",
        Web,
        "Search the web",
        "Returns titles, links and snippets. Needs no key; a vendor key routes through it.",
    ),
    tool(
        "write",
        Essential,
        "Write a file",
        "Creates or replaces a file, after formatting, and the card shows the diff.",
    ),
];

/// The catalog entry for `name`.
pub fn get(name: &str) -> Option<&'static CatalogTool> {
    CATALOG.iter().find(|tool| tool.name == name)
}

/// Every tool name, sorted.
pub fn names() -> impl Iterator<Item = &'static str> {
    CATALOG.iter().map(|tool| tool.name)
}

/// The tools a person may turn on and off, in group order and then by name.
pub fn toggleable() -> Vec<&'static CatalogTool> {
    ToolGroup::OFFERED
        .iter()
        .flat_map(|group| CATALOG.iter().filter(move |tool| tool.group == *group))
        .collect()
}

/// The tools that cannot be turned off, by name.
pub fn essential() -> impl Iterator<Item = &'static str> {
    CATALOG.iter().filter(|tool| tool.group == ToolGroup::Essential).map(|tool| tool.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_sorted_and_has_no_duplicates() {
        assert!(CATALOG.windows(2).all(|pair| pair[0].name < pair[1].name));
    }

    /// The essential group is exactly the set the deny-list refuses to remove, so a settings page
    /// can never offer a toggle the engine would ignore.
    #[test]
    fn the_essential_group_is_the_set_the_deny_list_refuses() {
        let mut essential: Vec<&str> = essential().collect();
        essential.sort_unstable();
        let mut refused = crate::config::ESSENTIAL_TOOLS.to_vec();
        refused.sort_unstable();
        assert_eq!(essential, refused);
    }

    #[test]
    fn only_people_facing_tools_are_offered() {
        let offered: Vec<&str> = toggleable().iter().map(|tool| tool.name).collect();
        for hidden in ["read", "write", "edit", "bash", "activate_tools", "loop_control"] {
            assert!(!offered.contains(&hidden), "{hidden} is offered as a toggle");
        }
        for wanted in ["web_search", "todo", "impact", "read_symbol", "mcp", "debug"] {
            assert!(offered.contains(&wanted), "{wanted} is not offered");
        }
        assert_eq!(offered.first(), Some(&"ast_grep"), "code first, by name");
    }

    #[test]
    fn every_entry_has_words_and_a_known_tool_is_found() {
        for tool in CATALOG {
            assert!(!tool.summary.is_empty() && !tool.detail.is_empty(), "{}", tool.name);
            assert!(!tool.summary.ends_with('.'), "a summary is a label: {}", tool.name);
            assert!(tool.detail.ends_with('.'), "a detail is a sentence: {}", tool.name);
        }
        assert_eq!(get("web_search").map(|tool| tool.group), Some(ToolGroup::Web));
        assert!(get("websearch").is_none());
    }
}
