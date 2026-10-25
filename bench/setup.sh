#!/usr/bin/env bash
# Clone the benchmark repos at pinned commits and install their dependencies.
# Usage: bench/setup.sh <srcdir>
# Needs git, cargo, bun, npm and uv.
set -euo pipefail
src=${1:?usage: setup.sh <srcdir>}
mkdir -p "$src"

clone() { # name url commit
  if [[ ! -d "$src/$1" ]]; then
    git init -q "$src/$1"
    git -C "$src/$1" remote add origin "$2"
  fi
  git -C "$src/$1" fetch -q --depth 1 origin "$3"
  git -C "$src/$1" checkout -q --force FETCH_HEAD
}

clone ripgrep https://github.com/BurntSushi/ripgrep 6892bc2b7efe5909cbbeec8f757ed2c4c76b1d30
clone hono https://github.com/honojs/hono bac5c4a8bcd76b165428ce757e1185753c7935bd
clone flask https://github.com/pallets/flask d086db856be187255b8ec61ef409357393020f32
clone express https://github.com/expressjs/express 1c6f1b0fe4000977b9a7b4f34097e4d527dd9824

(cd "$src/hono" && bun install --silent)
(cd "$src/express" && npm install --silent --no-audit --no-fund)
(cd "$src/flask" && uv venv -q --allow-existing && uv pip install -q -e '.[async]' pytest)
