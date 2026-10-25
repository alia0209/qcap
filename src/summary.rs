//! Picks what to print from the log lines: everything if short, else the first error block
//! (failures only) and the tail, or only grep matches.

use crate::text::Line;
use regex::Regex;
use std::sync::OnceLock;

const CONTEXT: usize = 3;

fn error_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Word start on error/fail so names like `qfail` don't match; `TypeError` still does.
    RE.get_or_init(|| {
        Regex::new(r"(?i:\b(error|fail))|Error\b|refused|REFUSED|panic|Traceback|[✗✖✘]|^not ok\b")
            .unwrap()
    })
}

/// A passing test whose name happens to say "error" or "fails" (`test fails_on_x ... ok`,
/// `(pass) parser fails on bad input`, `✓ ...`, TAP `ok 3 ...`).
fn pass_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*(\(pass\)|[✓✔√]|PASS\b|ok \d)|\.\.\. ok$|\bok \(\d+(\.\d+)?m?s\)$")
            .unwrap()
    })
}

/// Counters like "0 failed" or "0 errors" are good news, not an error line.
fn zero_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b0 (errors?|fail(ed|ures?|s)?)\b").unwrap())
}

/// Lines that carry the actual failure (a panic, a compiler error, an assertion, a refusal), as
/// opposed to a bare `test x ... FAILED` roll call.
fn strong_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"panicked|\berror(\[E\d+\])?:|\berror TS\d+|\w+Error\b|Error:|Traceback|REFUSED|\brefused\b|\bassert|\(fail\)|[✗✖✘]|^not ok\b|^FAIL\b",
        )
        .unwrap()
    })
}

pub fn is_error(line: &str) -> bool {
    !pass_re().is_match(line) && error_re().is_match(&zero_re().replace_all(line, ""))
}

fn is_strong(line: &str) -> bool {
    is_error(line) && strong_re().is_match(line)
}

/// Which lines start error groups: the strong ones when there are any, else every error line.
fn matcher(lines: &[Line]) -> fn(&str) -> bool {
    if lines.iter().any(|l| is_strong(l)) {
        is_strong
    } else {
        is_error
    }
}

/// Line indices (0-based) of the first error block: grep -C3 groups from the first match on,
/// at most `budget` lines.
pub fn error_block(lines: &[Line], budget: usize) -> Vec<usize> {
    let hit = matcher(lines);
    let mut picked: Vec<usize> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !hit(l) {
            continue;
        }
        let from = i.saturating_sub(CONTEXT);
        let to = (i + CONTEXT).min(lines.len() - 1);
        for j in from..=to {
            if picked.len() == budget {
                return picked;
            }
            if picked.last().is_none_or(|&last| j > last) {
                picked.push(j);
            }
        }
    }
    picked
}

/// Error matches above the tail that didn't fit in the error block (0-based, inclusive).
#[derive(Debug, PartialEq)]
pub struct More {
    pub count: usize,
    pub from: usize,
    pub to: usize,
}

/// What to show of a log, as line indices (0-based). Rendered as text by `render`, as JSON by
/// `report`.
#[derive(Debug, PartialEq)]
pub struct Picked {
    /// The whole output fits: `tail_from` is 0 and there is no error block.
    pub complete: bool,
    pub first_error: Option<usize>,
    pub block: Vec<usize>,
    pub more: Option<More>,
    pub tail_from: usize,
}

pub fn pick(lines: &[Line], budget: usize, tail: usize, failed: bool) -> Picked {
    if lines.len() <= budget + tail {
        return Picked {
            complete: true,
            first_error: None,
            block: Vec::new(),
            more: None,
            tail_from: 0,
        };
    }
    // The tail is always printed whole, so the error block only looks above it: no line twice.
    let start = lines.len().saturating_sub(tail);
    let block = if failed {
        error_block(&lines[..start], budget)
    } else {
        Vec::new()
    };
    let hit = matcher(&lines[..start]);
    let first_error = block.iter().copied().find(|&i| hit(&lines[i]));
    let more = first_error.and_then(|_| {
        let after = block.last().map_or(0, |&l| l + 1);
        let rest: Vec<usize> = (after..start).filter(|&i| hit(&lines[i])).collect();
        Some(More {
            count: rest.len(),
            from: *rest.first()?,
            to: *rest.last()?,
        })
    });
    Picked {
        complete: false,
        first_error,
        block: if first_error.is_some() {
            block
        } else {
            Vec::new()
        },
        more,
        tail_from: start,
    }
}

pub fn render(lines: &[Line], p: &Picked) -> String {
    let mut out = String::new();
    if p.complete {
        for l in lines {
            out.push_str(l);
            out.push('\n');
        }
        return out;
    }
    if let Some(first) = p.first_error {
        out.push_str(&format!("-- first error: line {} --\n", first + 1));
        let mut prev: Option<usize> = None;
        for &i in &p.block {
            if prev.is_some_and(|p| i > p + 1) {
                out.push_str("…\n");
            }
            out.push_str(&format!("{}: {}\n", i + 1, lines[i]));
            prev = Some(i);
        }
    }
    if let Some(m) = &p.more {
        out.push_str(&format!(
            "-- {} more error lines before the tail: q show {}-{} --\n",
            m.count,
            m.from + 1,
            m.to + 1
        ));
    }
    if p.tail_from < lines.len() {
        out.push_str(&format!(
            "-- last lines: {}-{} --\n",
            p.tail_from + 1,
            lines.len()
        ));
        for l in &lines[p.tail_from..] {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
pub fn sections(lines: &[Line], budget: usize, tail: usize, failed: bool) -> String {
    render(lines, &pick(lines, budget, tail, failed))
}

/// Matching line indices, at most `budget`, plus the total count.
pub fn grep_hits(lines: &[Line], re: &Regex, budget: usize) -> (Vec<usize>, usize) {
    let all: Vec<usize> = (0..lines.len())
        .filter(|&i| re.is_match(&lines[i]))
        .collect();
    let total = all.len();
    (all.into_iter().take(budget).collect(), total)
}

pub fn grep(lines: &[Line], re: &Regex, budget: usize) -> String {
    let (hits, total) = grep_hits(lines, re, budget);
    let mut out = String::new();
    for &i in &hits {
        out.push_str(&format!("{}: {}\n", i + 1, lines[i]));
    }
    if total == 0 {
        out.push_str(&format!("(no lines match {})\n", re.as_str()));
    } else if total > hits.len() {
        out.push_str(&format!(
            "(+{} more matches in the log)\n",
            total - hits.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(n: usize) -> Vec<Line<'static>> {
        (1..=n).map(|i| format!("line {i}").into()).collect()
    }

    #[test]
    fn error_matching() {
        assert!(is_error("Error: boom"));
        assert!(is_error("test x ... FAILED"));
        assert!(is_error("REFUSED lost_edit (exit 3)"));
        assert!(is_error("thread 'main' panicked at"));
        assert!(is_error("Traceback (most recent call last):"));
        assert!(!is_error("test result: ok. 5 passed; 0 failed; 0 ignored"));
        assert!(is_error("3 passed; 1 failed"));
        assert!(!is_error("all good"));
        assert!(!is_error("test parse::fails_on_bad_input ... ok"));
        assert!(!is_error("(pass) parser fails on bad input [0.1ms]"));
        assert!(!is_error("  ✓ reports an error for bad flags"));
        assert!(!is_error("ok 3 - error path"));
        assert!(is_error("not ok 4 - error path"));
        assert!(is_error("(fail) adds [0.09ms]"));
        assert!(is_error("  ✗ adds"));
        assert!(is_error("test parse::fails_on_bad_input ... FAILED"));
        assert!(!is_error("   Compiling qfail v0.1.0"));
        assert!(is_error("TypeError: x is undefined"));
    }

    #[test]
    fn strong_errors_beat_a_failed_roll_call() {
        let mut l: Vec<Line> = (0..100)
            .map(|i| format!("test t{i} ... FAILED").into())
            .collect();
        l.push("thread 't0' panicked at src/lib.rs:4:5:".into());
        l.push("assertion `left == right` failed: case 0".into());
        l.extend((0..30).map(|i| format!("pad {i}").into()));
        let out = sections(&l, 20, 5, true);
        assert!(out.contains("-- first error: line 101 --"));
        assert!(out.contains("102: assertion `left == right` failed"));
        assert!(!out.contains("t50 ... FAILED"));
    }

    #[test]
    fn short_output_prints_in_full() {
        let l = numbered(10);
        assert_eq!(sections(&l, 40, 15, true).lines().count(), 10);
    }

    #[test]
    fn long_failure_has_error_block_and_tail() {
        let mut l = numbered(200);
        l[99] = "error: bad thing".into();
        let out = sections(&l, 40, 15, true);
        assert!(out.contains("-- first error: line 100 --\n97: line 97\n"));
        assert!(out.contains("100: error: bad thing"));
        assert!(out.contains("-- last lines: 186-200 --"));
        assert!(!out.contains("line 50\n"));
    }

    #[test]
    fn success_prints_tail_only() {
        let mut l = numbered(200);
        l[99] = "error: ignored on success".into();
        let out = sections(&l, 40, 15, false);
        assert!(!out.contains("first error"));
        assert_eq!(out.lines().count(), 16);
    }

    #[test]
    fn budget_caps_error_block() {
        let l: Vec<Line> = (0..500).map(|i| format!("error {i}").into()).collect();
        assert_eq!(error_block(&l, 40).len(), 40);
        let out = sections(&l, 10, 5, true);
        assert!(out.contains("-- 485 more error lines before the tail: q show 11-495 --"));
        assert_eq!(out.lines().count(), 1 + 10 + 1 + 1 + 5);
    }

    #[test]
    fn tail_is_deduplicated_against_error_block() {
        let mut l = numbered(100);
        l[97] = "panic here".into();
        let out = sections(&l, 40, 15, true);
        assert!(!out.contains("first error"));
        assert!(out.contains("-- last lines: 86-100 --"));
        assert_eq!(out.matches("panic here").count(), 1);
    }

    #[test]
    fn grep_numbers_and_caps() {
        let l = numbered(100);
        let re = Regex::new("line 1").unwrap();
        let out = grep(&l, &re, 3);
        assert!(out.starts_with("1: line 1\n10: line 10\n"));
        assert!(out.contains("(+9 more matches in the log)"));
        assert!(grep(&l, &Regex::new("zzz").unwrap(), 3).contains("no lines match"));
    }
}
