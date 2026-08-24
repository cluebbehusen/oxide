"""Builds a browser-openable review page for map drafts.

Map drafts live in ``map-drafts/`` — outside ``scenarios/`` so the
shipped-map sweeps and hash fixtures never see unblessed work. This
tool renders each draft through the driver's CPU rasterizer, runs the
structural audit, and writes one self-contained HTML file (images
inlined) for review in any browser.

Blessing a draft means moving its JSON into ``scenarios/``, re-running
the map gates, blessing the hash fixture row, and committing — this
page only presents; it never promotes.

Usage (from the repository root):
    uv run tools/map_review.py
    open map-review/index.html
"""

from __future__ import annotations

import argparse
import base64
import html
import json
import math
import pathlib
import subprocess
import tempfile


def far_scrap_share(rows: list[str], threshold: float = 18.0) -> tuple[float, int]:
    """Share of scrap value sitting beyond turtling range of every
    Foundry anchor, and the total scrap units on the map."""
    anchors = [
        (x, y)
        for y, row in enumerate(rows)
        for x, c in enumerate(row)
        if c in "12345678" or c in "abcdefgh"
    ]
    scraps = [
        (x, y, 3 if c == "S" else 1)
        for y, row in enumerate(rows)
        for x, c in enumerate(row)
        if c in "sS"
    ]
    total = sum(v for _, _, v in scraps)
    if not anchors or not total:
        return 0.0, total
    far = sum(
        v
        for x, y, v in scraps
        if min(math.hypot(x - ax, y - ay) for ax, ay in anchors) > threshold
    )
    return far / total, total


def run_json(cmd: list[str]) -> dict | list | None:
    result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    text = result.stdout.strip()
    if text.startswith("{") or text.startswith("["):
        return json.loads(text)
    return None


def review_card(name: str, meta: dict, stats: dict, png: bytes) -> str:
    data = base64.b64encode(png).decode()
    rows = "".join(
        f"<tr><th>{html.escape(k)}</th><td>{html.escape(str(v))}</td></tr>"
        for k, v in stats.items()
    )
    hook = html.escape(meta.get("hook", ""))
    badges = " · ".join(
        html.escape(str(meta.get(k, "")))
        for k in ("mode", "pace", "richness", "theme")
        if meta.get(k)
    )
    return f"""
<section class="card">
  <h2>{html.escape(name)}</h2>
  <p class="badges">{badges}</p>
  <p class="hook">{hook}</p>
  <img src="data:image/png;base64,{data}" alt="{html.escape(name)} render">
  <table>{rows}</table>
</section>"""


def page(cards: list[str]) -> str:
    body = "\n".join(cards)
    return f"""<!doctype html>
<html><head><meta charset="utf-8"><title>Map drafts</title>
<style>
  body {{ background:#232327; color:#d8d4cc; font:15px/1.5 -apple-system, sans-serif;
         max-width: 1100px; margin: 2rem auto; padding: 0 1rem; }}
  h1 {{ font-weight: 600; letter-spacing: .02em; }}
  .card {{ background:#2b2b31; border:1px solid #3a3a42; border-radius:10px;
           padding:1.2rem 1.4rem; margin:1.4rem 0; }}
  .card h2 {{ margin:.1rem 0 .2rem; color:#e8b44c; font-weight:600; }}
  .badges {{ color:#8f8b84; margin:.1rem 0 .4rem; text-transform:uppercase;
             font-size:.78rem; letter-spacing:.06em; }}
  .hook {{ font-style: italic; color:#b8b4ac; margin:.2rem 0 .8rem; }}
  img {{ width:100%; border-radius:6px; border:1px solid #3a3a42; }}
  table {{ margin-top:.8rem; border-collapse:collapse; width:100%; }}
  th {{ text-align:left; color:#8f8b84; font-weight:500; padding:.15rem .8rem .15rem 0;
        white-space:nowrap; vertical-align:top; }}
  td {{ padding:.15rem 0; }}
</style></head>
<body>
<h1>Map drafts for review</h1>
<p>Rendered by the driver's CPU rasterizer. Blessing a draft = move its
JSON from <code>map-drafts/</code> into <code>scenarios/</code>, re-run
the map gates, bless the hash fixture, and commit.</p>
{body}
</body></html>
"""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--drafts", default="map-drafts")
    ap.add_argument("--out", default="map-review/index.html")
    ap.add_argument("--driver", default="target/release/oxide-driver")
    args = ap.parse_args()

    drafts = sorted(pathlib.Path(args.drafts).glob("*.json"))
    if not drafts:
        raise SystemExit(f"no drafts under {args.drafts}/")
    cards = []
    for draft in drafts:
        payload = json.loads(draft.read_text())
        rows = payload["map"]
        meta = payload.get("meta") or {}
        share, total = far_scrap_share(rows)

        with tempfile.NamedTemporaryFile(suffix=".png") as tmp:
            subprocess.run(
                [args.driver, "render", str(draft), "--out", tmp.name],
                check=True,
                capture_output=True,
            )
            png = pathlib.Path(tmp.name).read_bytes()

        audit = run_json([args.driver, "map-audit", str(draft), "--json"]) or {}
        stats: dict = {
            "size": f"{len(rows[0])} x {len(rows)}",
            "seats": len(payload.get("players", [])),
            "scrap": f"{total} units, {share:.0%} beyond turtling range",
        }
        if isinstance(audit, dict):
            for key in ("free_tiles", "nodes"):
                if key in audit:
                    stats[key.replace("_", " ")] = audit[key]

        cards.append(review_card(draft.stem, meta, stats, png))

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(page(cards))
    print(f"review page: {out} ({len(cards)} drafts)")


if __name__ == "__main__":
    main()
