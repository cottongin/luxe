use super::*;
use luxe_core::message::Message;
use luxe_core::session::EntryId;

fn messages() -> Vec<SessionMessage> {
    ["first", "second"]
        .into_iter()
        .map(|text| SessionMessage { entry_id: EntryId(text.into()), message: Message::user(text) })
        .collect()
}

async fn written<F: Serialize + ?Sized>(frame: &F) -> String {
    let mut out = Vec::new();
    write_frame(&mut out, frame).await.expect("writing to memory");
    String::from_utf8(out).expect("frames are UTF-8")
}

/// The shared transcript goes out byte for byte as the `session_loaded` event would.
///
/// The break this catches: the borrowed copy of the event's shape drifting from `Event`, a renamed
/// field or a lost `rename_all`, which every joining client would read as an unknown frame.
#[test]
fn the_shared_transcript_encodes_as_session_loaded() {
    let shared =
        SharedTranscript { session_id: "session-1".into(), messages: Arc::new(messages()) };
    let event = Event::SessionLoaded { session_id: "session-1".into(), messages: messages() };
    assert_eq!(serde_json::to_string(&shared).unwrap(), serde_json::to_string(&event).unwrap());
}

/// A frame bigger than several pieces arrives whole, as one line.
///
/// The break this catches: losing the last piece or the full ones, or cutting a piece a byte off,
/// which garbles exactly the large `session_loaded` frame a long session sends.
#[tokio::test]
async fn a_frame_of_many_pieces_is_written_whole() {
    let text: String =
        (0..CHUNK_BYTES * 3 + 17).map(|at| char::from(b'a' + (at % 26) as u8)).collect();
    let event = Event::Notice(text);
    let expected = format!("{}\n", serde_json::to_string(&event).unwrap());
    assert_eq!(written(&event).await, expected);
}

/// A large frame is held in pieces of one chunk at most, never one buffer of the whole.
///
/// The break this catches: dropping the cap on a piece, which grows the first one by doubling to the
/// frame's size, the very allocation this module exists to avoid.
#[test]
fn a_large_frame_is_held_in_chunk_sized_pieces() {
    let mut encoded = Pieces::default();
    serde_json::to_writer(&mut encoded, &"x".repeat(CHUNK_BYTES * 3)).unwrap();
    assert_eq!(encoded.full.len(), 3);
    assert!(encoded.full.iter().all(|piece| piece.len() == CHUNK_BYTES));
    assert!(encoded.current.capacity() <= CHUNK_BYTES, "{}", encoded.current.capacity());
}

/// A token's frame still fits in one piece, so the common case pays for no more than it did.
///
/// The break this catches: starting every frame on a full piece, a 64 KB allocation for each of
/// the thousands of token deltas a reply streams.
#[test]
fn a_small_frame_stays_in_its_first_piece() {
    let mut encoded = Pieces::default();
    serde_json::to_writer(&mut encoded, &Event::Notice("hi".into())).unwrap();
    assert!(encoded.full.is_empty());
    assert!(encoded.current.capacity() < CHUNK_BYTES, "{}", encoded.current.capacity());
}

/// Serializes as a list whose second element fails, after the first has been encoded.
struct FailsPartway;

impl Serialize for FailsPartway {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeSeq};
        let mut list = serializer.serialize_seq(Some(2))?;
        list.serialize_element("encoded")?;
        drop(list);
        Err(S::Error::custom("cannot encode the rest"))
    }
}

/// A frame that fails partway writes nothing rather than half a line.
///
/// The break this catches: streaming the encoding straight to the client, which leaves a broken
/// line that turns every later frame into a parse error.
#[tokio::test]
async fn a_frame_that_fails_partway_writes_nothing() {
    assert_eq!(written(&FailsPartway).await, "");
}
