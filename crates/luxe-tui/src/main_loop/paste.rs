//! Paste — reading the system clipboard off the event loop.

use tokio::sync::mpsc;

/// What a clipboard read found, handed back to the loop.
pub(super) enum ClipboardPaste {
    Text(String),
    Image { pixels: Vec<u8>, width: u32, height: u32 },
}

/// Reads the clipboard off the loop, an image first and then text, and sends back what it found.
pub(super) fn spawn_clipboard_read(tx: mpsc::Sender<ClipboardPaste>) {
    tokio::spawn(async move {
        let reader = crate::clipboard::create_reader().await;
        let image = match reader.read_image().await {
            Ok(image) => image,
            Err(error) => {
                tracing::warn!(%error, "clipboard image read failed; trying text");
                None
            }
        };
        let payload = if let Some(image) = image {
            Some(ClipboardPaste::Image {
                pixels: image.pixels,
                width: image.width,
                height: image.height,
            })
        } else {
            match reader.read_text().await {
                Ok(Some(text)) => Some(ClipboardPaste::Text(text)),
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(%error, "clipboard text paste failed");
                    None
                }
            }
        };
        if let Some(payload) = payload {
            if tx.send(payload).await.is_err() {
                tracing::warn!("clipboard paste result was dropped");
            }
        }
    });
}

/// Hands what a clipboard read found to the app.
pub(super) fn deliver(app: &mut crate::app::App, payload: ClipboardPaste) {
    match payload {
        ClipboardPaste::Text(text) => app.update(crate::app::Msg::PasteText(text)),
        ClipboardPaste::Image { pixels, width, height } => {
            app.update(crate::app::Msg::ImagePasted { pixels, width, height });
        }
    }
}
