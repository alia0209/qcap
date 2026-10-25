mod args;
mod hook;
mod logpath;
mod pipeline;
mod report;
mod run;
mod show;
mod signals;
mod summary;
mod text;

use args::{Cmd, RunOpts};
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

/// q itself failed before the command could run (like `timeout`'s 125), so the code can't be
/// mistaken for the command's.
const INTERNAL: i32 = 125;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match args::parse(&argv) {
        Ok(Cmd::Help) => {
            println!("{}", args::USAGE);
            0
        }
        Ok(Cmd::Hook { check: None }) => hook::main(),
        Ok(Cmd::Hook { check: Some(c) }) => match pipeline::explain(&c) {
            Some(h) => {
                println!("hint: {}: {}", h.why, h.command);
                if let Some(s) = h.suggestion {
                    println!("try: {s}");
                }
                0
            }
            None => {
                println!("quiet");
                1
            }
        },
        Ok(Cmd::Last) => print_or_fail(show::last()),
        Ok(Cmd::Show { range, log }) => print_or_fail(show::show(range.as_deref(), log.as_deref())),
        Ok(Cmd::Run(opts)) => run_cmd(&opts),
        Err(e) => {
            eprintln!("q: {e}");
            2
        }
    };
    ExitCode::from(code.clamp(0, 255) as u8)
}

fn print_or_fail(r: Result<String, String>) -> i32 {
    match r {
        Ok(s) => {
            print!("{}", if s.ends_with('\n') { s } else { s + "\n" });
            0
        }
        Err(e) => {
            eprintln!("q: {e}");
            1
        }
    }
}

fn run_cmd(opts: &RunOpts) -> i32 {
    let grep = match opts.grep.as_deref().map(regex::Regex::new).transpose() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("q: bad --grep pattern: {e}");
            return 2;
        }
    };
    let dir = logpath::dir();
    let (path, file) = match logpath::create(&dir, &logpath::slug(&opts.cmd)) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("q: cannot create a log in {}: {e}", dir.display());
            return INTERNAL;
        }
    };
    signals::install();
    let log_path = path.display().to_string();
    let watch = run::Watch {
        timeout: opts.timeout,
        note_after: note_after(),
        note_to_stderr: opts.json,
        max_log: max_log(),
        log_path: &log_path,
    };
    let outcome = run::run(&opts.cmd, file, &watch);
    let raw = std::fs::read(&path).unwrap_or_default();
    let report = report::Report {
        cmd: &opts.cmd,
        outcome: &outcome,
        log: &log_path,
        raw: &raw,
        budget: opts.budget,
        tail: opts.tail,
        grep: grep.as_ref(),
    };
    let out = if opts.json {
        report.json() + "\n"
    } else {
        report.text()
    };
    let _ = std::io::stdout().write_all(out.as_bytes());
    outcome.code
}

/// Log size at which q stops the command: 512 MiB, or `Q_MAX_LOG` bytes.
fn max_log() -> u64 {
    std::env::var("Q_MAX_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512 * 1024 * 1024)
}

/// When the "still running" note prints. `Q_NOTE_AFTER` (seconds) overrides it, for tests.
fn note_after() -> Duration {
    std::env::var("Q_NOTE_AFTER")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map_or(Duration::from_secs(10), Duration::from_secs_f64)
}
