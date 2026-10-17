//! `screenshot` tool — cross-platform screen/window/URL capture for visual verification.
//!
//! Captures the screen, a region, a window, or a rendered URL to a PNG, then returns it inline
//! (so vision-capable models can see it) alongside the saved path. Uses the platform's native
//! capture tool (see `capture`) — no heavyweight capture crate — so the dependency surface and
//! the supply-chain gate stay clean.

mod browser;
mod capture;

use async_trait::async_trait;
use base64::Engine;
use luxe_core::provider::ToolSchema;
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use capture::CaptureTarget;

/// The `screenshot` tool.
#[derive(Default)]
pub struct ScreenshotTool;

impl ScreenshotTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    /// `mode=url` hands a URL to a headless browser, and a browser told to load a host that never
    /// answers waits forever — `--virtual-time-budget` bounds the page's *clock*, not its network.
    /// The native capture backends are sub-second, so one network-shaped bound covers every mode.
    fn timeout(&self) -> Option<std::time::Duration> {
        Some(luxe_core::tool::NETWORK_TOOL_TIMEOUT)
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "screenshot".to_string(),
            description: "Capture a screenshot and return it inline (for visual UI verification) \
                          plus save it to .luxe/screenshots/. Cross-platform via the OS's native \
                          tool (Wayland/grim, X11/maim|scrot, macOS/screencapture, Windows/\
                          PowerShell); mode=url renders a page in headless Chrome/Chromium/Edge — \
                          the best way to check a web UI or dev-server output. Params by mode: \
                          fullscreen{output?}; region{region:\"X,Y WxH\"}; window{window?:title \
                          substring}; url{url, width?, height?, waitMs?}. `note` is echoed back.\n\
                          mode=url reaches public sites and this machine — localhost, 127.0.0.1 and \
                          ::1 all work with no setup, so capturing your dev server just works. It \
                          refuses private-network hosts and cloud-metadata endpoints (reaching \
                          those is not this tool's job), and refuses file:// (use `read` for file \
                          contents). Only the requested host resolves: redirects, frames and \
                          assets from any other host fail to load.\n\
                          Prefer mode=url for anything web: it needs no desktop, no approval, and \
                          shows only what the page drew. The other three photograph the reader's \
                          actual screen, so they prompt for approval and are refused outright when \
                          no one is at the keyboard (headless, the loop, a review agent)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["fullscreen", "region", "window", "url"],
                        "description": "What to capture (default: url when a `url` is given, else fullscreen)"
                    },
                    "region": {
                        "type": "string",
                        "description": "region mode: capture rectangle as 'X,Y WxH', e.g. '0,0 1280x800'"
                    },
                    "window": {
                        "type": "string",
                        "description": "window mode: case-sensitive title substring; omit for the active window (Hyprland/X11 only)"
                    },
                    "url": {
                        "type": "string",
                        "description": "url mode: the http(s):// page to render and capture. Public or local (localhost/127.0.0.1/::1); private-network and cloud-metadata hosts are refused, and so is file://"
                    },
                    "output": {
                        "type": "string",
                        "description": "fullscreen mode: a specific monitor/output name (Wayland); omit for all"
                    },
                    "width": {
                        "type": "integer",
                        "description": "url mode: viewport width in px (default 1280)"
                    },
                    "height": {
                        "type": "integer",
                        "description": "url mode: viewport height in px (default 800)"
                    },
                    "waitMs": {
                        "type": "integer",
                        "description": "url mode: ms to let the page render before capture (default 1500)"
                    },
                    "note": {
                        "type": "string",
                        "description": "optional label echoed back with the result (what you're inspecting)"
                    }
                }
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        match self.capture(&args, cx).await {
            Ok(result) => result,
            Err(err) => ToolResultData::error(format!("Screenshot failed: {err:#}")),
        }
    }
}

impl ScreenshotTool {
    async fn capture(
        &self,
        args: &serde_json::Value,
        cx: &ToolContext,
    ) -> anyhow::Result<ToolResultData> {
        let mode = luxe_core::permission::screenshot_mode(args);
        let note = args.get("note").and_then(|v| v.as_str());

        // Photographing the desktop needs somebody at the keyboard. The permission engine makes it
        // an `ask`, and an `ask` auto-allows where nothing can prompt — which is precisely the run
        // that must not have it: a headless session or a loop iteration would capture a screen no
        // one is watching and send it to a provider. Same rule, and the same reasoning, as a
        // language server out of the project's own `node_modules`.
        if captures_the_desktop(mode) && cx.question_broker.is_none() {
            anyhow::bail!(
                "mode={mode} photographs the whole screen, so it runs only with someone at the \
                 keyboard to approve it — this session has no way to ask. Use mode=url to capture \
                 a page (a dev server, a local build), which needs no desktop and no approval."
            );
        }

        let out_path = new_output_path(&cx.cwd)?;

        let backend_label = if mode == "url" {
            "headless browser".to_string()
        } else {
            capture::backend_label(capture::detect_backend()).to_string()
        };

        match mode {
            "url" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("mode=url requires a `url`"))?;
                let width = args.get("width").and_then(|v| v.as_u64()).unwrap_or(1280) as u32;
                let height = args.get("height").and_then(|v| v.as_u64()).unwrap_or(800) as u32;
                let wait_ms = luxe_core::tool::arg(args, &["waitMs", "wait_ms"])
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1500) as u32;
                browser::capture_url(url, (width, height), wait_ms, &out_path, &cx.cwd).await?;
            }
            "fullscreen" | "region" | "window" => {
                let target = build_target(mode, args)?;
                let backend = capture::detect_backend();
                capture::capture(backend, &target, &out_path).await?;
            }
            other => {
                anyhow::bail!("unknown mode '{other}' (use fullscreen | region | window | url)")
            }
        }

        encode_result(&out_path, mode, &backend_label, note)
    }
}

/// Builds the capture target for a screen-capture mode from the tool arguments.
fn build_target(mode: &str, args: &serde_json::Value) -> anyhow::Result<CaptureTarget> {
    match mode {
        "fullscreen" => Ok(CaptureTarget::Fullscreen {
            output: args.get("output").and_then(|v| v.as_str()).map(str::to_string),
        }),
        "region" => {
            let geometry = args
                .get("region")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("mode=region requires a `region` (\"X,Y WxH\")"))?;
            Ok(CaptureTarget::Region { geometry: geometry.to_string() })
        }
        "window" => Ok(CaptureTarget::Window {
            title: args.get("window").and_then(|v| v.as_str()).map(str::to_string),
        }),
        other => anyhow::bail!("unsupported capture mode '{other}'"),
    }
}

/// Reads the captured PNG and packages it as an inline image plus a text summary.
fn encode_result(
    out_path: &std::path::Path,
    mode: &str,
    backend_label: &str,
    note: Option<&str>,
) -> anyhow::Result<ToolResultData> {
    let bytes = std::fs::read(out_path)
        .map_err(|err| anyhow::anyhow!("captured file could not be read: {err}"))?;
    if bytes.is_empty() {
        anyhow::bail!("capture produced an empty file");
    }
    // Keep the saved file full-resolution, but downscale the INLINE copy so a big multi-monitor
    // capture doesn't blow up token cost.
    let (inline_bytes, dimensions) = downscale_for_inline(&bytes);
    let data = base64::engine::general_purpose::STANDARD.encode(&inline_bytes);
    let mut summary = format!(
        "Captured {mode} via {backend_label} ({dimensions}, {size} KB on disk). Saved: {path}",
        size = bytes.len() / 1024,
        path = luxe_core::paths::display(out_path)
    );
    if let Some(note) = note {
        summary.push_str(&format!("\nNote: {note}"));
    }
    Ok(ToolResultData {
        content: vec![
            ContentBlock::Text(summary),
            // The saved path travels with the pixels: a terminal that cannot draw the capture
            // offers to open it instead, and a model looking at three of them can tell them apart.
            ContentBlock::Image {
                data,
                mime: "image/png".to_string(),
                name: Some(luxe_core::paths::display(out_path)),
            },
        ],
        is_error: false,
    })
}

/// Longest edge (px) for the inline image sent to the model — keeps token cost sane while staying
/// legible (Anthropic's recommended max long edge).
const MAX_INLINE_DIM: u32 = 1568;

/// Returns the inline PNG bytes (downscaled if either side exceeds [`MAX_INLINE_DIM`]) plus a
/// human-readable dimensions string. On any decode/encode failure, falls back to the original
/// bytes so a capture is never lost to a resize hiccup.
fn downscale_for_inline(bytes: &[u8]) -> (Vec<u8>, String) {
    let Ok(img) = image::load_from_memory(bytes) else {
        return (bytes.to_vec(), "unknown size".to_string());
    };
    let (w, h) = (img.width(), img.height());
    if w <= MAX_INLINE_DIM && h <= MAX_INLINE_DIM {
        return (bytes.to_vec(), format!("{w}x{h}"));
    }
    let resized = img.resize(MAX_INLINE_DIM, MAX_INLINE_DIM, image::imageops::FilterType::Triangle);
    let (nw, nh) = (resized.width(), resized.height());
    let mut out = std::io::Cursor::new(Vec::new());
    match resized.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => (out.into_inner(), format!("{w}x{h} → {nw}x{nh} inline")),
        Err(_) => (bytes.to_vec(), format!("{w}x{h}")),
    }
}

/// Allocates a fresh `<cwd>/.luxe/screenshots/shot-<millis>.png`, creating the directory.
fn new_output_path(cwd: &std::path::Path) -> anyhow::Result<PathBuf> {
    let dir = cwd.join(".luxe").join("screenshots");
    std::fs::create_dir_all(&dir)
        .map_err(|err| anyhow::anyhow!("could not create {}: {err}", dir.display()))?;
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    Ok(dir.join(format!("shot-{millis}.png")))
}

/// Whether a screenshot could actually be taken here: a program that can photograph the desktop,
/// or a headless browser for URL captures.
///
/// On a CI runner or an SSH session with neither, every call this tool could make ends in an
/// install hint — so it is not worth a schema in every request. The desktop half asks for the
/// *program*, not merely a display: see `capture::desktop_capture_available`.
pub fn is_available() -> bool {
    capture::desktop_capture_available(capture::detect_backend()) || browser::is_available()
}

/// Modes that photograph whatever is on the reader's screen, rather than a page Luxe rendered.
///
/// The distinction the permission engine and the unattended guard are both drawn on: `url` shows a
/// model a document it asked Luxe to load, and these show it the desktop — a password manager, a
/// private message, a colleague on a shared display.
fn captures_the_desktop(mode: &str) -> bool {
    matches!(mode, "fullscreen" | "region" | "window")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn schema_exposes_four_modes() {
        let tool = ScreenshotTool::new();
        assert_eq!(tool.name(), "screenshot");
        let schema = tool.schema();
        let modes = schema.parameters["properties"]["mode"]["enum"].as_array().unwrap();
        assert_eq!(modes.len(), 4, "fullscreen/region/window/url");
    }

    #[test]
    fn region_target_requires_geometry() {
        assert!(build_target("region", &serde_json::json!({})).is_err());
        assert!(build_target("region", &serde_json::json!({ "region": "0,0 8x6" })).is_ok());
    }

    #[test]
    fn small_images_pass_through_undownscaled() {
        let png = tiny_png(64, 48);
        let (bytes, dims) = downscale_for_inline(&png);
        assert_eq!(bytes, png);
        assert_eq!(dims, "64x48");
    }

    #[test]
    fn large_images_are_downscaled_for_inline() {
        let png = tiny_png(3000, 1000);
        let (bytes, dims) = downscale_for_inline(&png);
        assert!(dims.contains('→'), "dims note should show the resize: {dims}");
        let img = image::load_from_memory(&bytes).unwrap();
        assert!(img.width() <= MAX_INLINE_DIM && img.height() <= MAX_INLINE_DIM);
    }

    #[test]
    fn output_path_creates_the_screenshots_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let path = new_output_path(tmp.path()).unwrap();
        assert!(path.parent().unwrap().is_dir());
        assert!(path.to_string_lossy().ends_with(".png"));
    }

    /// Availability is a real capability question, and the answer is a *program*: a display is not
    /// a screenshot tool. Wayland and X11 ship neither, so a desktop without `grim` must not
    /// advertise this tool and then fail every capture with an install hint.
    #[test]
    fn availability_tracks_a_capture_program_or_a_browser() {
        let can_capture = capture::desktop_capture_available(capture::detect_backend());
        assert_eq!(is_available(), can_capture || browser::is_available());
    }

    /// A display with nothing installed to photograph it is not availability.
    #[test]
    fn a_bare_display_server_provides_no_capture() {
        use capture::{desktop_capture_available, DisplayBackend};
        // The two that carry their capture tool in the OS, and the one that carries nothing.
        assert!(desktop_capture_available(DisplayBackend::MacOS));
        assert!(desktop_capture_available(DisplayBackend::Windows));
        assert!(!desktop_capture_available(DisplayBackend::Unknown));
    }

    /// The desktop modes are the ones that need a human; `url` renders a page and needs nobody.
    #[test]
    fn only_the_desktop_modes_photograph_the_reader() {
        for mode in ["fullscreen", "region", "window"] {
            assert!(captures_the_desktop(mode), "{mode}");
        }
        assert!(!captures_the_desktop("url"));
    }

    /// An unattended run refuses rather than capturing: an `ask` auto-allows where nothing can
    /// prompt, which is exactly the session that must not photograph a screen nobody is watching.
    /// The refusal names `mode=url`, because that is the one that still works there.
    #[tokio::test]
    async fn an_unattended_run_refuses_to_photograph_the_screen() {
        let tmp = tempfile::tempdir().unwrap();
        let (event_tx, _rx) = tokio::sync::mpsc::channel(8);
        let (_cancel, cancel_rx) = tokio::sync::watch::channel(false);
        let cx = ToolContext {
            cwd: tmp.path().to_path_buf(),
            session_id: "s".into(),
            cancelled: std::sync::Arc::new(cancel_rx),
            event_tx,
            hook_ctx: None,
            // The signal itself: `None` is "no front-end can ask a human anything".
            question_broker: None,
        };

        let result =
            ScreenshotTool::new().run(serde_json::json!({"mode": "fullscreen"}), &cx).await;

        assert!(result.is_error, "an unattended desktop capture must not proceed");
        let ContentBlock::Text(message) = &result.content[0] else { panic!("a text refusal") };
        assert!(message.contains("someone at the keyboard"), "says why: {message}");
        assert!(message.contains("mode=url"), "and what does work instead: {message}");
    }
}
