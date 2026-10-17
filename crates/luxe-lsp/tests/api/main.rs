use luxe_lsp::{
    IndexError, IndexedSymbol, JsonRpcError, JsonRpcMessage, SymbolIndex, SymbolIndexCache,
};
use std::sync::Arc;

#[test]
fn message_and_error_payloads_are_nameable_from_the_root() {
    let error = JsonRpcError { code: -32601, message: "unsupported".into(), data: None };
    let reply = JsonRpcMessage::error_response(7, error.code, error.message, error.data);
    assert_eq!(
        serde_json::to_value(reply).unwrap(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "error": { "code": -32601, "message": "unsupported" }
        })
    );
}

#[tokio::test]
async fn symbol_index_payloads_are_nameable_from_the_root() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("lib.rs"), "pub fn indexed_symbol() {}\n").unwrap();
    let cache = SymbolIndexCache::default();
    let built: Result<Arc<SymbolIndex>, IndexError> = cache.get(project.path(), false).await;
    let index = built.unwrap();
    let hits: Vec<&IndexedSymbol> = index.search("indexed_symbol", 1);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "indexed_symbol");
}
