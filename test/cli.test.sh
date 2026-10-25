#!/usr/bin/env bash
# Black-box tests for the release binary. Usage: bash test/cli.test.sh
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
q="$root/target/release/q"
fx="$root/test/fixtures"
export Q_DIR
Q_DIR="$(mktemp -d)"
trap 'rm -rf "$Q_DIR"' EXIT
fails=0

check() { # check <name> <expected-exit> <actual-exit> <output> <must-contain...>
  local name="$1" want="$2" got="$3" out="$4"
  shift 4
  if [[ "$got" != "$want" ]]; then
    echo "FAIL $name: exit $got, want $want"; echo "$out"; fails=$((fails + 1)); return
  fi
  for needle in "$@"; do
    if [[ "$out" != *"$needle"* ]]; then
      echo "FAIL $name: missing [$needle]"; echo "$out"; fails=$((fails + 1)); return
    fi
  done
  echo "ok   $name"
}

out="$("$q" true)"; check "success short" 0 $? "$out" "q: exit 0 · " "· 0 lines · log $Q_DIR/true-1.log"
out="$("$q" true)"; check "log numbering" 0 $? "$out" "true-2.log"

out="$("$q" bash "$fx/fail-long.sh")"; code=$?
check "failure sections" 3 "$code" "$out" "q: exit 3 ·" "· 300 lines ·" "-- first error: line 150 --" \
  "150: error: something broke" "-- last lines: 286-300 --" $'line 299 ok\nREFUSED lost_edit (exit 3)'
[[ "$out" != *"line 50 ok"* ]] || { echo "FAIL failure sections: printed uncapped output"; fails=$((fails + 1)); }
lines="$(printf '%s\n' "$out" | wc -l)"
((lines <= 1 + 1 + 40 + 1 + 15)) || { echo "FAIL output too long: $lines lines"; fails=$((fails + 1)); }

out="$("$q" -n 3 --tail 2 bash "$fx/fail-long.sh")"
check "budget flags" 3 $? "$out" "-- last lines: 299-300 --"
[[ "$(printf '%s\n' "$out" | wc -l)" -le 8 ]] || { echo "FAIL budget flags too long"; fails=$((fails + 1)); }

out="$("$q" --grep 'REFUSED|broke' bash "$fx/fail-long.sh")"
check "grep" 3 $? "$out" "150: error: something broke" "300: REFUSED lost_edit"
[[ "$out" != *"-- last lines"* ]] || { echo "FAIL grep printed sections"; fails=$((fails + 1)); }

out="$("$q" "echo out; echo err >&2; echo out2; exit 4")"
check "shell string, merged order" 4 $? "$out" $'out\nerr\nout2'

out="$("$q" bash -c 'kill -TERM $$')"; check "signal exit" 143 $? "$out" "q: exit 143 (killed by SIGTERM)"
out="$("$q" no-such-command-xyz)"; check "missing command" 127 $? "$out" "q: cannot run no-such-command-xyz"

out="$("$q" last)"; check "last" 0 $? "$out" "$Q_DIR/no-such-command-xyz-1.log"
out="$("$q" show 150-151 "$Q_DIR/bash-fail-long.sh-1.log")"; check "show range" 0 $? "$out" "150: error: something broke" "151: line 151 ok"

dir_a="$Q_DIR/work-a"; dir_b="$Q_DIR/work-b"; mkdir -p "$dir_a" "$dir_b"
(cd "$dir_a" && "$q" echo from-a >/dev/null); (cd "$dir_b" && "$q" echo from-b >/dev/null)
out="$(cd "$dir_a" && "$q" last)"; check "last is per directory" 0 $? "$out" "echo-from-a-1.log"

touch -t 202001010000 "$Q_DIR/true-1.log"; "$q" true >/dev/null
if [[ ! -e "$Q_DIR/true-1.log" && -e "$Q_DIR/true-2.log" ]]; then echo "ok   prune old logs"
else echo "FAIL prune old logs"; fails=$((fails + 1)); fi

marker="q-orphan-check-$$"
out="$("$q" -t 300ms bash -c "echo started; exec -a $marker sleep 30")"
check "timeout" 124 $? "$out" "q: exit 124 (timeout after 0.3s)" "started"
sleep 0.2
if pgrep -f "^$marker" >/dev/null; then echo "FAIL timeout left the command running"; fails=$((fails + 1))
else echo "ok   timeout stops the whole command"; fi

"$q" bash -c 'echo tick; sleep 30' >"$Q_DIR/term.out" &
qpid=$!
sleep 0.5; kill -TERM "$qpid"; wait "$qpid"; code=$?
check "q gets SIGTERM" 143 "$code" "$(cat "$Q_DIR/term.out")" "q: exit 143 (q got SIGTERM, command stopped)" "tick"

out="$(Q_NOTE_AFTER=0.1 "$q" bash -c 'sleep 0.4; echo done')"
check "still-running note" 0 $? "$out" $'q: still running · log '"$Q_DIR"$'/bash-sleep-1.log\nq: exit 0'
out="$("$q" true)"; [[ "$out" != *"still running"* ]] || { echo "FAIL note on a fast command"; fails=$((fails + 1)); }

out="$("$q" show -2 "$Q_DIR/bash-fail-long.sh-1.log")"; check "show last N" 0 $? "$out" "lines 299-300 of 300" "300: REFUSED"
out="$("$q" show "$Q_DIR/bash-fail-long.sh-1.log" 299-)"; check "show open end, log first" 0 $? "$out" "lines 299-300 of 300" "299: line 299 ok"
json="$("$q" --json bash "$fx/fail-long.sh")"; code=$?
jq_ok() { jq -e "$1" <<<"$json" >/dev/null; }
if ((code == 3)) && jq_ok '.exit == 3 and .lines == 300 and .complete == false and .first_error == 150
    and (.error_block[] | select(.line == 150) | .text) == "error: something broke"
    and (.tail | length) == 15 and .tail[-1].text == "REFUSED lost_edit (exit 3)"
    and .stopped == null and .binary == false and (.log | endswith(".log"))'; then
  echo "ok   json failure"
else echo "FAIL json failure (exit $code): $json"; fails=$((fails + 1)); fi
json="$(Q_NOTE_AFTER=0 "$q" --json -t 300ms --grep tick bash -c 'echo tick; sleep 5' 2>"$Q_DIR/json.err")"; code=$?
if ((code == 124)) && jq_ok '.stopped.reason == "timeout" and .grep.total == 1 and .grep.matches[0].line == 1' \
  && grep -q 'still running' "$Q_DIR/json.err"; then echo "ok   json timeout + grep, note on stderr"
else echo "FAIL json timeout (exit $code): $json"; fails=$((fails + 1)); fi
out="$(Q_MAX_LOG=100000 "$q" yes)"
check "output cap" 124 $? "$out" "q: exit 124 (output over 100000 bytes, command stopped)" "-- last lines:"
json="$("$q" --json echo one)"
if jq_ok '.complete and .tail == [{"line": 1, "text": "one"}] and .exit == 0'; then echo "ok   json short"
else echo "FAIL json short: $json"; fails=$((fails + 1)); fi

out="$("$q" --bogus ls 2>&1)"; check "unknown flag" 2 $? "$out" "unknown option --bogus"

hook_in='{"tool_name":"Bash","tool_input":{"command":"cargo test 2>&1 | tail -5 && git commit"}}'
out="$(printf '%s' "$hook_in" | "$q" hook)"; check "hook hints" 0 $? "$out" '"additionalContext"' '"PreToolUse"'
out="$(printf '%s' '{"tool_name":"Bash","tool_input":{"command":"ls | head"}}' | "$q" hook)"
check "hook quiet" 0 $? "$out:" ":"
[[ -z "$out" ]] || { echo "FAIL hook not quiet: $out"; fails=$((fails + 1)); }
out="$(printf 'garbage' | "$q" hook)"; check "hook garbage" 0 $? "$out" ""
out="$("$q" hook --check 'cd x && cargo test 2>&1 | tail -3')"; check "hook check hint" 0 $? "$out" "hint: capped gate: cargo test"
out="$("$q" hook --check 'rg -n foo src | head -20 && git log | head')"; check "hook check quiet" 1 $? "$out" "quiet"

out="$("$q" bash -c 'printf "a\0b"; exit 5')"; check "binary output" 5 $? "$out" "· 3 bytes of binary output (not shown) · log $Q_DIR/bash-printf-1.log"
out="$("$q" -- bash -c 'uname')"; check "slug skips flags" 0 $? "$out" "bash-uname-1.log"

if ((fails > 0)); then echo "$fails cli test(s) failed"; exit 1; fi
echo "all cli tests passed"
