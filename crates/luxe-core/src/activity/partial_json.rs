//! Partial JSON — what can honestly be read from a tool call's arguments while they stream.
//!
//! Providers stream a tool call's arguments as raw JSON fragments, so the buffer is almost never
//! parseable: it ends mid-string, mid-value, mid-object. [`PartialObject`] walks one such buffer's
//! structure once, stepping over each string whole, and answers the two questions a card asks of
//! it: which members have arrived complete, and how much of one still-streaming string is whole.

/// A half-written JSON object, as far as it has arrived.
pub(super) struct PartialObject<'a>(pub(super) &'a str);

impl PartialObject<'_> {
    /// The complete top-level members, as an object.
    ///
    /// Cutting the buffer after the last member that *finished* and closing the brace yields what
    /// did arrive whole, which is the most that can be said honestly. `None` before the first
    /// member completes, and for a buffer that is not an object.
    ///
    /// A member finishes when its value does — not when a comma follows it. The comma is the
    /// obvious reading and it strands the commonest shape there is: `{"command": "…"}` has exactly
    /// one member and therefore never has a top-level comma, so a `bash` card would stay nameless
    /// until the closing brace.
    ///
    /// Unbounded, and cheap anyway: a value is stepped over in one move rather than byte by byte,
    /// this runs only until the call *has* a name, and the body that keeps reading is rate-limited.
    /// A length cap would bound nothing the loop actually spends — it would only throw away an
    /// answer the scan had already found, leaving an `edit` whose path sits at byte thirty with no
    /// name once its edits run long.
    pub(super) fn complete_members(&self) -> Option<serde_json::Value> {
        let mut depth = 0usize;
        // Index just past the last complete top-level member.
        let mut cut = None;
        // Whether the next top-level token is a member's *value*. Read from the colon behind it
        // rather than from whatever follows it, because what follows may not have been written
        // yet — and a string that has closed with nothing after it is a key and a value in equal
        // measure.
        let mut expect_value = false;
        for token in self.tokens() {
            match token {
                Token::String { end, .. } if depth == 1 && expect_value => {
                    cut = Some(end);
                    expect_value = false;
                }
                Token::Open => depth += 1,
                // A closer with nothing open is not a fragment of anything this can read.
                Token::Close(at) => {
                    depth = depth.checked_sub(1)?;
                    // The whole thing arrived after all; nothing to repair.
                    if depth == 0 {
                        return serde_json::from_str(&self.0[..=at]).ok();
                    }
                    // A nested value closing is also a member finishing.
                    if depth == 1 {
                        cut = Some(at + 1);
                        expect_value = false;
                    }
                }
                Token::Colon if depth == 1 => expect_value = true,
                // What closes a bare value — a number, a boolean — since it has no closer of its own.
                Token::Comma(at) if depth == 1 => {
                    cut = Some(at);
                    expect_value = false;
                }
                _ => {}
            }
        }
        serde_json::from_str(&format!("{}}}", &self.0[..cut?])).ok()
    }

    /// The value of a top-level string member, decoded as far as it is whole, even while it is
    /// still arriving.
    pub(super) fn string_member(&self, key: &str) -> Option<String> {
        let rest = &self.0[self.string_member_start(key)?..];
        let prefix = &rest[..decodable_prefix(rest)];
        // Closing the quote and letting serde unescape it is what keeps surrogate pairs, `\u`
        // escapes and the rest of the grammar correct without a second decoder to keep in step
        // with the first.
        serde_json::from_str::<String>(&format!("\"{prefix}\"")).ok()
    }

    /// Where `key`'s string value begins, for a key at the top level.
    fn string_member_start(&self, key: &str) -> Option<usize> {
        let bytes = self.0.as_bytes();
        let skip_space = |mut at: usize| {
            while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                at += 1;
            }
            at
        };
        let mut depth = 0usize;
        for token in self.tokens() {
            match token {
                Token::Open => depth += 1,
                Token::Close(_) => depth = depth.checked_sub(1)?,
                // A key is a top-level string with a colon after it. Anything else is a value,
                // and stepping over it whole is how a `{` inside one stays out of the depth count.
                Token::String { start, end } if depth == 1 => {
                    let next = skip_space(end);
                    if bytes.get(next) != Some(&b':') {
                        continue;
                    }
                    let value = skip_space(next + 1);
                    if &self.0[start + 1..end - 1] == key && bytes.get(value) == Some(&b'"') {
                        return Some(value + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn tokens(&self) -> Tokens<'_> {
        Tokens { bytes: self.0.as_bytes(), at: 0 }
    }
}

/// One structural step through a half-written JSON document.
enum Token {
    /// `{` or `[`
    Open,
    /// `}` or `]`, at its index
    Close(usize),
    /// A whole string: the index of its opening quote, and the index just past its closing one
    String {
        start: usize,
        end: usize,
    },
    Colon,
    /// `,`, at its index
    Comma(usize),
}

/// The structural tokens of a buffer, in order, until it ends or a string has not closed yet.
struct Tokens<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Iterator for Tokens<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        loop {
            let here = self.at;
            self.at += 1;
            return Some(match *self.bytes.get(here)? {
                b'"' => {
                    let end = string_end(self.bytes, here)?;
                    self.at = end;
                    Token::String { start: here, end }
                }
                b'{' | b'[' => Token::Open,
                b'}' | b']' => Token::Close(here),
                b':' => Token::Colon,
                b',' => Token::Comma(here),
                _ => continue,
            });
        }
    }
}

/// The index just past a JSON string that starts at `at`, or `None` when it has not closed yet.
fn string_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut index = at + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// How much of a half-arrived string body is a complete, decodable prefix.
///
/// Stops at the closing quote when the value has in fact landed, and before a trailing escape the
/// model is still in the middle of (`\`, `\u00`) — which would otherwise fail the decode outright
/// and drop a frame of the file the reader is watching arrive.
fn decodable_prefix(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let mut at = 0usize;
    let mut whole = 0usize;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => return whole,
            b'\\' => {
                let width = match bytes.get(at + 1) {
                    None => return whole,
                    Some(b'u') => 6,
                    Some(_) => 2,
                };
                if at + width > bytes.len() {
                    return whole;
                }
                at += width;
            }
            // One whole character at a time, so a cut never lands inside a multi-byte one.
            _ => {
                at += 1;
                while bytes.get(at).is_some_and(|byte| byte & 0b1100_0000 == 0b1000_0000) {
                    at += 1;
                }
            }
        }
        whole = at;
    }
    whole
}

#[cfg(test)]
mod tests;
