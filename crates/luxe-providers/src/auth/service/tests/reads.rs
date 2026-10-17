//! Listing providers reads the two config files once, not once per row.
#![cfg(target_os = "linux")]

use super::*;

/// Bytes this process has asked the kernel to read, from `/proc/self/io`.
fn bytes_read() -> Option<u64> {
    let io = std::fs::read_to_string("/proc/self/io").ok()?;
    io.lines().find_map(|line| line.strip_prefix("rchar: ")?.trim().parse().ok())
}

/// `providers()` called `status()` per catalog entry, and each of those re-read and parsed
/// `auth.json` and `models.json`, so a first-run check on a real config meant dozens of parses.
///
/// Measured in bytes rather than counted in code, so a read added back anywhere in the listing
/// shows up whichever function does it. Both files carry a megabyte of padding to dwarf what the
/// rest of the process reads, and each should cost that once: the counter is the whole process's,
/// so the bound leaves room for another test's small reads under `cargo test`.
#[test]
fn listing_the_providers_reads_each_file_once() {
    let (dir, service) = service();
    let padding = "x".repeat(1 << 20);
    let models = serde_json::json!({
        "providers": { "mine": { "base_url": "http://127.0.0.1:1/v1", "api_key": "k" } },
        "padding": padding,
    });
    std::fs::write(dir.path().join("models.json"), models.to_string()).unwrap();
    let auth = serde_json::json!({ "padding": padding });
    std::fs::write(dir.path().join("auth.json"), auth.to_string()).unwrap();

    let before = bytes_read().expect("/proc/self/io, which Linux counts reads in");
    let options = service.providers();
    let read = bytes_read().expect("still readable") - before;

    assert!(options.iter().any(|option| option.id == "mine"), "the hand-written block is listed");
    // Two files, one read each; one per catalog entry would be ~50 of them.
    assert!(read < 3 << 20, "read {} MiB listing the providers", read >> 20);
}
