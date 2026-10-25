//! Turns raw log bytes into display lines: ANSI escapes stripped, `\r` progress redraws collapsed,
//! very long lines cut. Lines are read one at a time with a fixed memory ceiling, however big
//! the log or a single line in it.

use regex::Regex;
use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, Read};
use std::sync::OnceLock;

const MAX_LINE: usize = 400;
/// Bytes of an overlong line kept from its start, and from its end (for `\r` redraws).
const KEEP: usize = 64 * 1024;
/// Bytes sampled from the start of the log for binary detection.
pub const SAMPLE: usize = 64 * 1024;

/// A NUL byte, or mostly invalid UTF-8, in the sample.
pub fn is_binary(sample: &[u8]) -> bool {
    let sample = &sample[..sample.len().min(SAMPLE)];
    if sample.contains(&0) {
        return true;
    }
    let bad = String::from_utf8_lossy(sample)
        .chars()
        .filter(|&c| c == char::REPLACEMENT_CHARACTER)
        .count();
    bad * 10 > sample.len()
}

/// Cleaned display lines from a reader. A final line without `\n` still counts; a trailing
/// `\n` does not start an empty line.
pub struct Lines<R: Read> {
    r: BufReader<R>,
    /// The first KEEP bytes of the current line.
    head: Vec<u8>,
    /// The last bytes of an overlong line, at most 2 x KEEP.
    end: Vec<u8>,
    /// Chars dropped from the middle of an overlong line.
    dropped: usize,
    pub bytes: u64,
}

impl<R: Read> Lines<R> {
    pub fn new(r: R) -> Self {
        Lines {
            r: BufReader::with_capacity(64 * 1024, r),
            head: Vec::new(),
            end: Vec::new(),
            dropped: 0,
            bytes: 0,
        }
    }

    fn take(&mut self, chunk: &[u8]) {
        let room = KEEP.saturating_sub(self.head.len());
        let (h, rest) = chunk.split_at(room.min(chunk.len()));
        self.head.extend_from_slice(h);
        if rest.is_empty() {
            return;
        }
        self.end.extend_from_slice(rest);
        if self.end.len() > 2 * KEEP {
            let cut = self.end.len() - KEEP;
            self.dropped += chars(&self.end[..cut]);
            self.end.drain(..cut);
        }
    }

    fn finish_line(&mut self) -> String {
        let line = if self.end.is_empty() {
            clean(&String::from_utf8_lossy(&self.head)).into_owned()
        } else if self.end.contains(&b'\r') {
            // A progress bar redrawn with \r: the last frame is near the end.
            clean(&String::from_utf8_lossy(&self.end)).into_owned()
        } else {
            let extra = self.dropped + chars(&self.end);
            let head = String::from_utf8_lossy(&self.head);
            let shown = ansi().replace_all(&head, "");
            let count = shown.chars().count() + extra;
            let kept: String = shown.chars().take(MAX_LINE).collect();
            format!("{kept}…[+{} chars]", count - kept.chars().count())
        };
        self.head.clear();
        self.end.clear();
        self.dropped = 0;
        line
    }
}

impl<R: Read> Iterator for Lines<R> {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<io::Result<String>> {
        let mut any = false;
        loop {
            let buf = match self.r.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Some(Err(e)),
            };
            if buf.is_empty() {
                return any.then(|| Ok(self.finish_line()));
            }
            any = true;
            let (chunk, used, done) = match buf.iter().position(|&b| b == b'\n') {
                Some(p) => (buf[..p].to_vec(), p + 1, true),
                None => (buf.to_vec(), buf.len(), false),
            };
            self.r.consume(used);
            self.bytes += used as u64;
            self.take(&chunk);
            if done {
                return Some(Ok(self.finish_line()));
            }
        }
    }
}

/// UTF-8 chars in a byte run (continuation bytes don't start one).
fn chars(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c & 0xC0 != 0x80).count()
}

fn ansi() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[@-Z\\-_])").unwrap()
    })
}

pub fn clean(line: &str) -> Cow<'_, str> {
    // ANSI first: in "msg\r\x1b[0m" the reset comes after the \r, and splitting on \r first
    // would keep only the reset and blank the line.
    match ansi().replace_all(line, "") {
        Cow::Borrowed(s) => cut(last_frame(s)),
        Cow::Owned(s) => Cow::Owned(cut(last_frame(&s)).into_owned()),
    }
}

/// A progress bar redraws with \r: only the last frame is what a terminal would show.
fn last_frame(line: &str) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.rsplit('\r').next().unwrap_or(line)
}

fn cut(line: &str) -> Cow<'_, str> {
    // Byte length bounds the char count, so most lines skip counting.
    if line.len() <= MAX_LINE {
        return Cow::Borrowed(line);
    }
    let count = line.chars().count();
    if count <= MAX_LINE {
        return Cow::Borrowed(line);
    }
    let head: String = line.chars().take(MAX_LINE).collect();
    Cow::Owned(format!("{head}…[+{} chars]", count - MAX_LINE))
}

#[cfg(test)]
pub fn lines(raw: &[u8]) -> Vec<String> {
    Lines::new(raw).map(Result::unwrap).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_and_cr() {
        let l = lines(b"\x1b[31merror\x1b[0m: x\r\n10%\r50%\r100%\nlast");
        assert_eq!(l, vec!["error: x", "100%", "last"]);
        // A reset after a trailing \r used to blank the whole line.
        assert_eq!(lines(b"error: boom\r\x1b[0m\n"), vec!["error: boom"]);
    }

    #[test]
    fn trailing_newline_is_not_a_line() {
        assert_eq!(lines(b"a\nb\n").len(), 2);
        assert_eq!(lines(b"a\n\n").len(), 2);
        assert!(lines(b"").is_empty());
    }

    #[test]
    fn binary_detection() {
        assert!(is_binary(b"abc\0def"));
        assert!(is_binary(&[0xff; 100]));
        assert!(!is_binary("plain ✓ text\n".as_bytes()));
        assert!(!is_binary(b""));
    }

    #[test]
    fn long_lines_are_cut() {
        let long = "x".repeat(1000);
        assert!(lines(long.as_bytes())[0].ends_with("…[+600 chars]"));
        // Past the per-line ceiling: the count stays exact, a \r redraw keeps its last frame.
        let huge = "y".repeat(300_000);
        assert!(lines(huge.as_bytes())[0].ends_with("…[+299600 chars]"));
        let bar = format!("{}\rdone 100%\n", "10%\r".repeat(100_000));
        assert_eq!(lines(bar.as_bytes()), vec!["done 100%"]);
    }
}
