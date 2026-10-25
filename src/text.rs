//! Turns raw log bytes into display lines: ANSI escapes stripped, `\r` progress redraws collapsed,
//! very long lines cut. Lines borrow from the log where they can, so a huge log costs one
//! pointer-sized entry per line rather than a copy of its text.

use memmap2::Mmap;
use regex::Regex;
use std::borrow::Cow;
use std::fs::File;
use std::ops::Deref;
use std::path::Path;
use std::sync::OnceLock;

const MAX_LINE: usize = 400;

/// One display line; borrowed from the log unless cleaning had to change it.
pub type Line<'a> = Cow<'a, str>;

/// A log's bytes, memory-mapped. `/tmp` is often tmpfs, where the mapping shares the pages that
/// already hold the file instead of reading a second copy into the heap.
pub enum Bytes {
    Map(Mmap),
    Empty,
}

impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Map(m) => m,
            Bytes::Empty => &[],
        }
    }
}

/// Maps the log. Nothing truncates a log once q has created it, which is what makes the
/// mapping safe; a grandchild still appending past the mapped length is not seen.
pub fn read(path: &Path) -> std::io::Result<Bytes> {
    let file = File::open(path)?;
    if file.metadata()?.len() == 0 {
        return Ok(Bytes::Empty);
    }
    // SAFETY: see above; the file is only ever appended to.
    Ok(Bytes::Map(unsafe { Mmap::map(&file)? }))
}

/// A NUL byte, or mostly invalid UTF-8, in the first 64 KiB.
pub fn is_binary(raw: &[u8]) -> bool {
    let sample = &raw[..raw.len().min(64 * 1024)];
    if sample.contains(&0) {
        return true;
    }
    let bad = String::from_utf8_lossy(sample)
        .chars()
        .filter(|&c| c == char::REPLACEMENT_CHARACTER)
        .count();
    bad * 10 > sample.len()
}

pub fn lines(raw: &[u8]) -> Vec<Line<'_>> {
    let mut out: Vec<Line> = raw
        .split(|&b| b == b'\n')
        .map(|l| match String::from_utf8_lossy(l) {
            Cow::Borrowed(s) => clean(s),
            Cow::Owned(s) => Cow::Owned(clean(&s).into_owned()),
        })
        .collect();
    if out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

fn ansi() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[@-Z\\-_])").unwrap()
    })
}

fn clean(line: &str) -> Line<'_> {
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

fn cut(line: &str) -> Line<'_> {
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
        let l = lines(long.as_bytes());
        assert!(l[0].ends_with("…[+600 chars]"));
    }
}
