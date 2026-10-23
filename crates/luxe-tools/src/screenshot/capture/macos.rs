//! macOS window capture — finds a window on any desktop and hands its id to `screencapture -l`.
//!
//! `screencapture` takes a window by id and can photograph one on a desktop (Space) that isn't the
//! one on screen, which a fullscreen or region capture never can. Ids come from CoreGraphics'
//! window list, read through `osascript`'s JavaScript so nothing beyond the OS is needed.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tokio::process::Command;

/// Lists every window on every desktop as JSON, front to back.
///
/// `kCGWindowListOptionAll` rather than on-screen only: that is what makes a window on another
/// desktop visible to us at all.
const LIST_WINDOWS_SCRIPT: &str = r#"
ObjC.import('CoreGraphics');
const raw = $.CGWindowListCopyWindowInfo($.kCGWindowListOptionAll, 0);
const list = ObjC.deepUnwrap(ObjC.castRefToObject(raw));
JSON.stringify(list.map(w => ({
  id: w.kCGWindowNumber, owner: w.kCGWindowOwnerName || '', name: w.kCGWindowName || '',
  layer: w.kCGWindowLayer, w: w.kCGWindowBounds.Width, h: w.kCGWindowBounds.Height,
  on: !!w.kCGWindowIsOnscreen })));
"#;

/// How long the window list may take; it answers in about a tenth of a second.
const LIST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Windows narrower or shorter than this are the helper strips and offscreen buffers apps keep
/// around (a terminal alone leaves ten), not something a person means by "that window".
const MIN_SIDE: f64 = 100.0;

/// How many window names an unmatched search reports back.
const SHOWN_ON_MISS: usize = 12;

/// One entry of the window list.
#[derive(Debug, Clone, Deserialize, PartialEq)]
struct WindowInfo {
    id: u64,
    owner: String,
    name: String,
    layer: i64,
    w: f64,
    h: f64,
    on: bool,
}

impl WindowInfo {
    /// A normal, sizeable application window: layer 0 is where apps draw, and the menu bar, the
    /// Dock and overlays sit above it.
    fn is_usable(&self) -> bool {
        self.layer == 0 && self.w >= MIN_SIDE && self.h >= MIN_SIDE
    }

    fn label(&self) -> String {
        match self.name.is_empty() {
            true => self.owner.clone(),
            false => format!("{}: {}", self.owner, self.name),
        }
    }
}

/// Captures a window to `out`, wherever its desktop is. `title` matches a window's title, or
/// failing that its app's name; `None` takes the frontmost window on screen.
pub(super) async fn capture_window(title: Option<&str>, out: &str) -> Result<()> {
    let id = pick_window(&list_windows().await?, title)?;
    let args = ["-x", "-o", "-l", &id.to_string(), out].map(String::from);
    super::run("screencapture", &args).await
}

async fn list_windows() -> Result<Vec<WindowInfo>> {
    let child = Command::new("osascript")
        .args(["-l", "JavaScript", "-e", LIST_WINDOWS_SCRIPT])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .output();
    let output = tokio::time::timeout(LIST_DEADLINE, child)
        .await
        .map_err(|_| {
            anyhow!("`osascript` did not list windows within {}s", LIST_DEADLINE.as_secs())
        })?
        .context("failed to run `osascript` to list windows")?;
    if !output.status.success() {
        bail!("listing windows failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    serde_json::from_slice(&output.stdout).context("could not parse the window list")
}

/// Chooses the window to capture. A title match beats an app-name match; among matches an
/// on-screen window beats a hidden one and a bigger beats a smaller.
fn pick_window(windows: &[WindowInfo], title: Option<&str>) -> Result<u64> {
    let usable = || windows.iter().filter(|window| window.is_usable());
    let Some(needle) = title else {
        let frontmost = usable().find(|window| window.on);
        return frontmost.map(|window| window.id).ok_or_else(|| anyhow!("no window is on screen"));
    };
    let titled = best(usable().filter(|window| window.name.contains(needle)));
    let by_app = || best(usable().filter(|window| window.owner.contains(needle)));
    titled.or_else(by_app).ok_or_else(|| no_match(windows, needle))
}

fn best<'a>(matches: impl Iterator<Item = &'a WindowInfo>) -> Option<u64> {
    matches.max_by_key(|window| (window.on, (window.w * window.h) as u64)).map(|window| window.id)
}

fn no_match(windows: &[WindowInfo], needle: &str) -> anyhow::Error {
    // Titled windows are what a search can name; with titles hidden, the apps are all there is
    let usable = || windows.iter().filter(|w| w.is_usable());
    let mut shown: Vec<&WindowInfo> = usable().filter(|w| !w.name.is_empty()).collect();
    if shown.is_empty() {
        shown = usable().collect();
    }
    let labels: Vec<String> =
        shown.into_iter().take(SHOWN_ON_MISS).map(WindowInfo::label).collect();
    anyhow!(
        "no window whose title or app contains '{needle}' (case-sensitive). Windows seen: {}. \
         Titles of other apps' windows need Screen Recording permission for the terminal.",
        labels.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64, owner: &str, name: &str, size: (f64, f64), on: bool) -> WindowInfo {
        let (w, h) = size;
        WindowInfo { id, owner: owner.into(), name: name.into(), layer: 0, w, h, on }
    }

    /// The Ghostty entries from a real listing: one real window among its helper strips.
    fn ghostty_with_helpers(real_is_on_screen: bool) -> Vec<WindowInfo> {
        vec![
            window(102536, "Ghostty", "", (500.0, 500.0), false),
            window(102535, "Ghostty", "luxe", (2539.0, 1391.0), real_is_on_screen),
            window(102534, "Ghostty", "", (3840.0, 30.0), false),
            window(102530, "Ghostty", "", (1728.0, 33.0), false),
        ]
    }

    /// Copied from `osascript -l JavaScript` output on macOS, so a renamed key fails here.
    #[test]
    fn the_window_list_parses_as_the_script_prints_it() {
        let line = r#"[{"id":102535,"owner":"Ghostty","name":"luxe","layer":0,"w":2539,"h":1391,"on":true}]"#;
        let parsed: Vec<WindowInfo> = serde_json::from_str(line).expect("parses");
        assert_eq!(parsed, vec![window(102535, "Ghostty", "luxe", (2539.0, 1391.0), true)]);
    }

    /// Catches a title match falling through to the app name, which would also hit the helpers.
    #[test]
    fn a_title_picks_that_window() {
        assert_eq!(pick_window(&ghostty_with_helpers(true), Some("lux")).expect("found"), 102535);
    }

    /// A window on another desktop is not on screen; it must still be found.
    #[test]
    fn a_window_on_another_desktop_is_found() {
        assert_eq!(pick_window(&ghostty_with_helpers(false), Some("luxe")).expect("found"), 102535);
    }

    /// Two windows share a title: the one in front of the reader wins over a bigger one hidden away.
    #[test]
    fn an_on_screen_window_beats_a_bigger_hidden_one() {
        let windows = vec![
            window(1, "Ghostty", "luxe", (3000.0, 2000.0), false),
            window(2, "Ghostty", "luxe", (1000.0, 700.0), true),
        ];
        assert_eq!(pick_window(&windows, Some("luxe")).expect("found"), 2);
    }

    /// Without a title permission every name is empty; the app name still finds the big window,
    /// not the 500 by 500 offscreen buffer or the thin strips.
    #[test]
    fn an_app_name_picks_its_biggest_window_when_titles_are_hidden() {
        let mut windows = ghostty_with_helpers(false);
        windows[1].name.clear();
        assert_eq!(pick_window(&windows, Some("Ghostty")).expect("found"), 102535);
    }

    /// A title beats an app match even when the app's other window is on screen and bigger.
    #[test]
    fn a_title_match_beats_an_app_match() {
        let windows = vec![
            window(1, "Safari", "docs", (1200.0, 800.0), true),
            window(2, "Ghostty", "docs", (900.0, 600.0), false),
            window(3, "docs", "", (2000.0, 1500.0), true),
        ];
        assert_eq!(pick_window(&windows, Some("docs")).expect("found"), 1);
    }

    #[test]
    fn matching_is_case_sensitive_and_a_miss_lists_what_exists() {
        let message = pick_window(&ghostty_with_helpers(true), Some("LUXE"))
            .expect_err("no match")
            .to_string();
        assert!(message.contains("'LUXE'"), "{message}");
        assert!(message.contains("Ghostty: luxe"), "{message}");
    }

    /// No title means the frontmost window on screen: the first usable one, in front-to-back order.
    #[test]
    fn no_title_takes_the_frontmost_window_on_screen() {
        let mut menu_bar = window(9, "Window Server", "Menubar", (3840.0, 30.0), true);
        menu_bar.layer = 24;
        let windows = vec![
            menu_bar,
            window(8, "Ghostty", "", (3840.0, 30.0), true),
            window(7, "Safari", "docs", (1200.0, 800.0), false),
            window(6, "Ghostty", "luxe", (2539.0, 1391.0), true),
            window(5, "Notes", "todo", (800.0, 600.0), true),
        ];
        assert_eq!(pick_window(&windows, None).expect("found"), 6);
        assert!(pick_window(&windows[..3], None).is_err(), "nothing usable is on screen");
    }
}
