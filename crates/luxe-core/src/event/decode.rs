//! Decoding — how a frame becomes an `Action` or an `Event`.

use super::{Action, Event};
use serde::de::value::MapDeserializer;
use serde::{Deserialize, Deserializer};

mod known_action;
mod known_event;

use known_action::KnownAction;
use known_event::KnownEvent;

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // `"content":null` lands as `None` here, so an action reads it as no content at all
        #[derive(Deserialize)]
        struct Frame {
            kind: String,
            content: Option<serde_json::Value>,
        }
        let raw = serde_json::Value::deserialize(deserializer)?;
        let frame = Frame::deserialize(&raw)
            .map_err(|e| serde::de::Error::custom(format!("action: {e}")))?;
        decode_frame(frame.kind, frame.content, raw)
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = serde_json::Value::deserialize(deserializer)?;
        let fields = raw.as_object().ok_or_else(|| serde::de::Error::custom("expected object"))?;
        let kind = fields
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("expected string kind"))?
            .to_string();
        let content = fields.get("content").cloned();
        decode_frame(kind, content, raw)
    }
}

/// The part of a frame decoder serde derives: every known variant, decoded through a mirror enum.
///
/// The derive cannot express the rest, which [`decode_frame`] adds: an unknown tag becomes `Other`
/// with the whole frame kept, and a payload-less frame takes any of the empty forms.
trait KnownFrames: Sized {
    fn decode_known<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error>;
    fn unknown(kind: String, frame: serde_json::Value) -> Self;
}

impl KnownFrames for Action {
    fn decode_known<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        KnownAction::deserialize(deserializer)
    }

    fn unknown(kind: String, frame: serde_json::Value) -> Self {
        Self::Other { kind, content: frame }
    }
}

impl KnownFrames for Event {
    fn decode_known<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        KnownEvent::deserialize(deserializer)
    }

    fn unknown(kind: String, frame: serde_json::Value) -> Self {
        Self::Other { kind, content: frame }
    }
}

/// Decodes one frame from its tag and content, keeping the whole `frame` for an unknown tag.
///
/// A payload-less frame may be written `{"kind":"cancel"}`, with `"content":null` or with
/// `"content":{}` — refusing two of them would be pedantry a hand-written client trips over — but
/// real content on one is a mistake and is named as one. A payload frame with no content reads as
/// `{}`, so one whose fields all default can be sent bare. Errors name the frame they came from.
fn decode_frame<T, E>(
    kind: String,
    content: Option<serde_json::Value>,
    frame: serde_json::Value,
) -> Result<T, E>
where
    T: KnownFrames,
    E: serde::de::Error,
{
    let empty = content.as_ref().is_none_or(|content| {
        content.is_null() || content.as_object().is_some_and(serde_json::Map::is_empty)
    });
    match T::decode_known(TagOnly::frame(&kind)) {
        Err(TagOnly::Unknown) => Ok(T::unknown(kind, frame)),
        Ok(bare) if empty => Ok(bare),
        // an optional payload decodes bare too, so content is refused only where it cannot decode;
        // a bad value for one would be named as no content, which no variant here can hit yet
        Ok(_) => decode_content(&kind, content)
            .map_err(|_: E| E::custom(format!("{kind} takes no content"))),
        Err(TagOnly::NeedsContent) => decode_content(&kind, content),
    }
}

/// Decodes a known frame's payload, reading no content as `{}`.
fn decode_content<T, E>(kind: &str, content: Option<serde_json::Value>) -> Result<T, E>
where
    T: KnownFrames,
    E: serde::de::Error,
{
    let content = content.unwrap_or_else(|| serde_json::json!({}));
    T::decode_known(serde_json::json!({ "kind": kind, "content": content }))
        .map_err(|error| E::custom(format!("{kind}: {error}")))
}

/// What decoding a tag alone says: unknown, or known and wanting content.
///
/// A typed error rather than a message to match on. Decoding `{"kind": …}` with no content
/// succeeds for a payload-less variant, fails through `unknown_variant` for a tag the mirror lacks,
/// and fails for want of content otherwise — which is every other error a tag alone can raise.
#[derive(Debug)]
enum TagOnly {
    Unknown,
    NeedsContent,
}

impl TagOnly {
    fn frame(kind: &str) -> MapDeserializer<'_, std::iter::Once<(&'static str, &str)>, Self> {
        MapDeserializer::new(std::iter::once(("kind", kind)))
    }
}

impl std::fmt::Display for TagOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unknown => "unknown tag",
            Self::NeedsContent => "content required",
        })
    }
}

impl std::error::Error for TagOnly {}

impl serde::de::Error for TagOnly {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self::NeedsContent
    }

    fn unknown_variant(_: &str, _: &'static [&'static str]) -> Self {
        Self::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(tag = "kind", content = "content", rename_all = "snake_case")]
    enum Frames {
        Bare,
        Maybe(Option<u32>),
    }

    impl KnownFrames for Frames {
        fn decode_known<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Self::deserialize(deserializer)
        }

        fn unknown(_: String, _: serde_json::Value) -> Self {
            unreachable!("every tag here is known")
        }
    }

    fn decode(kind: &str, content: Option<serde_json::Value>) -> Result<Frames, serde_json::Error> {
        decode_frame(kind.into(), content, serde_json::Value::Null)
    }

    /// An optional payload decodes bare, like a payload-less frame, but its content is not refused.
    ///
    /// The break this catches: a tag that decodes without content was taken to mean the frame
    /// takes none, which would refuse every real value of a `Foo(Option<T>)` variant.
    #[test]
    fn an_optional_payload_is_read_not_refused() {
        assert_eq!(decode("maybe", None).unwrap(), Frames::Maybe(None));
        assert_eq!(decode("maybe", Some(serde_json::json!(3))).unwrap(), Frames::Maybe(Some(3)));
        let error = decode("bare", Some(serde_json::json!(3))).unwrap_err();
        assert_eq!(error.to_string(), "bare takes no content");
    }
}
