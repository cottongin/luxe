//! output — the one stdout every command writes through.
//!
//! `println!` panics when the reader stops: `std::io::_print` unwraps, so `luxe doctor | head`
//! ends in `failed printing to stdout: Broken pipe` and a non-zero exit for an ordinary shell
//! idiom.
//!
//! The usual cure — resetting `SIGPIPE` to its default so the process dies silently — is wrong
//! here. Luxe writes to hook, MCP and LSP pipes and relies on `EPIPE` arriving as an error: the
//! command-hook runner deliberately tolerates a hook that exits without reading its stdin, and a
//! default `SIGPIPE` would kill Luxe at that line instead. So the pipe stays ignored and the
//! writing goes through one place, where a reader that has gone away is a quiet, successful end of
//! output.
//!
//! Once the reader is gone nothing further is written, and [`is_open`] lets a streaming loop stop
//! early rather than compose output nobody will read.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether stdout is still worth writing to. Latches false; nothing reopens a pipe.
static OPEN: AtomicBool = AtomicBool::new(true);

/// Whether anything written from here on would reach a reader.
///
/// A streaming loop — the headless event streams — checks this to stop at the next boundary
/// instead of driving a whole run into a closed pipe.
pub(crate) fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Write `text` to stdout, ending output for good if the reader has gone.
pub(crate) fn write(text: &str) {
    if !is_open() {
        return;
    }
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if let Err(error) = emit(&mut handle, text) {
        OPEN.store(false, Ordering::Relaxed);
        // A closed reader is the expected end of `| head`, and saying so would be the only thing
        // in the terminal. Anything else really is a failure to report.
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!("luxe: cannot write to stdout: {error}");
        }
    }
}

/// Write `text` through `sink` and get it out of the buffer.
///
/// Split out so the closed-reader path is testable against a sink that fails on command, which is
/// the only way to exercise it without a real pipe and a real reader that stops.
fn emit(sink: &mut impl Write, text: &str) -> std::io::Result<()> {
    sink.write_all(text.as_bytes())?;
    sink.flush()
}

/// `println!` for a reader that may stop reading. See [`crate::output`].
macro_rules! outln {
    () => { $crate::output::write("\n") };
    ($($arg:tt)*) => { $crate::output::write(&format!("{}\n", format_args!($($arg)*))) };
}

/// `print!` for a reader that may stop reading. See [`crate::output`].
macro_rules! out {
    ($($arg:tt)*) => { $crate::output::write(&format!("{}", format_args!($($arg)*))) };
}

pub(crate) use {out, outln};

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink that refuses every write with the error a stopped reader produces.
    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The defect, at the seam: a write to a reader that has stopped comes back as an error to
    /// handle, not as a panic. `println!` unwraps this exact error.
    #[test]
    fn a_stopped_reader_is_an_error_to_handle_rather_than_a_panic() {
        let error = emit(&mut ClosedPipe, "luxe 0.1.0\n").expect_err("the write cannot succeed");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    /// And an ordinary sink still gets every byte, flushed — a doctor line has to appear before
    /// the next language server is started, not when the command ends.
    #[test]
    fn an_open_reader_gets_the_bytes() {
        let mut sink = Vec::new();
        emit(&mut sink, "  ok  rust-analyzer\n").expect("an open sink accepts writes");
        assert_eq!(String::from_utf8(sink).expect("utf-8"), "  ok  rust-analyzer\n");
    }

    /// The latch: after the reader goes, nothing is written and nothing is retried. Without it
    /// every later line pays for another failed syscall, and a second failure would report itself.
    #[test]
    fn output_stops_for_good_once_the_reader_has_gone() {
        assert!(is_open(), "a fresh process starts out writable");
        OPEN.store(false, Ordering::Relaxed);
        assert!(!is_open());
        // Nothing to assert on stdout itself; what matters is that this is a no-op rather than a
        // panic or a second error report.
        write("this line goes nowhere\n");
        OPEN.store(true, Ordering::Relaxed);
    }
}
