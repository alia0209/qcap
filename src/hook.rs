//! `q hook`: a Claude Code PreToolUse hook for Bash. When a command caps a gate or an `&&` chain with
//! head/tail (see `pipeline`), it adds a hint pointing at q. Per session the full hint shows once,
//! then a one-line reminder, then nothing after `MAX_HINTS`. It never blocks: it always exits 0 and
//! never sets a permission decision.

use crate::{logpath, pipeline};
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

const MAX_HINTS: u32 = 10;
/// Longest command quoted back in a hint.
const QUOTE_MAX: usize = 100;

fn short(s: &str) -> String {
    if s.chars().count() <= QUOTE_MAX {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(QUOTE_MAX).collect::<String>())
    }
}

/// The hint for the first time in a session (`full`) or later ones.
pub fn text(hint: &pipeline::Hint, full: bool) -> String {
    let run = hint
        .suggestion
        .as_deref()
        .map_or_else(|| "q '<cmd>'".to_string(), short);
    if full {
        format!(
            "hint from q: piping `{}` through head/tail hides its exit code (an `&&` chain then tests tail's) and can cut the error line. Run `{run}` instead: it keeps the exit code, logs the full output to /tmp/q, and prints one header line plus the first error block and the last 15 lines (`q --grep PAT ...` for matching lines, `q show A-B` for more, `q -t 100s ...` to stop it before your tool timeout).",
            short(&hint.command)
        )
    } else {
        format!("q: `| head`/`| tail` hides this exit code; run `{run}`.")
    }
}

/// How many hints this session has had; bumps the count. No session id: always 0.
fn seen(session: Option<&str>) -> u32 {
    let Some(id) = session.filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    else {
        return 0;
    };
    let path: PathBuf = logpath::dir().join("hook").join(id);
    let n = fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if path.parent().is_some_and(|p| fs::create_dir_all(p).is_ok()) {
        let _ = fs::write(&path, (n + 1).to_string());
    }
    n
}

/// Returns the hook's stdout (empty when there is nothing to say).
pub fn respond(input: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(input) else {
        return String::new();
    };
    if v["tool_name"] != "Bash" {
        return String::new();
    }
    let Some(cmd) = v["tool_input"]["command"].as_str() else {
        return String::new();
    };
    let Some(found) = pipeline::explain(cmd) else {
        return String::new();
    };
    let hint = match seen(v["session_id"].as_str()) {
        0 => text(&found, true),
        n if n < MAX_HINTS => text(&found, false),
        _ => return String::new(),
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": hint,
        }
    })
    .to_string()
}

pub fn main() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_ok() {
        let out = respond(&input);
        if !out.is_empty() {
            println!("{out}");
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(cmd: &str, session: Option<&str>) -> String {
        json!({"tool_name": "Bash", "tool_input": {"command": cmd}, "session_id": session})
            .to_string()
    }

    fn context(out: &str) -> String {
        let v: Value = serde_json::from_str(out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(v["hookSpecificOutput"].get("permissionDecision").is_none());
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn respond_shapes() {
        let full = context(&respond(&bash("cargo test 2>&1 | tail", None)));
        assert!(full.starts_with("hint from q: piping `cargo test 2>&1` through head/tail"));
        assert!(full.contains("Run `q cargo test` instead"));
        assert_eq!(respond(&bash("ls", None)), "");
        assert_eq!(respond("not json"), "");
        assert_eq!(respond(r#"{"tool_name":"Edit","tool_input":{}}"#), "");
    }

    #[test]
    fn full_once_then_short_then_quiet() {
        let id = format!("test-{}", std::process::id());
        let _ = fs::remove_file(logpath::dir().join("hook").join(&id));
        let mut outs = Vec::new();
        for _ in 0..12 {
            outs.push(respond(&bash("cargo test | tail", Some(&id))));
        }
        assert!(context(&outs[0]).starts_with("hint from q:"));
        let short = "q: `| head`/`| tail` hides this exit code; run `q cargo test`.";
        assert_eq!(context(&outs[1]), short);
        assert_eq!(context(&outs[9]), short);
        assert_eq!(outs[10], "");
        let _ = fs::remove_file(logpath::dir().join("hook").join(&id));
    }
}
