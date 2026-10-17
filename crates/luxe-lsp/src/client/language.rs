//! Languages — which `languageId` a file's extension is opened under.

/// The LSP `languageId` for a document, by extension.
///
/// These are the identifiers the specification enumerates, not free-form names: `typescriptreact`
/// for `.tsx`, `c` and `cpp` as separate languages, `csharp` rather than `cs`. A server is entitled
/// to reject or mishandle one it does not know, and the wrong one is worse than a missing one
/// because it asserts something false about the file.
pub(super) fn language_id_for_extension(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" => "c",
        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" => "cpp",
        "h" => "c",
        "java" => "java",
        "rb" => "ruby",
        "php" => "php",
        "cs" => "csharp",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "lua" => "lua",
        "zig" => "zig",
        "sh" | "bash" => "shellscript",
        "json" => "json",
        "jsonc" => "jsonc",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "html" => "html",
        "css" => "css",
        _ => return None,
    })
}
