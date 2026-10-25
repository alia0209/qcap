# qcap (`q`)

`q <cmd...>` runs a command, keeps its exit code, writes the merged stdout+stderr to
`/tmp/q/<slug>-<n>.log`, and prints one header line plus a capped summary (first error block on
failure, last 15 lines). Replaces `cmd | tail` and `>/tmp/x.log 2>&1; echo $?`.

## Validate

`q make validate` (fmt check, clippy -D warnings, unit tests, release build, `test/cli.test.sh`
against the release binary). Needs `shellcheck`.

## Run / drive

- `make install` symlinks `~/.local/bin/q` -> `target/release/q`.
- `q cargo test`, `q -n 80 bun run validate`, `q --grep 'FAIL|panicked' cargo test`, `q "a && b | c"`
  (one argument with spaces or shell syntax runs through `bash -c`), `q -t 100s bun test`,
  `q --json cargo test | jq .first_error`.
- `q last`, `q show 120-160` (numbered slice of the newest log started from this directory),
  `q show -50` (last 50), `q show 1800-` (to the end, 500 lines max), `q show /tmp/q/x.log 1-40`.
- `q hook`: Claude Code PreToolUse (Bash) hint, set up as in the README. Reads hook JSON on
  stdin, prints `additionalContext` when a command pipes a gate (validate/test/check/build/lint/tsc/
  make) or a non-read-only `&&` step through head/tail, naming the rewrite
  (`run \`q cargo test\``). Read-only surveys (`rg`, `git`, `ls`, `cat`...), heredoc bodies and quoted strings never trigger it. Per `session_id`: full
  hint once, a one-line reminder up to 10 times, then silent (`/tmp/q/hook/`). Never blocks, always
  exits 0. `q hook --check CMD` explains a decision and prints the rewrite (exit 0 hint, 1 quiet).
- `Q_DIR` overrides the log directory (tests use a temp dir). Logs older than 2 days are pruned
  whenever a new one is created. `Q_NOTE_AFTER` (seconds) moves the still-running note, `Q_MAX_LOG`
  (bytes) the output cap (tests).

## Behaviour contract

- Exit code = the command's; killed by signal N -> 128+N; not found -> 127; stopped by q (`-t` or
  the output cap) -> 124; q itself failed (can't create the log) -> 125; bad q usage -> 2.
- Output cap: `/tmp` is often RAM-backed (tmpfs), so once the log passes 512 MiB (checked every 50ms) q stops the
  command like a timeout: `(output over 512 MiB, command stopped)`.
- Without a terminal on stdin the command runs in its own process group, so a stop reaches its
  children. q catches SIGTERM/SIGINT/SIGHUP (a harness timeout or kill): forwards the signal,
  SIGKILL after 1s, prints the summary with `(q got SIGTERM, command stopped)`, exits 128+signal.
  `-t DUR`: SIGTERM at DUR, SIGKILL 3s later, `(timeout after DUR)`.
- Runs longer than 10s print `q: still running · log <path>` first, so a SIGKILLed q still leaves
  the log path behind.
- Output of at most `-n` + `--tail` lines prints in full, no sections, and the header leaves out
  `· log <path>` (the log holds nothing more; `q last` and `--json` still give it). If the run was
  also clean (exit 0, not stopped or killed, no `--grep`) there is no header at all: stdout is
  exactly the cleaned output.
- Longer: on failure `-- first error: line L --` then numbered grep -C3 groups, at most `-n` lines,
  only from above the tail. Groups start at strong lines (`panicked`, `error:`/`error[E..]:`,
  `error TS..`, `...Error`, `Traceback`, `REFUSED`/`refused`, `assert`, `(fail)`, `✗✖✘`, TAP
  `not ok`, `FAIL` at line start) when any exist, else at any error line (`error`/`fail` at a word
  start). `0 failed`/`0 errors` and passing-test lines (`... ok`, `(pass)`, `✓`) never match.
  After the first group, a match whose trimmed text is already in a group or in the tail starts
  no group (pytest/vitest repeat one assertion per failing test).
  Left-over matches: `-- N more error lines before the tail: q show A-B --`. Then
  `-- last lines: A-B --` and the tail. On success only the tail.
- Binary output (NUL or mostly invalid UTF-8): header says `N bytes of binary output (not shown)`.
- `--json`: exactly one object on stdout (the still-running note goes to stderr): `exit`, `seconds`,
  `log`, `command` (argv), `bytes`, `lines` (null if binary), `binary`, `stopped` (null |
  `{reason: timeout, after_seconds}` | `{reason: signal, signal}` | `{reason: output_cap,
  max_bytes}`), `killed_by` (signal name or null), `complete` (whole output is in `tail`),
  `first_error` (line or null), `error_block`, `more_errors` (`{count, from, to}` or null), `tail`,
  `grep` (null | `{pattern, total, matches}`). Line entries are `{line, text}`, 1-based. Unlike the
  text form, `--grep` adds `grep` and keeps the sections.
- Display strips ANSI, keeps the last `\r` frame, cuts lines over 400 chars. The log is raw.
- Slug: first two words that are not flags or `VAR=value` (`bash -c 'seq 3'` -> `bash-seq`).

## Code map

- `src/main.rs` dispatch, env knobs, wiring a run together.
- `src/report.rs` header line, text and `--json` rendering of one run.
- `src/args.rs` flag parsing (durations too) and usage text.
- `src/run.rs` spawn (one fd for stdout+stderr), `bash -c` detection, wait loop (signals, timeout,
  still-running note), exit codes.
- `src/signals.rs` signal handler, forwarding to the command's process group.
- `src/logpath.rs` slug, atomic next `<slug>-<n>.log`, pruning, newest log per directory.
- `src/text.rs` `Lines`: streaming reader of cleaned display lines (bounded per line, even for a
  huge `\r` progress line), binary detection.
- `src/summary.rs` error matching (strong/weak), `summarize`: one streaming pass with bounded
  memory (head for short output, tail ring, error groups, grep hits), text render.
- `src/show.rs` `q last`, `q show` ranges.
- `src/hook.rs` PreToolUse hook I/O, hint text, per-session rate limit.
- `src/pipeline.rs` shell-command analysis: does it cap a gate or an `&&` step, and the `q` rewrite.
- `test/cli.test.sh` black-box tests, `test/fixtures/`.
- `bench/run.sh` runs token benchmark cases through q; `bench/analyze.py` (`uv run`) writes
  `bench/results.*` and the README chart `docs/bench-{light,dark}.svg`. Rerun both and update the
  README numbers when summary output changes.
