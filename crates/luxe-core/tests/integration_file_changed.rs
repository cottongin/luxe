//! P5: the FileChanged hook formats a file on save.
//!
//! Proves the format-on-save path end-to-end: fire `FileChanged` for a badly-formatted
//! `.rs` file and assert rustfmt rewrote it. Skipped when rustfmt isn't installed.

#![allow(clippy::unwrap_used)]

use luxe_core::formatter::FormatterRegistry;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookPoint, HookRegistry};
use luxe_core::hooks::file_changed::FileChangedHook;

fn rustfmt_available() -> bool {
    std::process::Command::new("rustfmt")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn file_changed_formats_on_save() {
    if !rustfmt_available() {
        eprintln!("rustfmt not available; skipping format-on-save test");
        return;
    }

    let dir = std::env::temp_dir().join(format!(
        "luxe-fc-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("messy.rs");
    // Deliberately mis-formatted; rustfmt will normalize it.
    std::fs::write(&file, "fn   main( ) {let x=1;println!(\"{}\",x);}\n").unwrap();

    let hook = FileChangedHook::new(Some(FormatterRegistry::default()), None);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let ctx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tx,
        hook_registry: HookRegistry::new(),
    };
    let ev = HookEvent::FileChanged {
        path: file.to_string_lossy().to_string(),
        operation: "write".to_string(),
    };

    let _ = hook.on(HookPoint::FileChanged, &ev, &ctx).await;

    let formatted = std::fs::read_to_string(&file).unwrap();
    assert!(
        formatted.contains("fn main() {") && formatted.contains("    let x = 1;"),
        "rustfmt should have reformatted the file, got:\n{formatted}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
