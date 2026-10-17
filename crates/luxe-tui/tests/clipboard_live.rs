//! Live clipboard write, against the machine's real clipboard.
//!
//! This is the check that catches the class of bug where a copy lands somewhere the desktop's own
//! paste never looks — `arboard` built without `wayland-data-control` sets XWayland's X11 selection,
//! which `wl-paste` and a compositor keybinding never see, and which evaporates when the process
//! that owns it exits.
//!
//! **Opt-in**: it replaces whatever is on the clipboard, so a normal gate run must not do it. Run it
//! with `LUXE_CLIPBOARD_LIVE=1 cargo nextest run -p luxe-tui --features clipboard --test
//! clipboard_live`.

#![allow(clippy::unwrap_used)]

#[cfg(all(feature = "clipboard", target_os = "linux"))]
#[tokio::test]
async fn a_copy_is_readable_by_the_wayland_clipboard() {
    use luxe_tui::clipboard::create_clipboard;

    if std::env::var_os("LUXE_CLIPBOARD_LIVE").is_none() {
        eprintln!("skipped: set LUXE_CLIPBOARD_LIVE=1 to let this replace the clipboard");
        return;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("skipped: no WAYLAND_DISPLAY");
        return;
    }
    if tokio::process::Command::new("wl-paste").arg("--version").output().await.is_err() {
        eprintln!("skipped: wl-paste unavailable");
        return;
    }

    let payload = format!("luxe clipboard check {}", std::process::id());
    let sink = create_clipboard().await;
    sink.write_text(&payload).await.expect("the write reaches a backend");

    // wl-paste is the same path a compositor keybinding or another Wayland app would use.
    let pasted = tokio::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .await
        .expect("wl-paste runs");
    let pasted = String::from_utf8_lossy(&pasted.stdout).to_string();
    assert_eq!(pasted, payload, "the Wayland clipboard must hold what Luxe just copied");
}
