//! Command-line parsing. Hand-rolled: q's own flags come first, everything after is the command.

use std::time::Duration;

pub const USAGE: &str = "\
q: run a command, keep its exit code, log the full output, print a capped summary

usage:
  q [-n N] [--tail N] [--grep PAT] [-t DUR] [--json] [--] <cmd...>
                         run cmd (a single argument with spaces or shell syntax runs through bash -c)
  q last                 print the newest log path
  q show [RANGE] [LOG]   numbered lines of the newest log from this directory (or LOG); RANGE is
                         A-B, A (40 lines), A- (to the end), -N (last N); at most 500 lines
  q hook                 Claude Code PreToolUse hook: hint at q when a Bash command pipes a gate
                         or an && chain through head/tail (reads hook JSON on stdin, never blocks)
  q hook --check CMD     say whether CMD would get the hint, and why (exit 0 = hint, 1 = quiet)

options:
  -n N         error-block budget in lines (default 40)
  --tail N     trailing lines to print (default 15)
  --grep PAT   print only lines matching the regex PAT, numbered (at most N of them)
  -t, --timeout DUR
               stop the command after DUR (90, 90s, 2m, 500ms): SIGTERM, SIGKILL 3s later;
               exit 124. Set it below the harness's own timeout to always get a summary.
  --json       print one JSON object instead of the text summary: exit, seconds, log, command,
               bytes, lines, binary, stopped, killed_by, complete, first_error, error_block,
               more_errors, tail, grep (line entries are {line, text}, 1-based)
  -h, --help   this help

output: one header line `q: exit 1 · 3.2s · 1840 lines · log /tmp/q/bun-test-3.log`, then the
first error block and the last lines. Short output prints whole, under a header without the log
path (`q last` has it). Exit code = the command's
(128+signal if killed). Runs over 10s first print `q: still running · log <path>`. If q itself
gets SIGTERM/SIGINT/SIGHUP it stops the command, prints the summary, exits 128+signal.
Logs live in /tmp/q. To run a command named last/show/hook: q -- last.";

#[derive(Debug, PartialEq)]
pub struct RunOpts {
    pub budget: usize,
    pub tail: usize,
    pub grep: Option<String>,
    pub timeout: Option<Duration>,
    pub json: bool,
    pub cmd: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Help,
    Last,
    Show {
        range: Option<String>,
        log: Option<String>,
    },
    Hook {
        check: Option<String>,
    },
    Run(RunOpts),
}

pub fn parse(args: &[String]) -> Result<Cmd, String> {
    match args.first().map(String::as_str) {
        None => return Err("no command given (try q --help)".into()),
        Some("-h" | "--help" | "help") => return Ok(Cmd::Help),
        Some("last") => return Ok(Cmd::Last),
        Some("hook") => {
            return match (args.get(1).map(String::as_str), args.get(2)) {
                (None, _) => Ok(Cmd::Hook { check: None }),
                (Some("--check"), Some(c)) => Ok(Cmd::Hook {
                    check: Some(c.clone()),
                }),
                _ => Err("usage: q hook [--check CMD]".into()),
            }
        }
        Some("show") => {
            return Ok(Cmd::Show {
                range: args.get(1).cloned(),
                log: args.get(2).cloned(),
            })
        }
        _ => {}
    }
    let mut opts = RunOpts {
        budget: 40,
        tail: 15,
        grep: None,
        timeout: None,
        json: false,
        cmd: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--" => {
                i += 1;
                break;
            }
            "-n" | "--tail" | "--grep" | "-t" | "--timeout" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{a} needs a value"))?;
                match a {
                    "-n" => opts.budget = number(a, v)?,
                    "--tail" => opts.tail = number(a, v)?,
                    "-t" | "--timeout" => opts.timeout = Some(duration(a, v)?),
                    _ => opts.grep = Some(v.clone()),
                }
                i += 2;
            }
            "-h" | "--help" => return Ok(Cmd::Help),
            "--json" => {
                opts.json = true;
                i += 1;
            }
            _ if a.starts_with('-') && a.len() > 1 => {
                return Err(format!(
                    "unknown option {a} (use -- before a command that starts with -)"
                ))
            }
            _ => break,
        }
    }
    opts.cmd = args[i..].to_vec();
    if opts.cmd.is_empty() {
        return Err("no command given (try q --help)".into());
    }
    Ok(Cmd::Run(opts))
}

/// `90`, `90s`, `1.5s`, `2m`, `1h`, `500ms`.
pub fn duration(flag: &str, v: &str) -> Result<Duration, String> {
    let bad = || format!("{flag} wants a duration like 90s, 2m or 500ms, got {v:?}");
    let (num, unit) = match v.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => v.split_at(i),
        None => (v, "s"),
    };
    let n: f64 = num.parse().map_err(|_| bad())?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "s" => n,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        _ => return Err(bad()),
    };
    if !secs.is_finite() || secs <= 0.0 {
        return Err(bad());
    }
    Ok(Duration::from_secs_f64(secs))
}

fn number(flag: &str, v: &str) -> Result<usize, String> {
    v.parse()
        .map_err(|_| format!("{flag} wants a number, got {v:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn plain_command() {
        let Cmd::Run(o) = parse(&s(&["cargo", "test", "-q"])).unwrap() else {
            panic!()
        };
        assert_eq!(o.cmd, s(&["cargo", "test", "-q"]));
        assert_eq!((o.budget, o.tail), (40, 15));
    }

    #[test]
    fn flags_then_command() {
        let Cmd::Run(o) = parse(&s(&["-n", "80", "--tail", "5", "--grep", "x", "ls"])).unwrap()
        else {
            panic!()
        };
        assert_eq!((o.budget, o.tail, o.grep.as_deref()), (80, 5, Some("x")));
        assert_eq!(o.cmd, s(&["ls"]));
    }

    #[test]
    fn double_dash_escapes_subcommands() {
        let Cmd::Run(o) = parse(&s(&["--", "last"])).unwrap() else {
            panic!()
        };
        assert_eq!(o.cmd, s(&["last"]));
        assert_eq!(parse(&s(&["last"])).unwrap(), Cmd::Last);
    }

    #[test]
    fn durations() {
        let d = |v| duration("-t", v);
        assert_eq!(d("90").unwrap(), Duration::from_secs(90));
        assert_eq!(d("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(d("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(d("1.5s").unwrap(), Duration::from_millis(1500));
        assert!(d("0").is_err() && d("x").is_err() && d("5d").is_err() && d("-1").is_err());
        let Cmd::Run(o) = parse(&s(&["-t", "30s", "sleep", "1"])).unwrap() else {
            panic!()
        };
        assert_eq!(o.timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn errors() {
        assert!(parse(&[]).is_err());
        assert!(parse(&s(&["-n"])).is_err());
        assert!(parse(&s(&["-n", "x", "ls"])).is_err());
        assert!(parse(&s(&["--bogus", "ls"])).is_err());
        assert!(parse(&s(&["-n", "3"])).is_err());
    }
}
