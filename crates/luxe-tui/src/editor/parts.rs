//! Parts — the atomic tokens a draft holds: compacted pastes, images and `@` references.

use super::Editor;

/// An image attached to the editor input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedImage {
    /// Base64-encoded image data
    pub data: String,
    /// MIME type sent to the provider
    pub mime: String,
    /// Display filename synthesized for clipboard images
    pub filename: String,
    /// Original width in pixels
    pub width: u32,
    /// Original height in pixels
    pub height: u32,
}

#[derive(Debug, Clone)]
pub(super) enum PromptPartKind {
    PastedText {
        text: String,
    },
    Image {
        image: AttachedImage,
    },
    /// A reference picked from the `@` picker: sent as written, held as one token
    Reference,
}

#[derive(Debug, Clone)]
pub(super) struct PromptPart {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) kind: PromptPartKind,
}

const PASTE_SUMMARY_MIN_LINES: usize = 3;

const PASTE_SUMMARY_MIN_BYTES: usize = 150;

pub(super) fn shift_offset(offset: usize, inserted: usize, removed: usize) -> usize {
    if inserted >= removed {
        offset.saturating_add(inserted - removed)
    } else {
        offset.saturating_sub(removed - inserted)
    }
}

impl Editor {
    /// Inserts terminal-pasted text, compacting large payloads into an atomic token.
    pub fn paste_text(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let line_count = normalized.chars().filter(|character| *character == '\n').count() + 1;
        if line_count >= PASTE_SUMMARY_MIN_LINES || normalized.len() > PASTE_SUMMARY_MIN_BYTES {
            let marker = format!("[Pasted {line_count} lines]");
            self.insert_part(marker, PromptPartKind::PastedText { text: normalized });
        } else {
            self.insert_text(&normalized);
        }
    }

    /// Attaches raw clipboard RGBA pixels as a PNG-backed atomic prompt token.
    ///
    /// Returns `false` when image support is disabled or the pixel buffer is invalid.
    pub fn attach_image(&mut self, pixels: Vec<u8>, width: u32, height: u32) -> bool {
        let Some(encoded) = encode_clipboard_png(pixels, width, height) else {
            return false;
        };
        let image_number = self
            .parts
            .iter()
            .filter(|part| matches!(part.kind, PromptPartKind::Image { .. }))
            .count()
            + 1;
        let filename = if image_number == 1 {
            "clipboard.png".to_string()
        } else {
            format!("clipboard-{image_number}.png")
        };
        let marker = "[Pasted Image]".to_string();
        self.insert_part(
            marker,
            PromptPartKind::Image {
                image: AttachedImage {
                    data: encoded,
                    mime: "image/png".to_string(),
                    filename,
                    width,
                    height,
                },
            },
        );
        true
    }

    /// Puts messages back at the head of the draft, each with the images it was sent with.
    ///
    /// How a waiting submission is taken back for editing: it goes *before* whatever is being typed
    /// now, because it was typed first, and the cursor lands at the end of the restored text — the
    /// thing the user came back to change. Images are re-attached as the same atomic tokens a paste
    /// produces rather than re-encoded, so nothing is lost on the round trip.
    pub fn prepend_messages(
        &mut self,
        messages: impl IntoIterator<Item = (String, Vec<AttachedImage>)>,
    ) {
        let occupied = !self.buffer.is_empty();
        self.cursor = 0;
        let mut restored = false;
        for (text, images) in messages {
            if restored {
                self.insert_text("\n\n");
            }
            restored = true;
            self.insert_text(&text);
            for image in images {
                self.insert_part("[Pasted Image]".to_string(), PromptPartKind::Image { image });
            }
        }
        if restored && occupied {
            let resume = self.cursor;
            self.insert_text("\n\n");
            self.cursor = resume;
        }
        self.history_index = None;
        self.selection_anchor = None;
    }

    /// Inserts a reference — `@[file:src/main.rs]`, `@[github:owner/repo/pull/7]` — at the
    /// cursor as one token: drawn as a chip, stepped over by the cursor, deleted whole.
    pub fn insert_reference(&mut self, token: &str) {
        self.insert_part(token.to_string(), PromptPartKind::Reference);
    }

    /// Removes the last image attachment and its display token.
    pub fn remove_last_image(&mut self) {
        if let Some(index) =
            self.parts.iter().rposition(|part| matches!(part.kind, PromptPartKind::Image { .. }))
        {
            let range = self.parts[index].start..self.parts[index].end;
            self.replace_range(range, "");
        }
    }

    /// Reports whether the draft contains image attachments.
    pub fn has_images(&self) -> bool {
        self.parts.iter().any(|part| matches!(part.kind, PromptPartKind::Image { .. }))
    }

    fn insert_part(&mut self, marker: String, kind: PromptPartKind) {
        self.clamp_cursor();
        let start = self.cursor;
        self.replace_range(start..start, &marker);
        self.parts.push(PromptPart { start, end: start + marker.len(), kind });
        self.parts.sort_by_key(|part| part.start);
    }

    /// The draft's text in `range`, with every part wholly inside it expanded: a paste to what was
    /// pasted, an image to nothing, and a reference left as written.
    pub(super) fn expand_parts(&self, range: std::ops::Range<usize>) -> String {
        let mut text = self.buffer[range.clone()].to_string();
        let mut parts = self
            .parts
            .iter()
            .filter(|part| part.start >= range.start && part.end <= range.end)
            .collect::<Vec<_>>();
        // From the back, so each part's offsets still hold when it is replaced
        parts.sort_by_key(|part| std::cmp::Reverse(part.start));
        for part in parts {
            let replacement = match &part.kind {
                PromptPartKind::PastedText { text } => text.as_str(),
                PromptPartKind::Image { .. } => "",
                PromptPartKind::Reference => continue,
            };
            text.replace_range(part.start - range.start..part.end - range.start, replacement);
        }
        text
    }

    pub(super) fn snap_out_of_part(&self, offset: usize, forward: bool) -> usize {
        self.parts
            .iter()
            .find(|part| offset > part.start && offset < part.end)
            .map_or(offset, |part| if forward { part.end } else { part.start })
    }
}

#[cfg(feature = "clipboard-images")]
fn encode_clipboard_png(pixels: Vec<u8>, width: u32, height: u32) -> Option<String> {
    use base64::Engine;
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use std::io::Cursor;

    let image = RgbaImage::from_raw(width, height, pixels)?;
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image).write_to(&mut encoded, ImageFormat::Png).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(encoded.into_inner()))
}

#[cfg(not(feature = "clipboard-images"))]
fn encode_clipboard_png(_pixels: Vec<u8>, _width: u32, _height: u32) -> Option<String> {
    None
}
