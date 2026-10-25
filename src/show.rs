//! `q last` and `q show`: get back to the newest log without hunting for its path.

use crate::{logpath, text};
use std::path::PathBuf;

const DEFAULT_SPAN: usize = 40;
/// Most lines one `q show` prints; the footer says how to continue.
const MAX_SPAN: usize = 500;

pub fn last() -> Result<String, String> {
    logpath::newest(&logpath::dir())
        .map(|p| p.display().to_string())
        .ok_or_else(|| format!("no logs in {}", logpath::dir().display()))
}

/// 1-based inclusive line range for a log of `total` lines: `A-B`, `A` (40 lines from A), `A-`
/// (A to the end), `-N` (the last N), none (the first 40).
fn range(spec: Option<&str>, total: usize) -> Result<(usize, usize), String> {
    let bad = || format!("range wants A, A-B, A- or -N (1-based line numbers), got {spec:?}");
    let num = |s: &str| s.parse::<usize>().map_err(|_| bad());
    let (a, b) = match spec {
        None => (1, DEFAULT_SPAN),
        Some(s) => match s.split_once('-') {
            Some(("", n)) => {
                let n = num(n)?;
                (total.saturating_sub(n) + 1, total.max(1))
            }
            Some((a, "")) => (num(a)?, usize::MAX),
            Some((a, b)) => (num(a)?, num(b)?),
            None => {
                let a = num(s)?;
                (a, a + DEFAULT_SPAN - 1)
            }
        },
    };
    if a == 0 || b < a {
        return Err(bad());
    }
    Ok((a, b))
}

fn is_log_arg(s: &str) -> bool {
    s.contains('/') || s.ends_with(".log")
}

/// `q show [RANGE] [LOG]`, either order.
pub fn show(first: Option<&str>, second: Option<&str>) -> Result<String, String> {
    let (spec, log) = match (first, second) {
        (Some(f), s) if is_log_arg(f) => (s, Some(f)),
        (f, s) => (f, s),
    };
    let path = match log {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(last()?),
    };
    let raw = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lines = text::lines(&raw);
    let (a, b) = range(spec, lines.len())?;
    let b = b.min(lines.len()).min(a + MAX_SPAN - 1);
    let mut out = format!(
        "-- {} lines {a}-{b} of {} --\n",
        path.display(),
        lines.len()
    );
    for (i, l) in lines.iter().enumerate().take(b).skip(a - 1) {
        out.push_str(&format!("{}: {l}\n", i + 1));
    }
    if b < lines.len() && spec.is_some_and(|s| s.ends_with('-')) {
        out.push_str(&format!(
            "-- more: q show {}- {} --\n",
            b + 1,
            path.display()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(range(None, 1000).unwrap(), (1, 40));
        assert_eq!(range(Some("100"), 1000).unwrap(), (100, 139));
        assert_eq!(range(Some("5-9"), 1000).unwrap(), (5, 9));
        assert_eq!(range(Some("-50"), 1000).unwrap(), (951, 1000));
        assert_eq!(range(Some("-50"), 10).unwrap(), (1, 10));
        assert_eq!(range(Some("990-"), 1000).unwrap(), (990, usize::MAX));
        assert!(range(Some("9-5"), 1000).is_err());
        assert!(range(Some("0"), 1000).is_err());
        assert!(range(Some("x"), 1000).is_err());
        assert!(range(Some("-x"), 1000).is_err());
    }
}
