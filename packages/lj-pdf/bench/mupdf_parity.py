# /// script
# requires-python = ">=3.12"
# dependencies = ["pymupdf>=1.28"]
# ///
"""Text decoding parity with MuPDF.

`cargo run --release -p lj-pdf --example glyph_dump -- DIR OUT` writes the
glyphs lj-pdf extracts, one JSON line per file. This script reads MuPDF's
characters from the same files and scores, per file: did it open, how much of
MuPDF's text lj-pdf found (recall), how much of what it found MuPDF agrees with
(precision), and how many matched characters sit where MuPDF puts them.

    uv run packages/lj-pdf/bench/mupdf_parity.py OUT [--worst 30]
"""

import argparse
import json
import statistics
import sys
import time
import unicodedata
from collections import Counter, defaultdict

import pymupdf

pymupdf.TOOLS.mupdf_display_errors(False)
pymupdf.TOOLS.mupdf_display_warnings(False)


def norm(c):
    return unicodedata.normalize("NFKC", c)


def oracle(path):
    """MuPDF's characters per page, as (char, x0, y0, x1, y1) on the page as
    shown: origin at the lower left of the crop box, rotation applied, y up,
    which is the space lj-pdf gives its glyphs in."""
    doc = pymupdf.open(path)
    pages = []
    for page in doc:
        height = page.rect.height
        rotate = page.rotation_matrix
        chars = []
        for block in page.get_text("rawdict", flags=pymupdf.TEXT_PRESERVE_LIGATURES)["blocks"]:
            for line in block.get("lines", []):
                for span in line["spans"]:
                    for ch in span["chars"]:
                        x0, y0, x1, y1 = pymupdf.Rect(ch["bbox"]) * rotate
                        chars.append((ch["c"], x0, height - y1, x1, height - y0))
        pages.append(chars)
    return pages


def flat(chars):
    out = []
    for c, *box in chars:
        for n in norm(c):
            if not n.isspace():
                out.append((n, box))
    return out


def score(ours, theirs):
    """Characters both read, whatever the order, and how many of ours sit on
    the box of the same character in MuPDF, the nearest one still free."""
    matched = sum((Counter(c for c, _ in ours) & Counter(c for c, _ in theirs)).values())
    cell = 24.0
    grid = defaultdict(list)
    for index, (c, box) in enumerate(theirs):
        cx, cy = (box[0] + box[2]) / 2, (box[1] + box[3]) / 2
        grid[(c, int(cx // cell), int(cy // cell))].append(index)
    used = set()
    placed = 0
    errors = []
    for c, oa in ours:
        cx, cy = (oa[0] + oa[2]) / 2, (oa[1] + oa[3]) / 2
        gx, gy = int(cx // cell), int(cy // cell)
        best = None
        for i in (gx - 1, gx, gx + 1):
            for j in (gy - 1, gy, gy + 1):
                for index in grid.get((c, i, j), ()):
                    if index in used:
                        continue
                    ob = theirs[index][1]
                    dx = abs(cx - (ob[0] + ob[2]) / 2)
                    dy = abs(cy - (ob[1] + ob[3]) / 2)
                    fits = dx <= 0.5 * max(ob[2] - ob[0], 1.0) + 0.5
                    fits = fits and dy <= 0.6 * max(ob[3] - ob[1], 1.0)
                    if fits and (best is None or dx + dy < best[0]):
                        best = (dx + dy, index, dx)
        if best is not None:
            used.add(best[1])
            placed += 1
            errors.append(best[2])
    return matched, placed, errors


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("dump")
    parser.add_argument("--worst", type=int, default=25)
    parser.add_argument("--rows", help="write the per-file scores here, one JSON line each")
    args = parser.parse_args()

    rows = []
    ms_ours = ms_mupdf = 0.0
    for line in open(args.dump):
        entry = json.loads(line)
        path = entry["file"]
        started = time.perf_counter()
        try:
            theirs_pages = oracle(path)
        except Exception as e:  # MuPDF refuses: the file is not a PDF worth scoring
            rows.append({"file": path, "skip": f"mupdf: {e}"[:60]})
            continue
        ms_mupdf += (time.perf_counter() - started) * 1000
        ms_ours += entry["ms"]
        theirs = [flat(p) for p in theirs_pages]
        total = sum(len(p) for p in theirs)
        if "error" in entry:
            rows.append({"file": path, "error": entry["error"][:70], "total": total})
            continue
        ours_pages = [[] for _ in theirs]
        for page, c, x0, y0, x1, y1 in entry["glyphs"]:
            if page < len(ours_pages):
                ours_pages[page].append((c, x0, y0, x1, y1))
        ours = [flat(p) for p in ours_pages]
        found = sum(len(p) for p in ours)
        matched = placed = 0
        errors = []
        for o, t in zip(ours, theirs, strict=True):
            m, p, e = score(o, t)
            matched += m
            placed += p
            errors += e
        rows.append(
            {
                "file": path,
                "total": total,
                "found": found,
                "matched": matched,
                "placed": placed,
                "dx": statistics.median(errors) if errors else 0.0,
            }
        )

    if args.rows:
        with open(args.rows, "w") as out:
            out.writelines(json.dumps(r) + "\n" for r in rows)
    scored = [r for r in rows if "skip" not in r]
    texted = [r for r in scored if r.get("total", 0) > 0]
    opened = [r for r in scored if "error" not in r]
    total = sum(r["total"] for r in texted)
    matched = sum(r.get("matched", 0) for r in texted)
    found = sum(r.get("found", 0) for r in texted if "error" not in r)
    placed = sum(r.get("placed", 0) for r in texted)
    print(f"files {len(rows)}, MuPDF opens {len(scored)}, engine opens {len(opened)}")
    print(f"with text (MuPDF) {len(texted)}")
    print(f"text recall    {matched / max(total, 1):.1%}  ({matched}/{total} characters)")
    print(f"text precision {matched / max(found, 1):.1%}")
    print(f"placed         {placed / max(matched, 1):.1%} of them on the box MuPDF gives them")
    good = [
        r
        for r in texted
        if "error" not in r
        and r["matched"] >= 0.98 * r["total"]
        and r["placed"] >= 0.98 * max(r["matched"], 1)
    ]
    print(f"files read as MuPDF reads them (>=98% text, >=98% placed): {len(good)}/{len(texted)}")
    print(f"time: engine {ms_ours / 1000:.1f} s, MuPDF {ms_mupdf / 1000:.1f} s")

    errors = {}
    for r in scored:
        if "error" in r:
            errors.setdefault(
                r["error"].split(":")[0] + ":" + r["error"].split(":")[1][:40]
                if ":" in r["error"]
                else r["error"],
                [],
            ).append(r["file"])
    print("\nrefusals:")
    for k, v in sorted(errors.items(), key=lambda kv: -len(kv[1])):
        print(f"  {len(v):>4}  {k}")

    def badness(r):
        if "error" in r:
            return 2.0
        recall = r["matched"] / max(r["total"], 1)
        place = r["placed"] / max(r["matched"], 1)
        return (1 - recall) + (1 - place)

    print(f"\nworst {args.worst}:")
    for r in sorted(texted, key=badness, reverse=True)[: args.worst]:
        name = r["file"].split("/")[-1][:50]
        if "error" in r:
            print(f"  {name:<50} REFUSED {r['error'][:50]}")
        else:
            print(
                f"  {name:<50} recall {r['matched'] / max(r['total'], 1):6.1%} "
                f"placed {r['placed'] / max(r['matched'], 1):6.1%} dx {r['dx']:.1f}"
            )


if __name__ == "__main__":
    sys.exit(main())
