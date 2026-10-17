//! Engine types — image attachments. Config, actions and errors each have a module of their own.

use super::*;
use serde::{Deserialize, Serialize};

/// An image attached to a user message for vision model upload.
///
/// Serializable because it is part of the RPC wire vocabulary: a front-end that can paste a
/// screenshot has to be able to send one ([`crate::event::Action::SubmitUserMessage`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttachment {
    /// Base64-encoded image data.
    pub data: String,
    /// MIME type (e.g. "image/png").
    pub mime: String,
    /// Where it came from — a pasted filename, when there was one.
    pub name: Option<String>,
}

impl ImageAttachment {
    /// The content block this attachment travels as.
    pub fn to_block(&self) -> ContentBlock {
        ContentBlock::Image {
            data: self.data.clone(),
            mime: self.mime.clone(),
            name: self.name.clone(),
        }
    }
}
