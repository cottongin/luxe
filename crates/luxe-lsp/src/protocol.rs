// LSP protocol types and constants.

use serde::{Deserialize, Serialize};

/// Text document sync kind.
///
/// Wire format is an integer per the LSP spec (`TextDocumentSyncKind = 0|1|2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum TextDocumentSyncKind {
    None = 0,
    Full = 1,
    Incremental = 2,
}

impl TryFrom<u8> for TextDocumentSyncKind {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, String> {
        match v {
            0 => Ok(Self::None),
            1 => Ok(Self::Full),
            2 => Ok(Self::Incremental),
            _ => Err(format!("invalid TextDocumentSyncKind: {}", v)),
        }
    }
}

impl From<TextDocumentSyncKind> for u8 {
    fn from(v: TextDocumentSyncKind) -> u8 {
        v as u8
    }
}

/// Diagnostic severity levels.
///
/// Wire format is an integer per the LSP spec (3.17: `severity?: number`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl TryFrom<u8> for DiagnosticSeverity {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, String> {
        match v {
            1 => Ok(Self::Error),
            2 => Ok(Self::Warning),
            3 => Ok(Self::Information),
            4 => Ok(Self::Hint),
            _ => Err(format!("invalid DiagnosticSeverity: {}", v)),
        }
    }
}

impl From<DiagnosticSeverity> for u8 {
    fn from(v: DiagnosticSeverity) -> u8 {
        v as u8
    }
}

/// Diagnostic tags.
///
/// Wire format is an integer per the LSP spec (`DiagnosticTag = 1|2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum DiagnosticTag {
    Unnecessary = 1,
    Deprecated = 2,
}

impl TryFrom<u8> for DiagnosticTag {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, String> {
        match v {
            1 => Ok(Self::Unnecessary),
            2 => Ok(Self::Deprecated),
            _ => Err(format!("invalid DiagnosticTag: {}", v)),
        }
    }
}

impl From<DiagnosticTag> for u8 {
    fn from(v: DiagnosticTag) -> u8 {
        v as u8
    }
}

/// A location in a text document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    /// URI of the text document.
    pub uri: String,

    /// Range within the document.
    pub range: Range,
}

/// A range in a text document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    /// Start position.
    pub start: Position,

    /// End position.
    pub end: Position,
}

/// A position in a text document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Line number (0-indexed).
    pub line: u32,

    /// Character offset (0-indexed).
    pub character: u32,
}

/// A diagnostic message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Range within the document.
    pub range: Range,

    /// Severity level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<DiagnosticSeverity>,

    /// Source of the diagnostic (e.g., "clippy").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Diagnostic code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<serde_json::Value>,

    /// Message text.
    pub message: String,
}

/// A hover result with markup content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hover {
    /// Markup content (markdown).
    pub contents: MarkupContent,

    /// Range to highlight (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

/// Markup content for hover results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkupContent {
    /// Markup language (e.g., "markdown").
    pub kind: String,

    /// Value (markdown text).
    pub value: String,
}

/// A symbol information object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInformation {
    /// Name of the symbol.
    pub name: String,

    /// Kind of symbol (e.g., "function", "class").
    pub kind: u32,

    /// Tags for the symbol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<SymbolTag>>,

    /// Location of the symbol.
    pub location: Location,

    /// Optional parent name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
}

/// Symbol kind tags.
///
/// Wire format is an integer per the LSP spec (`SymbolTag = 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum SymbolTag {
    Deprecated = 1,
}

impl TryFrom<u8> for SymbolTag {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, String> {
        match v {
            1 => Ok(Self::Deprecated),
            _ => Err(format!("invalid SymbolTag: {}", v)),
        }
    }
}

impl From<SymbolTag> for u8 {
    fn from(v: SymbolTag) -> u8 {
        v as u8
    }
}

/// A normalized symbol result, flattened from either `SymbolInformation` (flat, has `location`) or
/// `DocumentSymbol` (hierarchical, has `range`/`selectionRange`/`children`) so callers don't care
/// which shape a server returns. `uri` is empty for document-scoped results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatSymbol {
    /// Symbol name.
    pub name: String,
    /// LSP `SymbolKind` integer.
    pub kind: u32,
    /// Document URI (empty for `documentSymbol` results).
    pub uri: String,
    /// 0-based line of the symbol's selection (name) start — the navigation position.
    pub line: u32,
    /// 0-based character of the symbol's selection (name) start.
    pub character: u32,
    /// 0-based first line of the symbol's full extent (its body).
    pub start_line: u32,
    /// 0-based last line of the symbol's full extent (its body).
    pub end_line: u32,
    /// Enclosing container/parent name, if any.
    pub container: Option<String>,
}

/// Human-readable name for an LSP `SymbolKind` integer.
pub fn symbol_kind_name(kind: u32) -> &'static str {
    use symbol_kinds as k;
    match kind {
        k::FILE => "file",
        k::MODULE => "module",
        k::NAMESPACE => "namespace",
        k::PACKAGE => "package",
        k::CLASS => "class",
        k::METHOD => "method",
        k::PROPERTY => "property",
        k::FIELD => "field",
        k::CONSTRUCTOR => "constructor",
        k::ENUM => "enum",
        k::INTERFACE => "interface",
        k::FUNCTION => "function",
        k::VARIABLE => "variable",
        k::CONSTANT => "constant",
        k::STRUCT => "struct",
        k::ENUM_MEMBER => "enum-member",
        k::TYPE_PARAMETER => "type-param",
        k::OPERATOR => "operator",
        k::EVENT => "event",
        _ => "symbol",
    }
}

/// Symbol kinds (from LSP spec), minus the value kinds 15–21 that nothing here labels.
pub(crate) mod symbol_kinds {
    pub(crate) const FILE: u32 = 1;
    pub(crate) const MODULE: u32 = 2;
    pub(crate) const NAMESPACE: u32 = 3;
    pub(crate) const PACKAGE: u32 = 4;
    pub(crate) const CLASS: u32 = 5;
    pub(crate) const METHOD: u32 = 6;
    pub(crate) const PROPERTY: u32 = 7;
    pub(crate) const FIELD: u32 = 8;
    pub(crate) const CONSTRUCTOR: u32 = 9;
    pub(crate) const ENUM: u32 = 10;
    pub(crate) const INTERFACE: u32 = 11;
    pub(crate) const FUNCTION: u32 = 12;
    pub(crate) const VARIABLE: u32 = 13;
    pub(crate) const CONSTANT: u32 = 14;
    pub(crate) const ENUM_MEMBER: u32 = 22;
    pub(crate) const STRUCT: u32 = 23;
    pub(crate) const EVENT: u32 = 24;
    pub(crate) const OPERATOR: u32 = 25;
    pub(crate) const TYPE_PARAMETER: u32 = 26;
}

/// Document highlight kind.
///
/// Wire format is an integer per the LSP spec (`DocumentHighlightKind = 1|2|3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum DocumentHighlightKind {
    Text = 1,
    Read = 2,
    Write = 3,
}

impl TryFrom<u8> for DocumentHighlightKind {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, String> {
        match v {
            1 => Ok(Self::Text),
            2 => Ok(Self::Read),
            3 => Ok(Self::Write),
            _ => Err(format!("invalid DocumentHighlightKind: {}", v)),
        }
    }
}

impl From<DocumentHighlightKind> for u8 {
    fn from(v: DocumentHighlightKind) -> u8 {
        v as u8
    }
}

/// A document highlight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentHighlight {
    /// Range of the highlight.
    pub range: Range,

    /// Kind of highlight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<DocumentHighlightKind>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_serialization() {
        let pos = Position { line: 10, character: 5 };
        let json = serde_json::to_string(&pos).unwrap();
        assert!(json.contains("10"));
        assert!(json.contains("5"));
    }

    #[test]
    fn test_range_serialization() {
        let range = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 1, character: 10 },
        };
        let json = serde_json::to_string(&range).unwrap();
        assert!(json.contains("0"));
        assert!(json.contains("10"));
    }

    #[test]
    fn test_diagnostic_serialization() {
        let diag = Diagnostic {
            range: Range {
                start: Position { line: 5, character: 0 },
                end: Position { line: 5, character: 10 },
            },
            severity: Some(DiagnosticSeverity::Error),
            source: Some("clippy".to_string()),
            code: Some(serde_json::json!("clippy::unnecessary_clone")),
            message: "This clone is unnecessary".to_string(),
        };
        let json = serde_json::to_string(&diag).unwrap();
        assert!(json.contains("1")); // Error severity = 1
        assert!(json.contains("clippy"));
        assert!(json.contains("This clone is unnecessary"));
    }

    #[test]
    fn test_symbol_kinds() {
        assert_eq!(symbol_kinds::FUNCTION, 12);
        assert_eq!(symbol_kinds::CLASS, 5);
        assert_eq!(symbol_kinds::VARIABLE, 13);
    }

    /// Every LSP enum here must (de)serialize as an integer — real servers send
    /// numbers, not strings. A regression to string repr silently drops diagnostics.
    #[test]
    fn test_enums_use_integer_wire_format() {
        assert_eq!(serde_json::to_string(&DiagnosticSeverity::Warning).unwrap(), "2");
        assert_eq!(serde_json::to_string(&TextDocumentSyncKind::Incremental).unwrap(), "2");
        assert_eq!(serde_json::to_string(&DiagnosticTag::Deprecated).unwrap(), "2");
        assert_eq!(serde_json::to_string(&SymbolTag::Deprecated).unwrap(), "1");
        assert_eq!(serde_json::to_string(&DocumentHighlightKind::Write).unwrap(), "3");

        assert_eq!(
            serde_json::from_str::<DiagnosticSeverity>("1").unwrap(),
            DiagnosticSeverity::Error
        );
        assert_eq!(
            serde_json::from_str::<TextDocumentSyncKind>("0").unwrap(),
            TextDocumentSyncKind::None
        );
        assert_eq!(serde_json::from_str::<DiagnosticTag>("1").unwrap(), DiagnosticTag::Unnecessary);
        assert_eq!(
            serde_json::from_str::<DocumentHighlightKind>("2").unwrap(),
            DocumentHighlightKind::Read
        );

        // Strings (the old buggy wire format) and out-of-range ints must be rejected.
        assert!(serde_json::from_str::<DiagnosticSeverity>("\"error\"").is_err());
        assert!(serde_json::from_str::<DiagnosticSeverity>("99").is_err());
        assert!(serde_json::from_str::<DocumentHighlightKind>("\"write\"").is_err());
    }
}
