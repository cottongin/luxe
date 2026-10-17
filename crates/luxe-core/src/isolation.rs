//! Isolation — whether this process runs as root or inside a container.
//!
//! Both are where a browser's own sandbox cannot start, and the one place that decides it should
//! ask here rather than guess from a file's owner or an environment variable.

/// Whether the effective user is root. Always `false` where there is no such thing.
pub fn running_as_root() -> bool {
    #[cfg(unix)]
    return rustix::process::geteuid().is_root();
    #[cfg(not(unix))]
    false
}

/// Whether this process runs inside a Docker or Podman container, by the marker each one leaves.
pub fn in_container() -> bool {
    ["/.dockerenv", "/run/.containerenv"].iter().any(|marker| std::path::Path::new(marker).exists())
}
