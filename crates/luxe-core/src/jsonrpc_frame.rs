//! JSON-RPC frames — the `Content-Length` framing language servers and debug adapters speak.
//!
//! One reader and one writer for both protocols, because the copies drifted: one header line was
//! assumed, so a server that also sent `Content-Type` desynchronized the stream for good, and the
//! body was allocated from whatever length the server claimed. The servers can come from the
//! repository under review, so a claimed length is bounded before anything is read.

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest body a frame may carry. Generous: a workspace symbol dump or a big file's semantic
/// tokens run to megabytes, and this is a guard against a lie, not a budget.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// The longest header line read before the frame is refused, so a stream with no line break
/// cannot grow one without bound either.
const MAX_HEADER_LINE: u64 = 8 * 1024;

/// Writes `body` as one frame and flushes it.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
) -> std::io::Result<()> {
    writer.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

/// Reads one frame's body, or `None` at a clean end of stream.
///
/// Header names match without regard to case and any header but `Content-Length` is ignored, as
/// both specs allow. A length over `max_bytes`, a header block with no length, or a stream that
/// ends mid-frame is `InvalidData` or `UnexpectedEof`; the body is read through `take`, so a
/// claimed length never allocates more than arrives.
pub async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let Some(len) = read_header(reader, max_bytes).await? else { return Ok(None) };
    let mut body = Vec::with_capacity(len.min(64 * 1024));
    let read = (&mut *reader).take(len as u64).read_to_end(&mut body).await?;
    if read < len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("frame ended after {read} of {len} bytes"),
        ));
    }
    Ok(Some(body))
}

/// The `Content-Length` of the next header block, or `None` at end of stream before one starts.
async fn read_header<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<usize>> {
    let mut length = None;
    let mut started = false;
    loop {
        let mut line = Vec::new();
        let read = (&mut *reader).take(MAX_HEADER_LINE).read_until(b'\n', &mut line).await?;
        if read == 0 {
            return if started {
                Err(eof("the stream ended inside a frame header"))
            } else {
                Ok(None)
            };
        }
        if !line.ends_with(b"\n") {
            return Err(invalid("a frame header line has no end"));
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            // a stray blank line between frames is not a header block
            if !started {
                continue;
            }
            return length.map(Some).ok_or_else(|| invalid("a frame header has no Content-Length"));
        }
        started = true;
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let len: usize = value.trim().parse().map_err(|_| invalid("bad Content-Length"))?;
            if len > max_bytes {
                return Err(invalid(format!(
                    "a {len}-byte frame is over the {max_bytes}-byte cap"
                )));
            }
            length = Some(len);
        }
    }
}

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

fn eof(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, message)
}

#[cfg(test)]
mod tests;
