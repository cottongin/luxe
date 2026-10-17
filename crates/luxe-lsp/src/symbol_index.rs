//! Offline BM25 symbol index.
//!
//! Walks a project, extracts symbols with the tree-sitter [`crate::syntax`] outliner, and ranks
//! them for `symbol_search` **without a language server** — across every tree-sitter-supported
//! language at once. Names are tokenized on camelCase / snake_case / non-alphanumeric boundaries;
//! ranking is exact → prefix → BM25 over the name tokens.
//!
//! The files come from [`luxe_core::walk`], the walk `glob` and `grep` use, so the index honors the
//! same ignore files, sees them in sorted order, and says when it stopped short.

use crate::protocol::symbol_kind_name;
use crate::syntax;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const K1: f64 = 1.2;
const B: f64 = 0.75;
/// Directories never descended into, ignore files or not: vendored and generated trees. Hidden
/// ones (`.venv`, `.luxe`, `.yarn`) the walk already skips.
const IGNORED_DIRS: &[&str] = &["target", "node_modules", "dist", "build", "vendor"];
/// Safety caps so a pathological repo can't blow up the walk.
pub(crate) const MAX_FILES: usize = 5000;
const MAX_SYMBOLS: usize = 50_000;

/// One indexed symbol (project-relative).
#[derive(Debug, Clone)]
pub struct IndexedSymbol {
    /// Symbol name.
    pub name: String,
    /// LSP `SymbolKind` integer.
    pub kind: u32,
    /// Project-relative file path.
    pub path: String,
    /// 1-based line of the symbol's name.
    pub line: u32,
    /// Enclosing container/parent name, if any.
    pub container: Option<String>,
}

/// A built BM25 index over a project's symbols.
pub struct SymbolIndex {
    symbols: Vec<IndexedSymbol>,
    tokens: Vec<Vec<String>>,
    df: HashMap<String, usize>,
    avgdl: f64,
    capped: bool,
}

impl SymbolIndex {
    /// Build the index by walking `root` and outlining every supported source file.
    ///
    /// Blocking (filesystem + parsing); call from `spawn_blocking`.
    pub fn build(root: &Path) -> Self {
        let SourceFiles { files, capped } = source_files(root);

        let mut symbols = Vec::new();
        for file in files {
            if symbols.len() >= MAX_SYMBOLS {
                break;
            }
            let Ok(source) = std::fs::read_to_string(&file) else { continue };
            let Some(outline) = syntax::outline(&file, &source) else { continue };
            let rel = luxe_core::paths::display(file.strip_prefix(root).unwrap_or(&file));
            for sym in outline {
                symbols.push(IndexedSymbol {
                    name: sym.name,
                    kind: sym.kind,
                    path: rel.clone(),
                    line: sym.line + 1,
                    container: sym.container,
                });
            }
        }

        let tokens: Vec<Vec<String>> = symbols.iter().map(|s| tokenize(&s.name)).collect();
        let mut df: HashMap<String, usize> = HashMap::new();
        for doc in &tokens {
            for term in doc.iter().collect::<std::collections::HashSet<_>>() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }
        }
        let total: usize = tokens.iter().map(Vec::len).sum();
        let avgdl = if tokens.is_empty() { 1.0 } else { total as f64 / tokens.len() as f64 };

        Self { symbols, tokens, df, avgdl, capped }
    }

    /// The line a search adds when the walk stopped at its file limit, so a symbol past it is
    /// missing, not absent; or nothing.
    pub fn capped_note(&self) -> String {
        if !self.capped {
            return String::new();
        }
        format!("  (index capped at {MAX_FILES} files; a symbol past it is missing, not absent)\n")
    }

    /// Whether the index found no symbols (e.g. no tree-sitter-supported files).
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// Total indexed symbols.
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// Ranked search: exact name → prefix → BM25 over name tokens. Returns up to `limit` hits.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&IndexedSymbol> {
        self.search_with_prior(query, limit, |_| 1.0)
    }

    /// Like [`SymbolIndex::search`] but multiplies each hit's score by `prior(symbol)` (clamped to
    /// `>= 1.0`) before ranking — so a caller can fold in a graph-centrality prior (rank central
    /// symbols higher) without the index having to know about the graph. Tiers (exact → prefix →
    /// token) still dominate; the prior only reorders within a tier.
    pub fn search_with_prior(
        &self,
        query: &str,
        limit: usize,
        prior: impl Fn(&IndexedSymbol) -> f64,
    ) -> Vec<&IndexedSymbol> {
        let query_lower = query.to_lowercase();
        let query_tokens = unique(tokenize(query));
        let n = self.symbols.len() as f64;

        let mut scored: Vec<(u8, f64, usize)> = self
            .symbols
            .iter()
            .enumerate()
            .filter_map(|(index, sym)| {
                let name_lower = sym.name.to_lowercase();
                let tier = if name_lower == query_lower {
                    0
                } else if name_lower.starts_with(&query_lower) {
                    1
                } else {
                    2
                };
                let score = self.bm25(&query_tokens, index, n);
                // Keep exact/prefix hits regardless; keep token hits only if they scored. The prior
                // (>= 1.0) can't resurrect a zero BM25 score, so token filtering is unchanged.
                (tier < 2 || score > 0.0).then_some((tier, score * prior(sym).max(1.0), index))
            })
            .collect();

        scored.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| self.symbols[a.2].name.cmp(&self.symbols[b.2].name))
        });
        scored.into_iter().take(limit).map(|(_, _, index)| &self.symbols[index]).collect()
    }

    fn bm25(&self, query_tokens: &[String], doc: usize, n: f64) -> f64 {
        let doc_tokens = &self.tokens[doc];
        let dl = doc_tokens.len() as f64;
        let mut score = 0.0;
        for term in query_tokens {
            let tf = doc_tokens.iter().filter(|t| *t == term).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let df = self.df.get(term).copied().unwrap_or(0) as f64;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            score += idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * dl / self.avgdl));
        }
        score
    }
}

pub(crate) fn format_hit(sym: &IndexedSymbol) -> String {
    let container = sym.container.as_deref().map(|c| format!(" in {c}")).unwrap_or_default();
    format!("{} ({}) {}:{}{container}", sym.name, symbol_kind_name(sym.kind), sym.path, sym.line)
}

/// The source files an index or graph reads, and whether there were more than it would take.
pub(crate) struct SourceFiles {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) capped: bool,
}

/// Tree-sitter-supported source files under `root`, sorted, at most [`MAX_FILES`] of them. Shared
/// with the code graph so both see the same file set.
pub(crate) fn source_files(root: &Path) -> SourceFiles {
    let walked = luxe_core::walk::index_files(root, IGNORED_DIRS, syntax::supported, MAX_FILES);
    let files = walked.files.into_iter().map(|path| root.join(path)).collect();
    SourceFiles { files, capped: walked.capped }
}

/// Tokenize an identifier on camelCase / snake_case / non-alphanumeric boundaries, lowercased.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut prev_lower_or_digit = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && prev_lower_or_digit && !current.is_empty() {
                tokens.push(std::mem::take(&mut current).to_lowercase());
            }
            current.push(ch);
            prev_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current).to_lowercase());
            }
            prev_lower_or_digit = false;
        }
    }
    if !current.is_empty() {
        tokens.push(current.to_lowercase());
    }
    tokens
}

fn unique(mut tokens: Vec<String>) -> Vec<String> {
    tokens.sort();
    tokens.dedup();
    tokens
}

/// Why there is no index to search.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// The build panicked or was cancelled.
    #[error("the symbol index could not be built: {0}")]
    Build(#[source] tokio::task::JoinError),
}

/// The warm index for one project root, rebuilt when the root changes or a file does.
///
/// Shared between the `symbol_search` tool and the file-changed hook, which is what keeps a symbol
/// the agent just wrote findable on the next search.
#[derive(Default)]
pub struct SymbolIndexCache {
    built: tokio::sync::Mutex<Option<(PathBuf, std::sync::Arc<SymbolIndex>)>>,
}

impl SymbolIndexCache {
    /// The index for `root`, built if there is none for it yet or `refresh` asks.
    pub async fn get(
        &self,
        root: &Path,
        refresh: bool,
    ) -> Result<std::sync::Arc<SymbolIndex>, IndexError> {
        let mut built = self.built.lock().await;
        if let Some((indexed, index)) = built.as_ref().filter(|_| !refresh) {
            if indexed == root {
                return Ok(std::sync::Arc::clone(index));
            }
        }
        let owned = root.to_path_buf();
        let index = tokio::task::spawn_blocking(move || SymbolIndex::build(&owned))
            .await
            .map_err(IndexError::Build)?;
        let index = std::sync::Arc::new(index);
        *built = Some((root.to_path_buf(), std::sync::Arc::clone(&index)));
        Ok(index)
    }

    /// Forgets the index, so the next search rebuilds it from what is on disk now.
    pub async fn invalidate(&self) {
        *self.built.lock().await = None;
    }
}

#[cfg(test)]
mod tests;
