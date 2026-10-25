//! Termination signals sent to q itself (a harness timeout, Ctrl-C, a closed terminal). The handler
//! only records the signal; the wait loop in `run` forwards it to the command and still prints the
//! summary.

use std::sync::atomic::{AtomicI32, Ordering};

static PENDING: AtomicI32 = AtomicI32::new(0);

extern "C" fn record(sig: libc::c_int) {
    PENDING.store(sig, Ordering::SeqCst);
}

pub fn install() {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores into an atomic, which is async-signal-safe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = record as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = libc::SA_RESTART;
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
}

/// The last termination signal q received, if any.
pub fn pending() -> Option<i32> {
    match PENDING.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// Sends `sig` to the command: its whole process group when it has one of its own.
pub fn send(pid: u32, own_group: bool, sig: i32) {
    let pid = pid as libc::pid_t;
    // SAFETY: plain kill(2); a stale pid only yields ESRCH.
    unsafe {
        libc::kill(if own_group { -pid } else { pid }, sig);
    }
}

/// stdin is a terminal: the command must stay in the foreground process group.
pub fn stdin_is_tty() -> bool {
    // SAFETY: isatty on fd 0 has no side effects.
    unsafe { libc::isatty(0) == 1 }
}

pub fn name(sig: i32) -> String {
    match sig {
        libc::SIGTERM => "SIGTERM".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGHUP => "SIGHUP".into(),
        libc::SIGKILL => "SIGKILL".into(),
        s => format!("signal {s}"),
    }
}
