//! Native screen capture — OS/display-server detection and per-platform capture commands.
//!
//! Shells out to the platform's native tool rather than linking a capture crate, keeping the
//! dependency surface tiny and the gate clean: grim (+ hyprctl) on Wayland, maim/scrot/ImageMagick
//! on X11, `screencapture` on macOS, and PowerShell + System.Drawing on Windows. Each writes a PNG
//! that the tool then reads and returns inline.

use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DisplayBackend {
    /// Wayland compositor that is Hyprland (window geometry via `hyprctl`).
    WaylandHyprland,
    /// Any other wlroots-ish Wayland compositor (grim works; per-window needs help).
    Wayland,
    /// X11 (maim/scrot/import).
    X11,
    /// macOS (`screencapture`).
    MacOS,
    /// Windows (PowerShell + System.Drawing).
    Windows,
    /// Nothing usable detected.
    Unknown,
}

pub(super) enum CaptureTarget {
    /// The whole screen, optionally a single named output/monitor.
    Fullscreen { output: Option<String> },
    /// An explicit region, given as `"X,Y WxH"`.
    Region { geometry: String },
    /// A window matched by title substring, or the active window when `None`.
    Window { title: Option<String> },
}

/// Detects the display backend from the environment and target OS.
pub(super) fn detect_backend() -> DisplayBackend {
    if cfg!(target_os = "macos") {
        return DisplayBackend::MacOS;
    }
    if cfg!(target_os = "windows") {
        return DisplayBackend::Windows;
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some();
    if wayland {
        if hyprland {
            DisplayBackend::WaylandHyprland
        } else {
            DisplayBackend::Wayland
        }
    } else if std::env::var_os("DISPLAY").is_some() {
        DisplayBackend::X11
    } else {
        DisplayBackend::Unknown
    }
}

/// Human-readable name for error messages.
pub(super) fn backend_label(backend: DisplayBackend) -> &'static str {
    match backend {
        DisplayBackend::WaylandHyprland => "Wayland (Hyprland)",
        DisplayBackend::Wayland => "Wayland",
        DisplayBackend::X11 => "X11",
        DisplayBackend::MacOS => "macOS",
        DisplayBackend::Windows => "Windows",
        DisplayBackend::Unknown => "unknown display server",
    }
}

/// A parsed pixel rectangle.
struct Rect {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

/// Parses a `"X,Y WxH"` geometry string (the same shape grim accepts).
fn parse_geometry(geometry: &str) -> Result<Rect> {
    let (pos, size) = geometry
        .split_once(' ')
        .ok_or_else(|| anyhow!("geometry must look like 'X,Y WxH', got '{geometry}'"))?;
    let (x, y) = pos.split_once(',').ok_or_else(|| anyhow!("bad position in '{geometry}'"))?;
    let (w, h) = size
        .split_once(['x', 'X'])
        .ok_or_else(|| anyhow!("bad size in '{geometry}' (want WxH)"))?;
    let parse = |v: &str, what: &str| {
        v.trim().parse::<i64>().with_context(|| format!("invalid {what} in geometry '{geometry}'"))
    };
    Ok(Rect { x: parse(x, "x")?, y: parse(y, "y")?, w: parse(w, "width")?, h: parse(h, "height")? })
}

/// Whether this backend has a program that can actually take the picture.
///
/// A display is not a capture tool. `detect_backend` answers on the strength of `$WAYLAND_DISPLAY`
/// or `$DISPLAY` alone, and neither Wayland nor X11 ships a screenshot binary — so a box with a
/// desktop and no `grim` must not advertise `screenshot` to the model and then fail every desktop
/// capture with "install `grim`". macOS and Windows carry theirs in the OS.
pub(super) fn desktop_capture_available(backend: DisplayBackend) -> bool {
    match backend {
        DisplayBackend::WaylandHyprland | DisplayBackend::Wayland => which("grim"),
        DisplayBackend::X11 => which("maim") || which("scrot") || which("import"),
        // `screencapture` and PowerShell ship with the OS.
        DisplayBackend::MacOS | DisplayBackend::Windows => true,
        DisplayBackend::Unknown => false,
    }
}

/// Captures `target` to the PNG file at `out`, routing to the backend's native tool.
pub(super) async fn capture(
    backend: DisplayBackend,
    target: &CaptureTarget,
    out: &Path,
) -> Result<()> {
    match backend {
        DisplayBackend::WaylandHyprland | DisplayBackend::Wayland => {
            capture_wayland(backend, target, out).await
        }
        DisplayBackend::X11 => capture_x11(target, out).await,
        DisplayBackend::MacOS => capture_macos(target, out).await,
        DisplayBackend::Windows => capture_windows(target, out).await,
        DisplayBackend::Unknown => bail!(
            "no supported display server detected (neither $WAYLAND_DISPLAY nor $DISPLAY is set)"
        ),
    }
}

// === Wayland (grim + hyprctl) ===

async fn capture_wayland(
    backend: DisplayBackend,
    target: &CaptureTarget,
    out: &Path,
) -> Result<()> {
    require("grim", "install `grim` (and `slurp` for interactive regions)")?;
    let out = path_str(out)?;
    match target {
        CaptureTarget::Fullscreen { output } => {
            let mut args: Vec<String> = Vec::new();
            if let Some(name) = output {
                args.push("-o".into());
                args.push(name.clone());
            }
            args.push(out);
            run("grim", &args).await
        }
        CaptureTarget::Region { geometry } => {
            // grim consumes the "X,Y WxH" shape directly; validate it first for a clear error.
            parse_geometry(geometry)?;
            run("grim", &["-g".into(), geometry.clone(), out]).await
        }
        CaptureTarget::Window { title } => {
            if backend != DisplayBackend::WaylandHyprland {
                bail!(
                    "per-window capture on Wayland is only supported under Hyprland; \
                     use mode=region with an explicit geometry instead"
                );
            }
            let geometry = hyprland_window_geometry(title.as_deref()).await?;
            run("grim", &["-g".into(), geometry, out]).await
        }
    }
}

/// Reads a window's `X,Y WxH` geometry from `hyprctl` (active window, or the first title match).
async fn hyprland_window_geometry(title: Option<&str>) -> Result<String> {
    let query = if title.is_some() { "clients" } else { "activewindow" };
    let output = Command::new("hyprctl")
        .args(["-j", query])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("failed to run `hyprctl` for window geometry")?;
    if !output.status.success() {
        bail!("`hyprctl -j {query}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("could not parse hyprctl JSON")?;
    let window = match title {
        Some(needle) => json
            .as_array()
            .and_then(|clients| {
                clients
                    .iter()
                    .find(|client| client["title"].as_str().is_some_and(|t| t.contains(needle)))
            })
            .ok_or_else(|| anyhow!("no Hyprland window whose title contains '{needle}'"))?,
        None => &json,
    };
    let at = &window["at"];
    let size = &window["size"];
    let field = |value: &serde_json::Value, index: usize| -> Result<i64> {
        value[index].as_i64().ok_or_else(|| anyhow!("hyprctl returned no window geometry"))
    };
    Ok(format!("{},{} {}x{}", field(at, 0)?, field(at, 1)?, field(size, 0)?, field(size, 1)?))
}

// === X11 (maim / scrot / ImageMagick import) ===

async fn capture_x11(target: &CaptureTarget, out: &Path) -> Result<()> {
    let out = path_str(out)?;
    match target {
        CaptureTarget::Fullscreen { .. } => {
            try_candidates(
                &[
                    ("maim", vec![out.clone()]),
                    ("scrot", vec!["-o".into(), out.clone()]),
                    ("import", vec!["-window".into(), "root".into(), out.clone()]),
                ],
                "install one of: maim, scrot, or ImageMagick (`import`)",
            )
            .await
        }
        CaptureTarget::Region { geometry } => {
            let r = parse_geometry(geometry)?;
            let maim_geo = format!("{}x{}+{}+{}", r.w, r.h, r.x, r.y);
            let scrot_geo = format!("{},{},{},{}", r.x, r.y, r.w, r.h);
            let crop = format!("{}x{}+{}+{}", r.w, r.h, r.x, r.y);
            try_candidates(
                &[
                    ("maim", vec!["-g".into(), maim_geo, out.clone()]),
                    ("scrot", vec!["-a".into(), scrot_geo, "-o".into(), out.clone()]),
                    (
                        "import",
                        vec!["-window".into(), "root".into(), "-crop".into(), crop, out.clone()],
                    ),
                ],
                "install one of: maim, scrot, or ImageMagick (`import`)",
            )
            .await
        }
        CaptureTarget::Window { title } => {
            let id = x11_window_id(title.as_deref()).await?;
            try_candidates(
                &[
                    ("maim", vec!["-i".into(), id.clone(), out.clone()]),
                    ("import", vec!["-window".into(), id, out.clone()]),
                ],
                "install `maim` (with `xdotool`) or ImageMagick (`import`)",
            )
            .await
        }
    }
}

/// Resolves an X11 window id (hex) via `xdotool`, by title or the active window.
async fn x11_window_id(title: Option<&str>) -> Result<String> {
    require("xdotool", "install `xdotool` for per-window X11 capture")?;
    let args: Vec<&str> = match title {
        Some(needle) => vec!["search", "--name", "--limit", "1", needle],
        None => vec!["getactivewindow"],
    };
    let output = Command::new("xdotool")
        .args(&args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("failed to run xdotool")?;
    if !output.status.success() {
        bail!("xdotool could not resolve the window (title match failed?)");
    }
    let id =
        String::from_utf8_lossy(&output.stdout).trim().lines().next().unwrap_or("").to_string();
    if id.is_empty() {
        bail!("no matching X11 window found");
    }
    Ok(id)
}

// === macOS (screencapture) ===

async fn capture_macos(target: &CaptureTarget, out: &Path) -> Result<()> {
    let out = path_str(out)?;
    match target {
        CaptureTarget::Fullscreen { .. } => run("screencapture", &["-x".into(), out]).await,
        CaptureTarget::Region { geometry } => {
            let r = parse_geometry(geometry)?;
            let rect = format!("{},{},{},{}", r.x, r.y, r.w, r.h);
            run("screencapture", &["-x".into(), "-R".into(), rect, out]).await
        }
        CaptureTarget::Window { .. } => bail!(
            "per-window capture on macOS needs a window id; use mode=fullscreen or mode=region"
        ),
    }
}

// === Windows (PowerShell + System.Drawing) ===

async fn capture_windows(target: &CaptureTarget, out: &Path) -> Result<()> {
    let out = path_str(out)?;
    let script = match target {
        CaptureTarget::Fullscreen { .. } => windows_capture_script("virtual", &out),
        CaptureTarget::Region { geometry } => {
            let r = parse_geometry(geometry)?;
            windows_capture_script(&format!("{},{},{},{}", r.x, r.y, r.w, r.h), &out)
        }
        CaptureTarget::Window { .. } => bail!(
            "per-window capture on Windows is not supported yet; use mode=fullscreen or mode=region"
        ),
    };
    run("powershell", &["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script])
        .await
}

/// Builds a PowerShell one-liner that captures the virtual screen (or `x,y,w,h`) to `out`.
fn windows_capture_script(rect: &str, out: &str) -> String {
    let bounds = if rect == "virtual" {
        "$r=[System.Windows.Forms.SystemInformation]::VirtualScreen".to_string()
    } else {
        let parts: Vec<&str> = rect.split(',').collect();
        format!(
            "$r=New-Object System.Drawing.Rectangle({},{},{},{})",
            parts.first().unwrap_or(&"0"),
            parts.get(1).unwrap_or(&"0"),
            parts.get(2).unwrap_or(&"0"),
            parts.get(3).unwrap_or(&"0")
        )
    };
    format!(
        "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; {bounds}; \
         $bmp=New-Object System.Drawing.Bitmap($r.Width,$r.Height); \
         $g=[System.Drawing.Graphics]::FromImage($bmp); \
         $g.CopyFromScreen($r.Left,$r.Top,0,0,$bmp.Size); \
         $bmp.Save('{out}',[System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()"
    )
}

// === Shared helpers ===

/// Runs `program` with `args`, surfacing stderr on failure and a clear hint when it's missing.
///
/// `kill_on_drop` throughout: a stopped tool call unwinds by dropping its future, and a capture
/// helper waiting on a compositor that never answers has to go with it rather than linger. Stdin is
/// closed for the same reason it is in `bash` — a helper that reads it would eat the user's
/// keystrokes out from under the TUI.
/// How long a native capture may take. Each is sub-second on a screen that is drawing; one that is
/// locked or asleep draws no frames, and `grim` waits for one indefinitely, so a capture that has not
/// finished by now never will.
const CAPTURE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

async fn run(program: &str, args: &[String]) -> Result<()> {
    run_within(program, args, CAPTURE_DEADLINE).await
}

async fn run_within(program: &str, args: &[String], deadline: std::time::Duration) -> Result<()> {
    let child = Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .output();
    // Dropping the future on timeout kills the capture, `kill_on_drop` being what makes it so
    let Ok(output) = tokio::time::timeout(deadline, child).await else {
        bail!(
            "`{program}` produced no image within {}s. A screen that is locked or asleep draws no \
             frames to capture; wake it and try again.",
            deadline.as_secs()
        );
    };
    let output = output.map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!("`{program}` is not installed or not on PATH")
        } else {
            anyhow!("failed to spawn `{program}`: {err}")
        }
    })?;
    if !output.status.success() {
        bail!("`{program}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// Tries each `(program, args)` candidate in order, returning on the first success. Reports the
/// last error (and `hint`) when they all fail or none are installed.
async fn try_candidates(candidates: &[(&str, Vec<String>)], hint: &str) -> Result<()> {
    let mut last_error = None;
    for (program, args) in candidates {
        if !which(program) {
            continue;
        }
        match run(program, args).await {
            Ok(()) => return Ok(()),
            Err(err) => last_error = Some(err),
        }
    }
    match last_error {
        Some(err) => Err(err),
        None => bail!("no capture tool available — {hint}"),
    }
}

/// Errors with an install hint when `program` isn't on PATH.
fn require(program: &str, hint: &str) -> Result<()> {
    if which(program) {
        Ok(())
    } else {
        bail!("`{program}` is not installed or not on PATH — {hint}")
    }
}

/// True when `program` can be run.
///
/// Defers to [`luxe_core::program`], which is the one place that knows what "runnable" means on each
/// platform — the executable bit on unix, a `PATHEXT` suffix on Windows — and which also looks in the
/// install directories a package manager uses but a `PATH` often misses.
pub(super) fn which(program: &str) -> bool {
    luxe_core::program::is_available(program)
}

fn path_str(path: &Path) -> Result<String> {
    path.to_str().map(str::to_string).ok_or_else(|| anyhow!("output path is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_geometry() {
        let r = parse_geometry("10,20 300x400").unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (10, 20, 300, 400));
    }

    #[test]
    fn rejects_bad_geometry() {
        assert!(parse_geometry("nonsense").is_err());
        assert!(parse_geometry("10,20").is_err()); // missing size
        assert!(parse_geometry("a,b cxd").is_err()); // non-numeric
    }

    #[test]
    fn which_resolves_installed_binaries() {
        if cfg!(unix) {
            assert!(which("sh"), "sh should resolve on unix");
        }
        assert!(!which("definitely-not-a-real-binary-xyzzy"));
    }

    #[test]
    fn backend_label_is_stable() {
        assert_eq!(backend_label(DisplayBackend::X11), "X11");
        assert_eq!(backend_label(DisplayBackend::WaylandHyprland), "Wayland (Hyprland)");
    }

    /// A capture that never finishes — a screen asleep, for `grim` — is cut off and says why,
    /// rather than holding the tool until its network-shaped deadline.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_capture_that_never_finishes_is_cut_off_with_a_reason() {
        let started = std::time::Instant::now();
        let error = run_within("sleep", &["30".into()], std::time::Duration::from_millis(200))
            .await
            .expect_err("cut off");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(error.to_string().contains("locked or asleep"), "{error}");
    }
}
