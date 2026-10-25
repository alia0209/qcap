//! Decides whether a Bash command caps a meaningful verdict with `| head`/`| tail`. Read-only
//! surveys (`rg x | head`, `git log | head`) are fine; gates (`cargo test | tail`) and `&&` chains
//! that carry on after a capped step are not.

use regex::Regex;
use std::sync::OnceLock;

/// First words of read-only commands: capping them loses nothing that matters.
const VIEWERS: &str =
    "awk basename bat cat column cut date df diff difft dirname du echo env fd file find git \
    grep head jq journalctl less ls nl printf ps readlink realpath rg sed sort stat tail tokei \
    tree uniq wc which yq";

fn gate_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(validate\S*|test|tests|check|typecheck|clippy|lint|build|tsc|pytest|make)$")
            .unwrap()
    })
}

fn stmt_sep_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"&&|\|\||[;\n]").unwrap())
}

fn heredoc_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<<-?\s*['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?"#).unwrap())
}

/// Masks heredoc bodies (spaces) and the contents of quoted strings (`_`), so data never looks like
/// a pipe. Byte offsets are kept, so a span of the result is the same span of the original.
pub fn code_only(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut heredoc: Option<String> = None;
    for line in command.split_inclusive('\n') {
        if let Some(delim) = &heredoc {
            if line.trim() == delim {
                heredoc = None;
            }
            for c in line.chars() {
                if c == '\n' {
                    out.push('\n');
                } else {
                    out.extend(std::iter::repeat_n(' ', c.len_utf8()));
                }
            }
            continue;
        }
        let start = out.len();
        for ch in line.chars() {
            match quote {
                Some(q) => {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' && q == '"' {
                        escaped = true;
                    } else if ch == q {
                        quote = None;
                        out.push(ch);
                        continue;
                    }
                    if ch == '\n' {
                        out.push('\n');
                    } else {
                        out.extend(std::iter::repeat_n('_', ch.len_utf8()));
                    }
                }
                None => {
                    if ch == '\'' || ch == '"' {
                        quote = Some(ch);
                    }
                    out.push(ch);
                }
            }
        }
        if quote.is_none() {
            if let Some(c) = heredoc_re().captures(line) {
                let at = c.get(0).map_or(0, |m| m.start());
                if out[start..].get(at..at + 2) == Some("<<") {
                    heredoc = Some(c[1].to_string());
                }
            }
        }
    }
    out
}

struct Stmt<'a> {
    /// Byte offset of `text` in the command.
    start: usize,
    text: &'a str,
    then_and: bool,
}

fn statements(code: &str) -> Vec<Stmt<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    for m in stmt_sep_re().find_iter(code) {
        out.push(Stmt {
            start,
            text: &code[start..m.start()],
            then_and: m.as_str() == "&&",
        });
        start = m.end();
    }
    out.push(Stmt {
        start,
        text: &code[start..],
        then_and: false,
    });
    out
}

/// Command words of a pipeline stage: `x=$(` prefixes, subshell parens, env assignments, shell
/// keywords and wrappers like `timeout 60` dropped, paths reduced to their basename.
fn words(stage: &str) -> Vec<&str> {
    stage
        .split_whitespace()
        .map(|w| w.rsplit("$(").next().unwrap_or(w))
        .map(|w| w.trim_matches(|c| c == '(' || c == ')' || c == '`'))
        .filter(|w| !w.is_empty())
        .skip_while(|w| {
            w.contains('=')
                || w.starts_with(|c: char| c.is_ascii_digit())
                || matches!(
                    *w,
                    "time"
                        | "timeout"
                        | "nice"
                        | "command"
                        | "exec"
                        | "do"
                        | "then"
                        | "else"
                        | "if"
                        | "while"
                        | "until"
                        | "!"
                        | "{"
                )
        })
        .map(|w| w.rsplit('/').next().unwrap_or(w))
        .collect()
}

fn is_viewer(words: &[&str]) -> bool {
    if words
        .iter()
        .any(|w| matches!(*w, "--help" | "-h" | "--version" | "-V"))
    {
        return true;
    }
    match words.first() {
        None => true,
        Some(w) => VIEWERS.split_whitespace().any(|v| v == *w),
    }
}

fn is_gate(words: &[&str]) -> bool {
    words.iter().take(3).any(|w| gate_word_re().is_match(w))
}

/// The first stage's command words and its byte length, when a later stage is head/tail.
fn capped(stmt: &str) -> Option<(Vec<&str>, usize)> {
    let mut stages = stmt.split('|');
    let first_stage = stages.next()?;
    let caps = stages.any(|s| matches!(s.split_whitespace().next(), Some("head" | "tail")));
    caps.then(|| (words(first_stage), first_stage.len()))
}

#[derive(Debug, PartialEq)]
pub struct Hint {
    pub why: &'static str,
    /// The capped command as written, e.g. `cargo test 2>&1`.
    pub command: String,
    /// The same command run through q, when it can be written safely.
    pub suggestion: Option<String>,
}

/// Why the command deserves a hint, or None.
pub fn explain(command: &str) -> Option<Hint> {
    let code = code_only(command);
    statements(&code).iter().find_map(|s| {
        let (first, len) = capped(s.text)?;
        if first.first() == Some(&"q") || is_viewer(&first) {
            return None;
        }
        let why = if is_gate(&first) {
            "capped gate"
        } else if s.then_and {
            "capped step in an && chain"
        } else {
            return None;
        };
        let stage = command[s.start..s.start + len].trim();
        // Keep the rest of the line (`cd x && ...`, `... && echo ok`): only the capped statement
        // changes, so the rewrite still runs where and when the original would.
        let before = command[..s.start].trim_end();
        let after = command[s.start + s.text.trim_end().len()..].trim_end();
        let suggestion = suggest(stage).map(|q| {
            let sep = if before.is_empty() { "" } else { " " };
            format!("{before}{sep}{q}{after}")
        });
        Some(Hint {
            why,
            command: stage.to_string(),
            suggestion,
        })
    })
}

/// `cargo test 2>&1` -> `q cargo test`; `FOO=1 bun test` -> `q 'FOO=1 bun test'`; None when the
/// stage is part of a bigger construct (subshell, loop, heredoc) or can't be quoted simply.
fn suggest(stage: &str) -> Option<String> {
    let mut s = stage.trim();
    // q merges stderr into the log itself, so these redirections add nothing.
    while let Some(rest) = ["2>&1", "2>/dev/null"]
        .iter()
        .find_map(|r| s.strip_suffix(r))
    {
        s = rest.trim_end();
    }
    let first = s.split_whitespace().next()?;
    if s.contains("$(")
        || s.contains('\n')
        || s.contains("<<")
        || s.starts_with('(')
        || matches!(
            first,
            "do" | "then" | "else" | "if" | "while" | "until" | "!" | "{"
        )
    {
        return None;
    }
    let plain = !first.contains('=') && !s.contains(|c: char| "<>$`;&*?~{}[]()\"'\\#!".contains(c));
    if plain {
        Some(format!("q {s}"))
    } else if !s.contains('\'') {
        Some(format!("q '{s}'"))
    } else {
        None
    }
}

#[cfg(test)]
fn should_hint(command: &str) -> bool {
    explain(command).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_on_capped_gates() {
        assert!(should_hint("cargo test 2>&1 | tail -20"));
        assert!(should_hint("bun run validate | head -40"));
        assert!(should_hint("cd x && make check 2>&1 | tail -5"));
        assert!(should_hint("./validate 2>&1 | rg -v ok | head"));
        assert!(should_hint("cd /x; cargo test --quiet 2>&1 | tail -3"));
        assert!(should_hint("FOO=1 timeout 60 bun test | tail"));
        assert!(should_hint("(cd x && make) | tail -3"));
    }

    #[test]
    fn hints_on_chains_after_a_capped_step() {
        assert!(should_hint("npm publish | tail -1 && git push"));
        assert!(should_hint("./deploy.sh | tail -1 && echo done"));
    }

    #[test]
    fn quiet_on_surveys() {
        assert!(!should_hint("ls | head"));
        assert!(!should_hint("rg -n foo src | head -20"));
        assert!(!should_hint("git log --oneline | head -5 && git status"));
        assert!(!should_hint("rg -n 'install|link' validate.sh | head"));
        assert!(!should_hint("git log --grep test | head"));
        assert!(!should_hint(
            "cat src/a.rs | head -30 && cat src/b.rs | head"
        ));
        assert!(!should_hint("ls test | head"));
        assert!(!should_hint("git status | head && git diff --stat"));
        assert!(!should_hint("q cargo test | tail"));
        assert!(!should_hint("bun run validate"));
        assert!(!should_hint("./script.sh | tail -3"));
        assert!(!should_hint(
            "E=$(rg -n x f | head -n1 | cut -d: -f1) && echo $E"
        ));
        assert!(!should_hint("bun test --help 2>&1 | head -30"));
        assert!(!should_hint("ps aux | head && df -h | tail -3"));
        assert!(should_hint("for f in a b; do tsc -p . $f | head -5; done"));
        assert!(should_hint("r=$(cargo test 2>&1 | tail -1)"));
    }

    fn suggestion(cmd: &str) -> Option<String> {
        explain(cmd).and_then(|h| h.suggestion)
    }

    #[test]
    fn suggests_the_q_form() {
        // Dropping the `cd x &&` sent agents that copied the rewrite to the wrong directory.
        assert_eq!(
            suggestion("cd x && cargo test --quiet 2>&1 | tail -3").as_deref(),
            Some("cd x && q cargo test --quiet")
        );
        assert_eq!(
            suggestion("FOO=1 bun test | tail").as_deref(),
            Some("q 'FOO=1 bun test'")
        );
        assert_eq!(
            suggestion("bun test -t \"my test\" | tail").as_deref(),
            Some("q 'bun test -t \"my test\"'")
        );
        assert_eq!(suggestion("bun test -t 'it' | tail"), None);
        assert_eq!(
            suggestion("npm publish 2>/dev/null | tail -1 && echo ok").as_deref(),
            Some("q npm publish && echo ok")
        );
        assert_eq!(suggestion("for f in a; do tsc $f | head; done"), None);
        let h = explain("echo é; bun test 2>&1 | tail -1").unwrap();
        assert_eq!(h.command, "bun test 2>&1");
    }

    #[test]
    fn masking_keeps_offsets() {
        let c = "echo 'é | x' <<EOF\nbody | tail\nEOF\nls";
        let m = code_only(c);
        assert_eq!(m.len(), c.len());
        assert!(!m.contains("| x") && !m.contains("body"));
        assert!(m.ends_with("   \nls"));
        // A quote spanning lines stays masked.
        assert!(!should_hint(
            "git commit -m \"line one\ncargo test | tail && x\""
        ));
    }

    #[test]
    fn data_is_not_code() {
        assert!(!should_hint("git commit -m 'cargo test | tail && x'"));
        assert!(!should_hint("echo \"make | head && y\""));
        let heredoc = "cat > /tmp/x <<'EOF'\ncargo test | tail -1 && next\nEOF\nls";
        assert!(!should_hint(heredoc));
        assert!(should_hint(
            "cat > /tmp/x <<EOF\nhi\nEOF\ncargo test | tail"
        ));
    }
}
