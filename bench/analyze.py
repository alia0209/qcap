# /// script
# requires-python = ">=3.11"
# dependencies = ["tiktoken"]
# ///
"""Compare what an agent reads with and without q for each case that bench/run.sh recorded.

Usage: uv run bench/analyze.py <outdir>
Writes bench/results.json, bench/results.md, docs/bench-light.svg, docs/bench-dark.svg.

Token counts use tiktoken's o200k_base encoding as a stand-in. Claude's tokenizer gives
different absolute numbers, but the ratios between the columns are what matter here.
"""

import json
import math
import re
import sys
from pathlib import Path

import tiktoken

ROOT = Path(__file__).resolve().parent.parent
ENC = tiktoken.get_encoding("o200k_base")
ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
# Claude Code cuts Bash tool output at 30,000 characters by default.
HARNESS_CAP = 30_000


def tokens(text: str) -> int:
    return len(ENC.encode(text, disallowed_special=()))


def tail(text: str, n: int) -> str:
    return "\n".join(text.rstrip("\n").split("\n")[-n:])


def case(d: Path) -> dict:
    meta = json.loads((d / "meta.json").read_text())
    raw = ANSI.sub("", Path(meta["log"]).read_text(errors="replace"))
    summary = (d / "summary.txt").read_text()
    marker = None if meta["marker"] == "-" else meta["marker"]
    views = {
        "raw": raw,
        "harness": raw[:HARNESS_CAP],
        "tail15": tail(raw, 15),
        "tail50": tail(raw, 50),
        "q": summary,
    }
    return {
        "name": meta["name"],
        "command": meta["command"],
        "exit": meta["exit"],
        "lines": raw.count("\n"),
        "tokens": {k: tokens(v) for k, v in views.items()},
        # does the agent see the line that names the real failure?
        "sees_failure": None if marker is None else {k: marker in v for k, v in views.items()},
    }


def table(rows: list[dict]) -> str:
    out = [
        "| case | exit | lines | raw | `tail -15` | `tail -50` | q | raw / q | failure in `tail -15` / `tail -50` / q |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        t = r["tokens"]
        seen = r["sees_failure"]
        mark = "-" if seen is None else " / ".join("yes" if seen[k] else "**no**" for k in ("tail15", "tail50", "q"))
        out.append(
            f"| {r['name']} | {r['exit']} | {r['lines']} | {t['raw']:,} | {t['tail15']:,} | {t['tail50']:,} "
            f"| {t['q']:,} | {t['raw'] / t['q']:.1f}x | {mark} |"
        )
    tot = {k: sum(r["tokens"][k] for r in rows) for k in ("raw", "tail15", "tail50", "q")}
    failing = [r["sees_failure"] for r in rows if r["sees_failure"]]
    seen = " / ".join(f"{sum(s[k] for s in failing)}/{len(failing)}" for k in ("tail15", "tail50", "q"))
    out.append(
        f"| **total** | | | {tot['raw']:,} | {tot['tail15']:,} | {tot['tail50']:,} | {tot['q']:,} "
        f"| {tot['raw'] / tot['q']:.1f}x | {seen} |"
    )
    return "\n".join(out) + "\n"


THEMES = {
    "light": dict(bg="#fcfcfb", ink="#0b0b0b", ink2="#52514e", muted="#898781", grid="#e1e0d9",
                  raw="#2a78d6", q="#eb6834"),
    "dark": dict(bg="#1a1a19", ink="#f0efec", ink2="#c3c2b7", muted="#898781", grid="#2c2c2a",
                 raw="#3987e5", q="#d95926"),
}


def svg(rows: list[dict], theme: str) -> str:
    c = THEMES[theme]
    left, right, top, row_h = 190, 120, 70, 30
    plot_w = 520
    width = left + plot_w + right
    height = top + row_h * len(rows) + 50
    lo, hi = 1, 5  # 10 .. 100,000 tokens, log scale

    def x(v: float) -> float:
        return left + (math.log10(max(v, 10)) - lo) / (hi - lo) * plot_w

    font = 'font-family="ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif"'
    p = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" {font} role="img" '
        f'aria-label="Tokens an agent reads per command: raw output versus q summary">',
        f'<rect width="100%" height="100%" rx="8" fill="{c["bg"]}"/>',
        f'<text x="20" y="28" font-size="15" font-weight="600" fill="{c["ink"]}">'
        f"Tokens an agent reads per command</text>",
        # legend
        f'<circle cx="26" cy="48" r="5" fill="{c["raw"]}"/>'
        f'<text x="36" y="52" font-size="12" fill="{c["ink2"]}">raw output</text>',
        f'<circle cx="120" cy="48" r="5" fill="{c["q"]}"/>'
        f'<text x="130" y="52" font-size="12" fill="{c["ink2"]}">q summary</text>',
        f'<text x="{width - 20}" y="52" font-size="12" text-anchor="end" fill="{c["muted"]}">log scale</text>',
    ]
    axis_y = top + row_h * len(rows)
    for e in range(lo, hi + 1):
        gx = x(10**e)
        p.append(f'<line x1="{gx:.1f}" y1="{top - 8}" x2="{gx:.1f}" y2="{axis_y}" stroke="{c["grid"]}"/>')
        label = f"{10**e:,}"
        p.append(f'<text x="{gx:.1f}" y="{axis_y + 18}" font-size="11" text-anchor="middle" fill="{c["muted"]}">{label}</text>')
    for i, r in enumerate(rows):
        y = top + row_h * i + row_h / 2
        raw, q = r["tokens"]["raw"], r["tokens"]["q"]
        x1, x2 = sorted((x(raw), x(q)))
        p.append(f'<text x="{left - 12}" y="{y + 4}" font-size="12" text-anchor="end" fill="{c["ink"]}">{r["name"]}</text>')
        p.append(f'<line x1="{x1:.1f}" y1="{y}" x2="{x2:.1f}" y2="{y}" stroke="{c["muted"]}" stroke-width="2"/>')
        for v, col, kind in ((raw, c["raw"], "raw"), (q, c["q"], "q")):
            p.append(f'<circle cx="{x(v):.1f}" cy="{y}" r="5" fill="{col}" stroke="{c["bg"]}" stroke-width="2">'
                     f"<title>{r['name']}: {kind} {v:,} tokens</title></circle>")
        ratio = raw / q
        text = f"{ratio:.0f}x less" if ratio >= 2 else ("same" if ratio > 0.8 else "q adds header")
        p.append(f'<text x="{left + plot_w + 12}" y="{y + 4}" font-size="12" fill="{c["ink2"]}">{text}</text>')
    p.append("</svg>")
    return "\n".join(p) + "\n"


def main() -> None:
    out = Path(sys.argv[1])
    rows = sorted((case(d) for d in out.iterdir() if (d / "meta.json").exists()),
                  key=lambda r: -r["tokens"]["raw"])
    (ROOT / "bench/results.json").write_text(json.dumps(rows, indent=2) + "\n")
    (ROOT / "bench/results.md").write_text(table(rows))
    for theme in THEMES:
        (ROOT / f"docs/bench-{theme}.svg").write_text(svg(rows, theme))
    print(table(rows))


if __name__ == "__main__":
    main()
