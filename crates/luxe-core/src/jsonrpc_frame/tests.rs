use super::*;

async fn read_all(wire: &[u8]) -> std::io::Result<Option<Vec<u8>>> {
    let mut reader = tokio::io::BufReader::new(wire);
    read_frame(&mut reader, MAX_FRAME_BYTES).await
}

#[tokio::test]
async fn a_written_frame_reads_back_and_then_ends() {
    let mut wire = Vec::new();
    write_frame(&mut wire, br#"{"seq":1}"#).await.unwrap();
    assert_eq!(wire, b"Content-Length: 9\r\n\r\n{\"seq\":1}");

    let mut reader = tokio::io::BufReader::new(&wire[..]);
    assert_eq!(read_frame(&mut reader, MAX_FRAME_BYTES).await.unwrap().unwrap(), br#"{"seq":1}"#);
    assert!(read_frame(&mut reader, MAX_FRAME_BYTES).await.unwrap().is_none());
}

/// The break this catches: after `Content-Length` the readers took two bytes as the blank line,
/// so a `Content-Type` header, which both specs allow, was read as the body's start.
#[tokio::test]
async fn a_content_type_header_does_not_desynchronize_the_stream() {
    let wire =
        b"Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
    assert_eq!(read_all(wire).await.unwrap().unwrap(), b"{}");
    let first = b"Content-Type: x\r\nContent-Length: 2\r\n\r\n[]";
    assert_eq!(read_all(first).await.unwrap().unwrap(), b"[]");
}

/// Header names are case-insensitive, and the space after the colon optional.
#[tokio::test]
async fn header_names_match_without_regard_to_case() {
    assert_eq!(read_all(b"content-length:2\r\n\r\n{}").await.unwrap().unwrap(), b"{}");
    assert_eq!(read_all(b"CONTENT-LENGTH: 2\r\n\r\n{}").await.unwrap().unwrap(), b"{}");
}

/// The break this catches: the body was `vec![0; len]` from the server's own number, so one lie
/// aborted the process on allocation failure.
#[tokio::test]
async fn a_length_over_the_cap_is_refused_before_reading() {
    let error = read_all(b"Content-Length: 99999999999\r\n\r\n{}").await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");

    let mut reader = tokio::io::BufReader::new(&b"Content-Length: 5\r\n\r\nhello"[..]);
    let error = read_frame(&mut reader, 4).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
}

/// A claimed length the stream never delivers is an early end, not a padded body.
#[tokio::test]
async fn a_short_body_is_an_early_end() {
    let error = read_all(b"Content-Length: 50\r\n\r\n{}").await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof, "{error}");
    let error = read_all(b"Content-Length: 2\r\n").await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof, "{error}");
}

/// The break this catches: `Content-Length: ` with no number panicked the old sync reader.
#[tokio::test]
async fn a_header_with_no_usable_length_is_invalid() {
    for wire in [&b"Content-Length: \r\n\r\n{}"[..], b"Content-Type: x\r\n\r\n{}"] {
        let error = read_all(wire).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
    }
}

/// A header line with no end is refused once it passes its bound, rather than read forever.
#[tokio::test]
async fn an_endless_header_line_is_refused() {
    let wire = vec![b'x'; (MAX_HEADER_LINE as usize) * 2];
    let error = read_all(&wire).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
}
