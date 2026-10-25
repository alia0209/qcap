//! Runs the command with stdout and stderr on the same log file descriptor, so lines stay in order.
//! While it runs, q watches for its own termination signals and the `--timeout`, stops the command
//! when either fires, and prints a one-line "still running" note with the log path for long runs.

use crate::signals;
use std::fs::File;
use std::io::Write;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(50);
/// After a forwarded signal, how long the command gets before SIGKILL.
const GRACE_SIGNAL: Duration = Duration::from_secs(1);
/// After a timeout's SIGTERM, how long the command gets before SIGKILL.
const GRACE_TIMEOUT: Duration = Duration::from_secs(3);

/// Why q stopped the command early.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stop {
    Timeout(Duration),
    /// The log grew past this many bytes (`/tmp` is often RAM).
    Oversize(u64),
    Signal(i32),
}

pub struct Outcome {
    pub code: i32,
    /// Set when the command was killed by a signal (code is then 128+signal).
    pub signal: Option<i32>,
    pub elapsed: Duration,
    pub stopped: Option<Stop>,
}

pub struct Watch<'a> {
    pub timeout: Option<Duration>,
    /// Print `still running · log <path>` once the command has run this long.
    pub note_after: Duration,
    /// Stop the command once the log is bigger than this.
    pub max_log: u64,
    /// `--json` keeps stdout to the one JSON object, so the note goes to stderr.
    pub note_to_stderr: bool,
    pub log_path: &'a str,
}

/// A lone argument with whitespace or shell syntax is a shell string: run it through `bash -c`.
pub fn needs_shell(cmd: &[String]) -> bool {
    cmd.len() == 1
        && cmd[0]
            .chars()
            .any(|c| c.is_whitespace() || "|&;<>()$`*?[]{}~\"'\\#=!".contains(c))
}

fn command(cmd: &[String]) -> Command {
    if needs_shell(cmd) {
        let mut c = Command::new("bash");
        c.arg("-c").arg(&cmd[0]);
        c
    } else {
        let mut c = Command::new(&cmd[0]);
        c.args(&cmd[1..]);
        c
    }
}

pub fn run(cmd: &[String], mut log: File, watch: &Watch) -> Outcome {
    let start = Instant::now();
    // Without a terminal the command gets its own process group, so a stop reaches its children
    // too. With one it stays in the foreground group, so prompts and Ctrl-C work as usual.
    let own_group = !signals::stdin_is_tty();
    let mut command = command(cmd);
    if own_group {
        command.process_group(0);
    }
    let spawned = log.try_clone().and_then(|err| {
        command
            .stdout(log.try_clone()?)
            .stderr(err)
            .stdin(Stdio::inherit())
            .spawn()
    });
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            };
            let _ = writeln!(log, "q: cannot run {}: {e}", cmd[0]);
            return Outcome {
                code,
                signal: None,
                elapsed: start.elapsed(),
                stopped: None,
            };
        }
    };
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    let (status, stopped) = wait(&rx, pid, own_group, start, watch);
    // A harness that kills the whole tree hits q and the command together: the command's exit can
    // arrive before the wait loop sees q's own signal.
    let stopped = stopped.or_else(|| signals::pending().map(Stop::Signal));
    let elapsed = start.elapsed();
    let Some(status) = status else {
        return Outcome {
            code: 1,
            signal: None,
            elapsed,
            stopped,
        };
    };
    let natural = status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
    Outcome {
        code: match stopped {
            Some(Stop::Timeout(_) | Stop::Oversize(_)) => 124,
            Some(Stop::Signal(s)) => 128 + s,
            None => natural,
        },
        signal: status.signal(),
        elapsed,
        stopped,
    }
}

type Waited = std::io::Result<ExitStatus>;

fn wait(
    rx: &mpsc::Receiver<Waited>,
    pid: u32,
    own_group: bool,
    start: Instant,
    watch: &Watch,
) -> (Option<ExitStatus>, Option<Stop>) {
    let mut stopped: Option<Stop> = None;
    let mut kill_at: Option<Instant> = None;
    let mut noted = false;
    loop {
        match rx.recv_timeout(TICK) {
            Ok(status) => return (status.ok(), stopped),
            Err(mpsc::RecvTimeoutError::Disconnected) => return (None, stopped),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if stopped.is_none() {
            if let Some(sig) = signals::pending() {
                stopped = Some(Stop::Signal(sig));
                signals::send(pid, own_group, sig);
                kill_at = Some(now + GRACE_SIGNAL);
            } else if let Some(t) = watch.timeout.filter(|&t| now - start >= t) {
                stopped = Some(Stop::Timeout(t));
                signals::send(pid, own_group, libc::SIGTERM);
                kill_at = Some(now + GRACE_TIMEOUT);
            } else if std::fs::metadata(watch.log_path).is_ok_and(|m| m.len() > watch.max_log) {
                stopped = Some(Stop::Oversize(watch.max_log));
                signals::send(pid, own_group, libc::SIGTERM);
                kill_at = Some(now + GRACE_TIMEOUT);
            }
        }
        if kill_at.is_some_and(|k| now >= k) {
            signals::send(pid, own_group, libc::SIGKILL);
            kill_at = None;
        }
        if !noted && stopped.is_none() && now - start >= watch.note_after {
            noted = true;
            let note = format!("q: still running · log {}", watch.log_path);
            if watch.note_to_stderr {
                eprintln!("{note}");
            } else {
                let mut out = std::io::stdout();
                let _ = writeln!(out, "{note}");
                let _ = out.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn shell_detection() {
        assert!(needs_shell(&s(&["bun test | tail"])));
        assert!(needs_shell(&s(&["make validate"])));
        assert!(needs_shell(&s(&["a&&b"])));
        assert!(!needs_shell(&s(&["ls"])));
        assert!(!needs_shell(&s(&["echo", "a | b"])));
    }
}
