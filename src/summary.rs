//! Picks what to print from a log in one streaming pass: everything if short, else the first
//! error block (failures only) and the tail, or only grep matches. Memory stays bounded by the
//! line budget, however long the log.

use crate::text::{self, Lines};
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::io::{self, Cursor, Read};
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

/// Whether a line is an error line, and whether it is a strong one. Worked out once per line
/// and shared by both finders.
#[derive(Clone, Copy)]
struct Kind {
    error: bool,
    strong: bool,
}

fn kind(line: &str) -> Kind {
    let error = is_error(line);
    Kind {
        error,
        strong: error && strong_re().is_match(line),
    }
}

/// A 0-based line index and its display text.
pub type Numbered = (usize, String);

/// Error matches above the tail that the error block doesn't show (0-based, inclusive).
#[derive(Debug, PartialEq)]
pub struct More {
    pub count: usize,
    pub from: usize,
    pub to: usize,
}

pub struct Grep {
    pub hits: Vec<Numbered>,
    pub total: usize,
}

pub struct Opts<'a> {
    pub budget: usize,
    pub tail: usize,
    pub failed: bool,
    pub grep: Option<&'a Regex>,
}

/// What to show of a log. Rendered as text by `render`, as JSON by `report`.
pub struct Summary {
    pub bytes: u64,
    pub binary: bool,
    pub lines: usize,
    /// The whole output fits: `tail` holds every line and there is no error block.
    pub complete: bool,
    pub first_error: Option<usize>,
    pub block: Vec<Numbered>,
    pub more: Option<More>,
    pub tail: Vec<Numbered>,
    pub grep: Option<Grep>,
}

/// An error match and the context around it, merged with any matches inside that context.
struct Group {
    trigger: usize,
    key: String,
    lines: Vec<(usize, String, bool)>,
}

/// Collects error groups for one notion of "error line" (strong or any). Holds at most `cap`
/// lines; the final cut to the budget happens in `finish`, once the tail is known.
struct Finder {
    strong: bool,
    groups: Vec<Group>,
    stored: usize,
    cap: usize,
    full: bool,
    after: usize,
    last_added: Option<usize>,
    /// Trigger texts already in a group: a repeat of the same failure line starts no new group.
    keys: HashSet<String>,
    matches: usize,
    /// The newest match indices, enough to find the last one above the tail.
    recent: VecDeque<usize>,
    recent_cap: usize,
    /// Matches that start no group (repeats, or past `cap`): the first, and the newest few.
    skipped_first: Option<usize>,
    skipped: VecDeque<usize>,
}

fn key(line: &str) -> String {
    line.trim().to_string()
}

impl Finder {
    fn new(strong: bool, cap: usize, tail: usize) -> Self {
        Finder {
            strong,
            groups: Vec::new(),
            stored: 0,
            cap,
            full: false,
            after: 0,
            last_added: None,
            keys: HashSet::new(),
            matches: 0,
            recent: VecDeque::new(),
            recent_cap: tail + 1,
            skipped_first: None,
            skipped: VecDeque::new(),
        }
    }

    fn skip(&mut self, i: usize) {
        self.skipped_first.get_or_insert(i);
        self.skipped.push_back(i);
        if self.skipped.len() > self.recent_cap {
            self.skipped.pop_front();
        }
    }

    fn add(&mut self, i: usize, line: &str, m: bool) {
        if let Some(g) = self.groups.last_mut() {
            g.lines.push((i, line.to_string(), m));
        }
        self.stored += 1;
        self.last_added = Some(i);
        if self.stored >= self.cap {
            self.full = true;
        }
    }

    fn hit(&self, k: Kind) -> bool {
        if self.strong {
            k.strong
        } else {
            k.error
        }
    }

    fn push(&mut self, i: usize, line: &str, k: Kind, before: &VecDeque<(usize, String, Kind)>) {
        let m = self.hit(k);
        if m {
            self.matches += 1;
            self.recent.push_back(i);
            if self.recent.len() > self.recent_cap {
                self.recent.pop_front();
            }
        }
        if self.full {
            if m {
                self.skip(i);
            }
            return;
        }
        if self.after > 0 {
            self.add(i, line, m);
            if m {
                self.keys.insert(key(line));
                self.after = CONTEXT;
            } else {
                self.after -= 1;
            }
            return;
        }
        if !m {
            return;
        }
        let k = key(line);
        if !self.keys.insert(k.clone()) {
            self.skip(i);
            return;
        }
        self.groups.push(Group {
            trigger: i,
            key: k,
            lines: Vec::new(),
        });
        for (j, l, jk) in before {
            if self.last_added.is_none_or(|last| *j > last) && !self.full {
                let jm = self.hit(*jk);
                self.add(*j, l, jm);
            }
        }
        self.add(i, line, true);
        self.after = CONTEXT;
    }

    fn matches_above(&self, start: usize) -> usize {
        self.matches - self.recent.iter().filter(|&&j| j >= start).count()
    }

    /// The error block from the groups above the tail (`start` on), at most `budget` lines.
    /// After the first group, groups whose failure line the tail repeats are left out: the tail
    /// shows it anyway.
    fn finish(
        self,
        start: usize,
        tail_keys: &HashSet<String>,
        budget: usize,
    ) -> (Option<usize>, Vec<Numbered>, Option<More>) {
        let above = self.matches_above(start);
        // First and last error match above the tail that the block leaves out.
        let mut unshown: Option<(usize, usize)> = None;
        let mut note = |j: usize| {
            unshown = Some(unshown.map_or((j, j), |(a, b)| (a.min(j), b.max(j))));
        };
        if let Some(f) = self.skipped_first.filter(|&f| f < start) {
            note(f);
        }
        if let Some(&l) = self.skipped.iter().rev().find(|&&j| j < start) {
            note(l);
        }
        let mut block = Vec::new();
        let mut first = None;
        let mut shown = 0;
        for g in self.groups {
            if g.trigger >= start {
                break;
            }
            // The first failure always shows: it is where the agent starts reading.
            if tail_keys.contains(&g.key) && !block.is_empty() {
                note(g.trigger);
                continue;
            }
            for (j, l, m) in g.lines {
                if j >= start {
                    break;
                }
                if block.len() == budget {
                    if m {
                        note(j);
                    }
                    continue;
                }
                if m {
                    shown += 1;
                    first.get_or_insert(j);
                }
                block.push((j, l));
            }
        }
        let first = first.filter(|_| !block.is_empty());
        let more = match unshown {
            Some((from, to)) if first.is_some() && above > shown => Some(More {
                count: above - shown,
                from,
                to,
            }),
            _ => None,
        };
        (first, block, more)
    }
}

pub fn summarize<R: Read>(mut r: R, o: &Opts) -> io::Result<Summary> {
    let mut sample = Vec::new();
    (&mut r)
        .take(text::SAMPLE as u64)
        .read_to_end(&mut sample)?;
    if text::is_binary(&sample) {
        let rest = io::copy(&mut r, &mut io::sink())?;
        return Ok(Summary {
            bytes: sample.len() as u64 + rest,
            binary: true,
            lines: 0,
            complete: false,
            first_error: None,
            block: Vec::new(),
            more: None,
            tail: Vec::new(),
            grep: None,
        });
    }
    let fits = o.budget + o.tail;
    let cap = 2 * o.budget + o.tail + 2 * CONTEXT + 1;
    let mut head: Option<Vec<Numbered>> = Some(Vec::new());
    let mut tail: VecDeque<Numbered> = VecDeque::new();
    let mut before: VecDeque<(usize, String, Kind)> = VecDeque::new();
    let mut finders = o.failed.then(|| {
        (
            Finder::new(true, cap, o.tail),
            Finder::new(false, cap, o.tail),
        )
    });
    let mut grep = o.grep.map(|_| Grep {
        hits: Vec::new(),
        total: 0,
    });
    let mut lines = Lines::new(Cursor::new(sample).chain(r));
    let mut n = 0;
    for line in lines.by_ref() {
        let line = line?;
        let k = if finders.is_some() {
            kind(&line)
        } else {
            Kind {
                error: false,
                strong: false,
            }
        };
        if let Some((s, w)) = finders.as_mut() {
            s.push(n, &line, k, &before);
            w.push(n, &line, k, &before);
        }
        if let (Some(g), Some(re)) = (grep.as_mut(), o.grep) {
            if re.is_match(&line) {
                g.total += 1;
                if g.hits.len() < o.budget {
                    g.hits.push((n, line.clone()));
                }
            }
        }
        if let Some(h) = head.as_mut() {
            if h.len() < fits {
                h.push((n, line.clone()));
            } else {
                head = None;
            }
        }
        before.push_back((n, line.clone(), k));
        if before.len() > CONTEXT {
            before.pop_front();
        }
        tail.push_back((n, line));
        if tail.len() > o.tail {
            tail.pop_front();
        }
        n += 1;
    }
    let mut s = Summary {
        bytes: lines.bytes,
        binary: false,
        lines: n,
        complete: false,
        first_error: None,
        block: Vec::new(),
        more: None,
        tail: tail.into(),
        grep,
    };
    if let Some(all) = head {
        s.complete = true;
        s.tail = all;
        return Ok(s);
    }
    // The tail is always printed whole, so the error block only looks above it: no line twice.
    let start = n - s.tail.len();
    if let Some((strong, weak)) = finders {
        let keys: HashSet<String> = s.tail.iter().map(|(_, l)| key(l)).collect();
        let f = if strong.matches_above(start) > 0 {
            strong
        } else {
            weak
        };
        (s.first_error, s.block, s.more) = f.finish(start, &keys, o.budget);
    }
    Ok(s)
}

pub fn render(s: &Summary) -> String {
    let mut out = String::new();
    if s.complete {
        for (_, l) in &s.tail {
            out.push_str(l);
            out.push('\n');
        }
        return out;
    }
    if let Some(first) = s.first_error {
        out.push_str(&format!("-- first error: line {} --\n", first + 1));
        let mut prev: Option<usize> = None;
        for (i, l) in &s.block {
            if prev.is_some_and(|p| *i > p + 1) {
                out.push_str("…\n");
            }
            out.push_str(&format!("{}: {l}\n", i + 1));
            prev = Some(*i);
        }
    }
    if let Some(m) = &s.more {
        out.push_str(&format!(
            "-- {} more error lines before the tail: q show {}-{} --\n",
            m.count,
            m.from + 1,
            m.to + 1
        ));
    }
    if let Some((first, _)) = s.tail.first() {
        out.push_str(&format!("-- last lines: {}-{} --\n", first + 1, s.lines));
        for (_, l) in &s.tail {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

pub fn render_grep(g: &Grep, pattern: &str) -> String {
    let mut out = String::new();
    for (i, l) in &g.hits {
        out.push_str(&format!("{}: {l}\n", i + 1));
    }
    if g.total == 0 {
        out.push_str(&format!("(no lines match {pattern})\n"));
    } else if g.total > g.hits.len() {
        out.push_str(&format!(
            "(+{} more matches in the log)\n",
            g.total - g.hits.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    fn scan(lines: &[String], budget: usize, tail: usize, failed: bool) -> Summary {
        let raw = lines.join("\n");
        let o = Opts {
            budget,
            tail,
            failed,
            grep: None,
        };
        summarize(raw.as_bytes(), &o).unwrap()
    }

    fn sections(lines: &[String], budget: usize, tail: usize, failed: bool) -> String {
        render(&scan(lines, budget, tail, failed))
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
        let mut l: Vec<String> = (0..100).map(|i| format!("test t{i} ... FAILED")).collect();
        l.push("thread 't0' panicked at src/lib.rs:4:5:".into());
        l.push("assertion `left == right` failed: case 0".into());
        l.extend((0..30).map(|i| format!("pad {i}")));
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
        let l: Vec<String> = (0..500).map(|i| format!("error {i}")).collect();
        assert_eq!(scan(&l, 40, 15, true).block.len(), 40);
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

    /// pytest and vitest repeat one assertion message per failing test, and again in their
    /// closing summary: the benchmark's flask run printed it three times.
    #[test]
    fn a_repeated_failure_line_is_shown_once() {
        let mut l = numbered(300);
        for at in [50, 100, 150, 290] {
            l[at] = "E   AssertionError: assert 1 == 2".into();
        }
        l[200] = "E   KeyError: 'x'".into();
        let s = scan(&l, 40, 15, true);
        let out = render(&s);
        // Once in the block (the first failure always shows), once in the tail, never again.
        assert_eq!(out.matches("AssertionError").count(), 2);
        assert_eq!(s.first_error, Some(50));
        assert!(out.contains("201: E   KeyError: 'x'"));
        assert_eq!(
            s.more,
            Some(More {
                count: 2,
                from: 100,
                to: 150
            })
        );
    }

    #[test]
    fn grep_numbers_and_caps() {
        let raw = numbered(100).join("\n");
        let re = Regex::new("line 1").unwrap();
        let o = Opts {
            budget: 3,
            tail: 15,
            failed: false,
            grep: Some(&re),
        };
        let g = summarize(raw.as_bytes(), &o).unwrap().grep.unwrap();
        let out = render_grep(&g, re.as_str());
        assert!(out.starts_with("1: line 1\n10: line 10\n"));
        assert!(out.contains("(+9 more matches in the log)"));
        let none = Grep {
            hits: Vec::new(),
            total: 0,
        };
        assert!(render_grep(&none, "zzz").contains("no lines match"));
    }
}
