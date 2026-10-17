//! Prompt answers — what a login prompt has collected so far, held so it cannot leak.

use secrecy::zeroize::Zeroize;
use secrecy::SecretString;

/// Room for any key without growing: a reallocation would leave a copy of the prefix behind.
const RESERVED: usize = 512;

/// The text typed or pasted into a prompt. Usually an API key, so `Debug` never shows it and the
/// bytes are wiped when it is cleared or dropped.
pub(super) struct AnswerBuffer(String);

impl Default for AnswerBuffer {
    fn default() -> Self {
        Self(String::with_capacity(RESERVED))
    }
}

impl AnswerBuffer {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }

    pub(super) fn push(&mut self, ch: char) {
        self.0.push(ch);
    }

    pub(super) fn push_str(&mut self, text: &str) {
        self.0.push_str(text);
    }

    pub(super) fn pop(&mut self) {
        self.0.pop();
    }

    pub(super) fn clear(&mut self) {
        self.0.zeroize();
    }

    /// Hand the answer over as a secret. Copied into a box of its own size, since shrinking this
    /// buffer in place could leave the old block behind unwiped; the drop below wipes it.
    pub(super) fn into_secret(self) -> SecretString {
        SecretString::from(Box::<str>::from(self.0.as_str()))
    }
}

impl std::fmt::Debug for AnswerBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AnswerBuffer([REDACTED])")
    }
}

impl Drop for AnswerBuffer {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
