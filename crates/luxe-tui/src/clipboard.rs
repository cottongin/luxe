//! Clipboard abstraction.
//!
//! Provides `ClipboardSink` for writing text and `ClipboardReader` for reading text/images from the
//! system clipboard. Both are available behind the `clipboard` feature flag; without it, or with no
//! display to talk to, the sink is [`UnavailableClipboard`], whose refusal is what sends a copy on
//! to the terminal (OSC 52).
//!
//! # Wayland
//!
//! Both directions prefer the `wl-clipboard` tools when `WAYLAND_DISPLAY` is set, because `arboard`
//! is built here without `wayland-data-control` — it would otherwise talk to XWayland's X11
//! selection, which a Wayland-native paste (`wl-paste`, a compositor keybinding) never sees. The
//! X11 path also only holds a selection for as long as its `arboard::Clipboard` lives, which is why
//! [`create_clipboard`] hands back one long-lived instance for the whole process instead of building
//! one per copy.

/// Trait for writing text to the system clipboard.
#[async_trait::async_trait]
pub trait ClipboardSink: Send + Sync {
    /// Write text to the clipboard. Returns an error if no backend accepted it.
    async fn write_text(&self, text: &str) -> anyhow::Result<()>;
}

/// An image read from the system clipboard.
#[derive(Debug, Clone)]
pub struct ClipboardImage {
    /// Raw RGBA pixels.
    pub pixels: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Trait for reading from the system clipboard.
#[async_trait::async_trait]
pub trait ClipboardReader: Send + Sync {
    /// Read text from the clipboard. Returns None if the clipboard is empty or
    /// doesn't contain text.
    async fn read_text(&self) -> anyhow::Result<Option<String>>;

    /// Read an image from the clipboard. Returns None if the clipboard doesn't
    /// contain an image.
    async fn read_image(&self) -> anyhow::Result<Option<ClipboardImage>>;

    /// Clone this reader into a boxed trait object.
    fn clone_box(&self) -> Box<dyn ClipboardReader>;
}

/// A no-op clipboard for tests and deterministic fixtures.
#[cfg(any(test, feature = "fixture"))]
#[derive(Debug, Clone, Default)]
pub struct MockClipboard;

#[cfg(any(test, feature = "fixture"))]
#[async_trait::async_trait]
impl ClipboardSink for MockClipboard {
    async fn write_text(&self, _text: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(any(test, feature = "fixture"))]
#[async_trait::async_trait]
impl ClipboardReader for MockClipboard {
    async fn read_text(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn read_image(&self) -> anyhow::Result<Option<ClipboardImage>> {
        Ok(None)
    }

    fn clone_box(&self) -> Box<dyn ClipboardReader> {
        Box::new(Self)
    }
}

/// The sink and reader for a process with no clipboard it can reach: no display, as over ssh, or a
/// build without the `clipboard` feature.
///
/// A write is refused rather than swallowed, because the refusal is what routes the copy to the
/// terminal's own clipboard instead; a sink that said yes left the reader with a "copied" toast
/// and nothing copied. Reading finds nothing, and the terminal's own paste still works.
#[derive(Debug, Clone, Default)]
pub struct UnavailableClipboard;

#[async_trait::async_trait]
impl ClipboardSink for UnavailableClipboard {
    async fn write_text(&self, _text: &str) -> anyhow::Result<()> {
        anyhow::bail!("no clipboard backend")
    }
}

#[async_trait::async_trait]
impl ClipboardReader for UnavailableClipboard {
    async fn read_text(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn read_image(&self) -> anyhow::Result<Option<ClipboardImage>> {
        Ok(None)
    }

    fn clone_box(&self) -> Box<dyn ClipboardReader> {
        Box::new(Self)
    }
}

pub(crate) fn initial_sink() -> std::sync::Arc<dyn ClipboardSink> {
    #[cfg(any(test, feature = "fixture"))]
    let sink = MockClipboard;
    #[cfg(not(any(test, feature = "fixture")))]
    let sink = UnavailableClipboard;
    std::sync::Arc::new(sink)
}

/// System clipboard sink using `arboard` (requires the `clipboard` feature).
#[cfg(feature = "clipboard")]
pub struct SystemClipboard {
    clipboard: std::sync::Mutex<arboard::Clipboard>,
}

#[cfg(feature = "clipboard")]
impl SystemClipboard {
    /// Create a new system clipboard sink. Returns None if the clipboard is unavailable
    /// (e.g., headless environment).
    pub fn new() -> Option<Self> {
        arboard::Clipboard::new().ok().map(|cb| Self { clipboard: std::sync::Mutex::new(cb) })
    }
}

#[cfg(feature = "clipboard")]
#[async_trait::async_trait]
impl ClipboardSink for SystemClipboard {
    async fn write_text(&self, text: &str) -> anyhow::Result<()> {
        // Wayland first, for the reason in the module docs: arboard here can only reach X11, and a
        // Wayland paste would keep returning whatever was on the clipboard before.
        #[cfg(target_os = "linux")]
        if write_wayland_clipboard(text).await {
            return Ok(());
        }
        let mut cb =
            self.clipboard.lock().map_err(|_| anyhow::anyhow!("clipboard lock poisoned"))?;
        cb.set_text(text).map_err(|e| anyhow::anyhow!("clipboard error: {e}"))?;
        Ok(())
    }
}

#[cfg(feature = "clipboard")]
#[async_trait::async_trait]
impl ClipboardReader for SystemClipboard {
    async fn read_text(&self) -> anyhow::Result<Option<String>> {
        #[cfg(target_os = "linux")]
        if let Some(bytes) = read_wayland_clipboard(Some("text/plain;charset=utf-8")).await {
            return Ok(String::from_utf8(bytes).ok());
        }

        let mut cb =
            self.clipboard.lock().map_err(|_| anyhow::anyhow!("clipboard lock poisoned"))?;
        match cb.get_text() {
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("clipboard error: {e}")),
        }
    }

    async fn read_image(&self) -> anyhow::Result<Option<ClipboardImage>> {
        #[cfg(all(target_os = "linux", feature = "clipboard-images"))]
        if let Some(bytes) = read_wayland_clipboard(Some("image/png")).await {
            return Ok(decode_clipboard_image(&bytes));
        }

        let mut cb =
            self.clipboard.lock().map_err(|_| anyhow::anyhow!("clipboard lock poisoned"))?;
        match cb.get_image() {
            Ok(img) => {
                let clipboard_image = ClipboardImage {
                    pixels: img.bytes.to_vec(),
                    width: img.width as u32,
                    height: img.height as u32,
                };
                Ok(Some(clipboard_image))
            }
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("clipboard error: {e}")),
        }
    }

    fn clone_box(&self) -> Box<dyn ClipboardReader> {
        SystemClipboard::new()
            .map(|cb| Box::new(cb) as Box<dyn ClipboardReader>)
            .unwrap_or_else(|| Box::new(UnavailableClipboard))
    }
}

/// Hands `text` to `wl-copy`, returning whether it took it.
///
/// `wl-copy` forks a background process that serves the selection until something replaces it, so
/// the direct child exits immediately; stdio is null'd because inheriting it would make the wait
/// hang on the *daemon* rather than the parent.
#[cfg(all(feature = "clipboard", target_os = "linux"))]
async fn write_wayland_clipboard(text: &str) -> bool {
    use tokio::io::AsyncWriteExt;

    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    let mut child = match tokio::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(%error, "wl-copy unavailable; falling back to arboard");
            return false;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(text.as_bytes()).await {
            tracing::warn!(%error, "wl-copy stdin write failed");
            return false;
        }
        // Dropping stdin is the EOF wl-copy waits for before it forks and exits.
        drop(stdin);
    }
    match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.success() => true,
        Ok(Ok(status)) => {
            tracing::warn!(?status, "wl-copy exited unsuccessfully");
            false
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "wl-copy failed");
            false
        }
        Err(_) => {
            tracing::warn!("wl-copy timed out");
            false
        }
    }
}

#[cfg(all(feature = "clipboard", target_os = "linux"))]
async fn read_wayland_clipboard(mime: Option<&str>) -> Option<Vec<u8>> {
    std::env::var_os("WAYLAND_DISPLAY")?;

    let mut command = tokio::process::Command::new("wl-paste");
    command.arg("--no-newline").kill_on_drop(true);
    if let Some(mime) = mime {
        command.args(["--type", mime]);
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(1), command.output())
        .await
        .ok()?
        .ok()?;
    (output.status.success() && !output.stdout.is_empty()).then_some(output.stdout)
}

#[cfg(all(feature = "clipboard-images", target_os = "linux"))]
fn decode_clipboard_image(bytes: &[u8]) -> Option<ClipboardImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(ClipboardImage { width: image.width(), height: image.height(), pixels: image.into_raw() })
}

/// Create the process's clipboard sink. Uses `SystemClipboard` if the feature is enabled and
/// available, otherwise [`UnavailableClipboard`].
///
/// Shared rather than cloned per copy: on X11 the selection belongs to the live `arboard::Clipboard`
/// and evaporates when it drops, so a per-copy instance would surrender the clipboard the moment it
/// finished writing to it.
pub async fn create_clipboard() -> std::sync::Arc<dyn ClipboardSink> {
    #[cfg(feature = "clipboard")]
    {
        if let Some(cb) = SystemClipboard::new() {
            return std::sync::Arc::new(cb);
        }
    }
    fallback_sink()
}

/// The sink when no system clipboard is reachable: one that refuses, so the copy goes on to the
/// terminal.
fn fallback_sink() -> std::sync::Arc<dyn ClipboardSink> {
    std::sync::Arc::new(UnavailableClipboard)
}

/// Create a clipboard reader. Uses `SystemClipboard` if the feature is enabled and
/// available, otherwise [`UnavailableClipboard`].
pub async fn create_reader() -> Box<dyn ClipboardReader> {
    #[cfg(feature = "clipboard")]
    {
        if let Some(cb) = SystemClipboard::new() {
            return Box::new(cb);
        }
    }
    Box::new(UnavailableClipboard)
}

/// Largest OSC 52 payload we will emit, in base64 characters.
///
/// Terminals and multiplexers cap the sequence (tmux at ~74 KB by default); a payload past the limit
/// is dropped silently or, worse, printed. Refusing early keeps the failure honest.
const OSC52_MAX_BASE64: usize = 64 * 1024;

/// The OSC 52 escape sequence that asks the *terminal* to put `text` on its clipboard.
///
/// The last resort behind the local backends, and the only one that works over ssh or through a
/// multiplexer, where the process has no display of its own to talk to. Returns `None` when the
/// payload is too large to be carried reliably.
pub fn osc52_sequence(text: &str) -> Option<String> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let encoded = STANDARD.encode(text.as_bytes());
    (encoded.len() <= OSC52_MAX_BASE64).then(|| format!("\x1b]52;c;{encoded}\x07"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no clipboard to reach, a copy is refused, which is what sends it to the terminal.
    ///
    /// The break this catches: the fallback was the test mock, whose write succeeded doing
    /// nothing, so over ssh the OSC 52 path never ran and "copied" copied nothing.
    #[tokio::test]
    async fn an_unreachable_clipboard_refuses_the_copy() {
        assert!(UnavailableClipboard.write_text("text").await.is_err());
        assert!(UnavailableClipboard.read_text().await.unwrap().is_none());
    }

    /// The sequence has to be exactly what terminals expect, payload base64 and all.
    #[test]
    fn osc52_carries_the_selection_as_base64() {
        let sequence = osc52_sequence("hi").expect("a small payload is carried");
        assert_eq!(sequence, "\x1b]52;c;aGk=\x07");
        // Too big to travel reliably: say so rather than emitting a sequence that gets truncated.
        assert!(osc52_sequence(&"x".repeat(OSC52_MAX_BASE64)).is_none());
    }

    #[tokio::test]
    async fn test_mock_clipboard_never_fails() {
        let cb = MockClipboard;
        assert!(cb.write_text("hello").await.is_ok());
        assert!(cb.write_text("").await.is_ok());
    }

    #[cfg(feature = "clipboard")]
    #[tokio::test]
    async fn test_system_clipboard_creation() {
        // May be None in headless environments — that's fine.
        let _ = SystemClipboard::new();
    }

    #[tokio::test]
    async fn the_fallback_sink_refuses_so_the_terminal_gets_the_copy() {
        assert!(fallback_sink().write_text("text").await.is_err());
    }

    #[cfg(all(feature = "clipboard-images", target_os = "linux"))]
    #[test]
    fn decodes_wayland_png_as_rgba() {
        let source = image::RgbaImage::from_raw(1, 1, vec![10, 20, 30, 40]).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();

        let decoded = decode_clipboard_image(png.get_ref()).unwrap();
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.pixels, vec![10, 20, 30, 40]);
    }
}
