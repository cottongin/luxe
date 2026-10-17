//! uri — the `file://` URIs the language-server protocol speaks, and the paths behind them.
//!
//! Build and take apart these URIs here, never with `format!("file://{}", …)` or
//! `strip_prefix("file://")`. That works for one shape of path only: an absolute unix path with no
//! character a URI reserves. On Windows it produces `file://C:\src\main.rs`, whose authority is
//! `C:` and whose path is nothing, which every server rejects or misresolves; and on every platform
//! an unencoded space or `#` (`C:\Program Files`, `~/My Notes`) truncates the path.
//!
//! The correct spelling is `file:///C:/src/main.rs` with percent-encoding, and `url` knows all of
//! it (drive letters, UNC shares, the encoding table), so this is a thin wrapper around it.

use std::path::{Path, PathBuf};
use url::Url;

/// The `file://` URI for an absolute path.
///
/// A relative path has no URI, and a server would reject one anyway; rather than refuse mid-request
/// it degrades to the naive spelling with forward slashes, which is what every caller did before and
/// is at least parseable by the [`to_path`] on the other side.
pub(crate) fn from_path(path: &Path) -> String {
    match Url::from_file_path(path) {
        Ok(url) => url.into(),
        Err(()) => format!("file://{}", luxe_core::paths::display(path)),
    }
}

/// The path a `file://` URI names.
///
/// Lenient on the way back: servers are inconsistent about encoding (VS Code sends `file:///c%3A/`,
/// others send `file:///C:/`), and some send a bare path where a URI was expected. Anything that
/// does not parse as a URI is treated as a path already.
pub(crate) fn to_path(uri: &str) -> PathBuf {
    if let Ok(url) = Url::parse(uri) {
        if url.scheme() == "file" {
            if let Ok(path) = url.to_file_path() {
                return path;
            }
        }
    }
    PathBuf::from(uri.strip_prefix("file://").unwrap_or(uri))
}

/// The path a `file://` URI names, rendered for display with forward slashes.
fn to_display_path(uri: &str) -> String {
    luxe_core::paths::display(&to_path(uri))
}

/// The last two path segments of a URI — enough to recognize a file, short enough for a result row.
pub(crate) fn short_display(uri: &str) -> String {
    let path = to_display_path(uri);
    let parts: Vec<&str> = path.rsplit('/').take(2).collect();
    match parts.as_slice() {
        [file, parent] => format!("{parent}/{file}"),
        _ => path,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A URI is not `"file://" + the path`. Three authorities' worth of difference: the empty one
    /// after `file://`, the encoding, and (on Windows) the leading slash before the drive.
    #[test]
    fn a_uri_round_trips_back_to_the_path_it_came_from() {
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\projects\luxe\src\main.rs")
        } else {
            PathBuf::from("/projects/luxe/src/main.rs")
        };
        let uri = from_path(&path);
        assert!(uri.starts_with("file:///"), "three slashes, empty authority: {uri}");
        assert!(!uri.contains('\\'), "a URI has no backslashes: {uri}");
        assert_eq!(to_path(&uri), path);
    }

    /// The bug that was live on every platform, not just Windows: a space is not a URI character,
    /// and an unencoded one truncates the path at the server.
    #[test]
    fn reserved_characters_are_encoded_and_decode_back() {
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\Program Files\my app\a b#c.rs")
        } else {
            PathBuf::from("/opt/Program Files/my app/a b#c.rs")
        };
        let uri = from_path(&path);
        assert!(!uri.contains(' '), "spaces are encoded: {uri}");
        assert!(uri.contains("%20"), "{uri}");
        assert_eq!(to_path(&uri), path, "and survive the trip back");
    }

    /// Servers disagree about how to spell a drive letter; both readings have to work.
    #[cfg(windows)]
    #[test]
    fn either_spelling_of_a_drive_letter_is_understood() {
        let expected = PathBuf::from(r"C:\src\main.rs");
        assert_eq!(to_path("file:///C:/src/main.rs"), expected);
        assert_eq!(to_path("file:///c%3A/src/main.rs"), expected);
    }

    /// Some servers answer with a bare path where the protocol says URI. Treat it as a path rather
    /// than losing the location.
    #[test]
    fn a_bare_path_is_accepted_where_a_uri_was_expected() {
        let bare = if cfg!(windows) { r"C:\src\main.rs" } else { "/src/main.rs" };
        assert_eq!(to_path(bare), PathBuf::from(bare));
    }

    #[test]
    fn a_display_path_uses_forward_slashes_whatever_the_platform() {
        let path = if cfg!(windows) { r"C:\src\main.rs" } else { "/src/main.rs" };
        let rendered = to_display_path(&from_path(Path::new(path)));
        assert!(!rendered.contains('\\'), "{rendered}");
        assert!(rendered.ends_with("/src/main.rs"), "{rendered}");
    }

    /// A result row shows the file and its directory — decoded, so a path with a space reads as one
    /// rather than as `%20`. The fixtures carry a drive letter on Windows because a URI without one
    /// names no file there, and `Url` rightly refuses to invent it.
    #[test]
    fn a_short_display_keeps_two_segments_and_decodes_them() {
        let root = if cfg!(windows) { "file:///C:" } else { "file://" };
        assert_eq!(short_display(&format!("{root}/a/b/c/main.rs")), "c/main.rs");
        assert_eq!(short_display(&format!("{root}/my%20app/main.rs")), "my app/main.rs");
        assert_eq!(short_display("main.rs"), "main.rs", "nothing to shorten");
    }
}
