//! Images — keys for pasted ones, taking payloads into the store, and handing a picture or an
//! address to the desktop.

use super::{App, ConversationItem, ConvoBlock};

impl App {
    /// Puts images attached to a message into the transcript, after the message itself.
    pub(super) fn push_attached_images(&mut self, images: &[crate::editor::AttachedImage]) {
        for image in images {
            let key = self.attachment_image_key();
            let origin = self.image_origin("pasted image", Some(&image.filename));
            self.intern_image(&key, &image.data, origin);
            self.push_convo(ConvoBlock::Image { key, width: image.width, height: image.height });
        }
    }

    /// A fresh key for an image attached to the message being composed.
    ///
    /// Keyed by the provisional turn the attachment lands in, which is monotonic within a session,
    /// so two pastes of the same picture stay separate images in the transcript.
    pub fn attachment_image_key(&mut self) -> crate::graphics::ImageKey {
        self.next_attachment_image += 1;
        crate::graphics::ImageKey::new(format!(
            "paste:{}:{}",
            self.session_id.clone().unwrap_or_default(),
            self.next_attachment_image
        ))
    }

    /// Takes a base64 image into the store so the transcript can draw it.
    ///
    /// Decoding here rather than at paint time keeps the render path free of a megabyte of base64
    /// per frame; a payload the store refuses (unreadable, or larger than the caps allow) leaves
    /// the key unknown, which every drawing surface already reads as "say it in words instead".
    pub fn intern_image(
        &mut self,
        key: &crate::graphics::ImageKey,
        data: &str,
        origin: crate::graphics::ImageOrigin,
    ) -> bool {
        use base64::Engine as _;
        if self.images.contains(key) {
            return true;
        }
        match base64::engine::general_purpose::STANDARD.decode(data) {
            Ok(bytes) => self.images.insert(key.clone(), bytes, origin),
            Err(error) => {
                tracing::warn!(%error, "inline image payload is not valid base64");
                false
            }
        }
    }

    /// Names an image the way the transcript should, vetting any path it claims to live at.
    pub fn image_origin(
        &self,
        label: impl Into<String>,
        name: Option<&str>,
    ) -> crate::graphics::ImageOrigin {
        let cwd = std::env::current_dir().unwrap_or_default();
        crate::graphics::ImageOrigin::new(label, name, &cwd)
    }

    /// The image `Alt+I` should open: the selected message's, else the newest in the transcript.
    ///
    /// Newest rather than first, because the reason to reach for the key is a capture that just
    /// landed. A selected message wins when it has one, so the key can also be aimed.
    pub fn newest_visible_image(&self) -> Option<crate::graphics::ImageKey> {
        let from_item = |item: &ConversationItem| match item.block() {
            ConvoBlock::Image { key, .. } => Some(key.clone()),
            ConvoBlock::Tool(tool) => self
                .tool_images
                .get(&(tool.activity.session_id.clone(), tool.activity.call_id.clone()))
                .and_then(|keys| keys.last().cloned()),
            _ => None,
        };
        self.selected_block
            .and_then(|index| self.convo.get(index))
            .and_then(from_item)
            .or_else(|| self.convo.iter().rev().find_map(from_item))
    }

    /// Hands an image to whatever the desktop opens images with.
    ///
    /// The answer for a terminal that cannot draw one inline: the picture is on disk either way, so
    /// rather than approximate it in the transcript, hand over the real thing. The path was vetted
    /// when the image was taken in — it exists, it is a regular file, it has an image extension —
    /// because the string came from a model.
    ///
    /// Detached on purpose: an image viewer outlives the click, and waiting on one would hold the
    /// event loop until the reader closed it.
    pub fn open_image_externally(&mut self, key: &crate::graphics::ImageKey) {
        let Some(path) = self.images.origin(key).and_then(|origin| origin.path.clone()) else {
            return;
        };
        let label = luxe_core::paths::display(&path);
        self.open_externally(path.as_os_str(), &label);
    }

    /// Hands a file or an address to the desktop's opener, detached, and says so if it cannot.
    ///
    /// `label` is how the target is named in a failure, so a path reads as a path and an address as
    /// an address.
    pub fn open_externally(&mut self, target: &std::ffi::OsStr, label: &str) {
        if !self.opener.should_launch(label) {
            return;
        }
        let Some((program, leading)) = luxe_core::program::desktop_opener() else {
            self.push_convo(ConvoBlock::System(format!(
                "[cannot open {label}: this system has no desktop opener]"
            )));
            return;
        };
        let spawned = std::process::Command::new(program)
            .args(leading)
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Err(error) = spawned {
            tracing::warn!(%error, target = label, "could not open externally");
            self.push_convo(ConvoBlock::System(format!("[could not open {label}: {error}]")));
        }
    }
}
