#!/usr/bin/env bash
# Run each benchmark case through q once. The q log holds the raw output an agent would
# otherwise read; the printed summary is what q shows instead.
# Usage: bench/run.sh <cases.tsv> <srcdir> <outdir>
# cases.tsv columns: name, repo (a directory under srcdir), command, marker (text that identifies
# the real failure, or -), setup (optional shell run in the repo first, not measured).
# Every case starts with `git checkout -q .`, so setups don't leak into the next case.
set -uo pipefail
cases=${1:?usage: run.sh <cases.tsv> <srcdir> <outdir>}
src=${2:?usage: run.sh <cases.tsv> <srcdir> <outdir>}
out=${3:?usage: run.sh <cases.tsv> <srcdir> <outdir>}
mkdir -p "$out"
while IFS=$'\t' read -r name repo cmd marker setup; do
  [[ -z "$name" || "$name" == \#* ]] && continue
  dir="$src/$repo"
  git -C "$dir" checkout -q .
  if [[ -n "${setup:-}" ]] && ! (cd "$dir" && bash -c "$setup") >/dev/null 2>&1; then
    echo "$name: setup failed" >&2
    continue
  fi
  rm -rf "${out:?}/$name"
  mkdir -p "$out/$name/logs"
  (cd "$dir" && Q_DIR="$out/$name/logs" q "$cmd" >"$out/$name/summary.txt" 2>&1)
  code=$?
  log=$(find "$out/$name/logs" -maxdepth 1 -name '*.log' | head -1)
  jq -n --arg name "$name" --arg cmd "$cmd" --arg marker "$marker" --arg log "$log" \
    --argjson exit "$code" \
    '{name: $name, command: $cmd, marker: $marker, exit: $exit, log: $log}' \
    >"$out/$name/meta.json"
  git -C "$dir" checkout -q .
  echo "$name exit=$code"
done <"$cases"
