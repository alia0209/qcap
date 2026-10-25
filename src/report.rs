//! The result of one run, printed as the text summary (header line + sections) or as one JSON
//! object (`--json`). Both come from the same `summary::Summary`, so they always agree.

use crate::run::{Outcome, Stop};
use crate::signals;
use crate::summary::{self, Numbered, Summary};
use regex::Regex;
use serde_json::{json, Value};
use std::time::Duration;

pub struct Report<'a> {
    pub cmd: &'a [String],
    pub outcome: &'a Outcome,
    pub log: &'a str,
    pub summary: &'a Summary,
    pub grep: Option<&'a Regex>,
}

impl Report<'_> {
    fn reason(&self) -> String {
        match (self.outcome.stopped, self.outcome.signal) {
            (Some(Stop::Timeout(t)), _) => format!(" (timeout after {})", duration(t)),
            (Some(Stop::Oversize(n)), _) => {
                format!(" (output over {}, command stopped)", size(n))
            }
            (Some(Stop::Signal(s)), _) => {
                format!(" (q got {}, command stopped)", signals::name(s))
            }
            (None, Some(s)) => format!(" (killed by {})", signals::name(s)),
            (None, None) => String::new(),
        }
    }

    /// `with_log` is false when the whole output is printed: the log holds nothing more, so
    /// the path is noise (`q last` still finds it).
    fn header(&self, size: &str, with_log: bool) -> String {
        let log = if with_log {
            format!(" · log {}", self.log)
        } else {
            String::new()
        };
        format!(
            "q: exit {}{} · {} · {size}{log}\n",
            self.outcome.code,
            self.reason(),
            duration(self.outcome.elapsed),
        )
    }

    pub fn text(&self) -> String {
        let s = self.summary;
        if s.binary {
            return self.header(
                &format!("{} bytes of binary output (not shown)", s.bytes),
                true,
            );
        }
        let (body, whole) = match (self.grep, &s.grep) {
            (Some(re), Some(g)) => (summary::render_grep(g, re.as_str()), false),
            _ => (summary::render(s), s.complete),
        };
        let n = s.lines;
        self.header(
            &format!("{n} line{}", if n == 1 { "" } else { "s" }),
            !whole,
        ) + &body
    }

    pub fn json(&self) -> String {
        let o = self.outcome;
        let stopped = match o.stopped {
            Some(Stop::Timeout(t)) => {
                json!({"reason": "timeout", "after_seconds": round(t.as_secs_f64())})
            }
            Some(Stop::Signal(s)) => json!({"reason": "signal", "signal": signals::name(s)}),
            Some(Stop::Oversize(n)) => json!({"reason": "output_cap", "max_bytes": n}),
            None => Value::Null,
        };
        let mut v = json!({
            "exit": o.code,
            "seconds": round(o.elapsed.as_secs_f64()),
            "log": self.log,
            "command": self.cmd,
            "bytes": self.summary.bytes,
            "stopped": stopped,
            "killed_by": if o.stopped.is_none() { o.signal.map(signals::name) } else { None },
        });
        let s = self.summary;
        v["binary"] = json!(s.binary);
        if s.binary {
            v["lines"] = Value::Null;
            v["complete"] = json!(false);
            v["first_error"] = Value::Null;
            v["error_block"] = json!([]);
            v["more_errors"] = Value::Null;
            v["tail"] = json!([]);
            v["grep"] = Value::Null;
            return v.to_string();
        }
        let entries = |l: &[Numbered]| {
            Value::Array(
                l.iter()
                    .map(|(i, t)| json!({"line": i + 1, "text": t}))
                    .collect(),
            )
        };
        v["lines"] = json!(s.lines);
        v["complete"] = json!(s.complete);
        v["first_error"] = json!(s.first_error.map(|i| i + 1));
        v["error_block"] = entries(&s.block);
        v["more_errors"] = match &s.more {
            Some(m) => json!({"count": m.count, "from": m.from + 1, "to": m.to + 1}),
            None => Value::Null,
        };
        v["tail"] = entries(&s.tail);
        v["grep"] = match (self.grep, &s.grep) {
            (Some(re), Some(g)) => json!({
                "pattern": re.as_str(),
                "total": g.total,
                "matches": entries(&g.hits),
            }),
            _ => Value::Null,
        };
        v.to_string()
    }
}

fn size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

fn round(s: f64) -> f64 {
    (s * 1000.0).round() / 1000.0
}

pub fn duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1}s")
    } else {
        format!("{}m{:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(code: i32, stopped: Option<Stop>) -> Outcome {
        Outcome {
            code,
            signal: None,
            elapsed: Duration::from_millis(1234),
            stopped,
        }
    }

    fn scan(o: &Outcome, raw: &[u8]) -> Summary {
        let opts = summary::Opts {
            budget: 40,
            tail: 15,
            failed: o.code != 0,
            grep: None,
        };
        summary::summarize(raw, &opts).unwrap()
    }

    fn json(o: &Outcome, raw: &[u8], cmd: &[String]) -> Value {
        let s = scan(o, raw);
        let r = Report {
            cmd,
            outcome: o,
            log: "/tmp/q/x-1.log",
            summary: &s,
            grep: None,
        };
        serde_json::from_str(&r.json()).unwrap()
    }

    #[test]
    fn json_shape_for_a_failure() {
        let mut raw = String::new();
        for i in 1..=200 {
            raw.push_str(&if i == 100 {
                "error: boom".into()
            } else {
                format!("l{i}")
            });
            raw.push('\n');
        }
        let cmd = vec!["make".to_string()];
        let o = outcome(2, None);
        let v = json(&o, raw.as_bytes(), &cmd);
        assert_eq!(v["exit"], 2);
        assert_eq!(v["seconds"], 1.234);
        assert_eq!(v["lines"], 200);
        assert_eq!(v["complete"], false);
        assert_eq!(v["first_error"], 100);
        assert_eq!(
            v["error_block"][3],
            json!({"line": 100, "text": "error: boom"})
        );
        assert_eq!(v["tail"].as_array().unwrap().len(), 15);
        assert_eq!(v["tail"][14]["line"], 200);
        assert!(v["stopped"].is_null() && v["grep"].is_null() && v["killed_by"].is_null());
        assert_eq!(v["command"], json!(["make"]));
    }

    #[test]
    fn json_short_and_stopped() {
        let cmd = vec!["x".to_string()];
        let o = outcome(124, Some(Stop::Timeout(Duration::from_secs(30))));
        let v = json(&o, b"a\nb\n", &cmd);
        assert_eq!(v["complete"], true);
        assert_eq!(
            v["tail"],
            json!([{"line": 1, "text": "a"}, {"line": 2, "text": "b"}])
        );
        assert_eq!(
            v["stopped"],
            json!({"reason": "timeout", "after_seconds": 30.0})
        );
        let v = json(&o, b"\0\x01", &cmd);
        assert_eq!(v["binary"], true);
        assert!(v["lines"].is_null());
    }

    #[test]
    fn text_matches_old_header() {
        let cmd = vec!["x".to_string()];
        let o = outcome(0, None);
        let s = scan(&o, b"hi\n");
        let r = Report {
            cmd: &cmd,
            outcome: &o,
            log: "/tmp/q/x-1.log",
            summary: &s,
            grep: None,
        };
        assert_eq!(r.text(), "q: exit 0 · 1.2s · 1 line\nhi\n");
    }
}
