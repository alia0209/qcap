//! Turns raw log bytes into display lines: ANSI escapes stripped, `\r` progress redraws collapsed,
//! very long lines cut.

use regex::Regex;
use std::sync::OnceLock;

const MAX_LINE: usize = 400;

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

pub fn lines(raw: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(raw);
    let mut out: Vec<String> = text.split('\n').map(clean).collect();
    if out.last().is_some_and(String::is_empty) {
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

fn clean(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    // A progress bar redraws with \r: only the last frame is what a terminal would show.
    let line = line.rsplit('\r').next().unwrap_or(line);
    let line = ansi().replace_all(line, "");
    cut(&line)
}

fn cut(line: &str) -> String {
    let count = line.chars().count();
    if count <= MAX_LINE {
        return line.to_string();
    }
    let head: String = line.chars().take(MAX_LINE).collect();
    format!("{head}…[+{} chars]", count - MAX_LINE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_and_cr() {
        let l = lines(b"\x1b[31merror\x1b[0m: x\r\n10%\r50%\r100%\nlast");
        assert_eq!(l, vec!["error: x", "100%", "last"]);
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
        let l = lines("x".repeat(1000).as_bytes());
        assert!(l[0].ends_with("…[+600 chars]"));
    }
}
