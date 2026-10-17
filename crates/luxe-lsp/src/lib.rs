// luxe-lsp — Code intelligence: a warm LSP client plus tree-sitter outlines.
//
// Provides diagnostics, IDE-style navigation (definition/references/hover/call hierarchy/…),
// and a discovery funnel (symbol_search → module_report → read_symbol) via the Language Server
// Protocol, backed by warm, reused servers. The `syntax` module adds fast, offline tree-sitter
// outlines so file overviews and symbol reads don't require a running server.

pub mod client;
mod edit_apply;
pub mod graph;
mod message;
pub mod protocol;
pub mod registry;
mod symbol_index;
pub mod syntax;
mod tools;
mod uri;
mod workspace;

pub use message::{JsonRpcError, JsonRpcMessage};
pub use registry::{ServerEntry, ServerEntryPatch, ServerRegistry};

// Re-export public types for use by luxe-cli.
pub use client::{LspClient, LspConfig, LspError};
pub use protocol::{DiagnosticSeverity, Position, TextDocumentSyncKind};
pub use symbol_index::{IndexError, IndexedSymbol, SymbolIndex, SymbolIndexCache};
pub use tools::{
    ImpactTool, LspDiagnosticsTool, LspNavigationTool, ModuleReportTool, ReadSymbolTool,
    SymbolSearchTool,
};
pub use workspace::WorkspaceLspClient;
