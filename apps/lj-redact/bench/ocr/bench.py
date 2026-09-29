# /// script
# requires-python = ">=3.12"
# dependencies = ["pymupdf>=1.28", "pillow", "numpy", "scipy", "rapidfuzz"]
# ///
"""Robustness of the redactor's OCR to degraded scans, and what preprocessing buys.

    uv run bench.py prepare PDF_DIR [--work DIR]    rasterise text PDFs, their text layer as truth
    uv run bench.py sweep [--engine native|wasm] [--degrade a,b] [--pre x,y+z]

The work directory holds the text of the source PDFs: delete it after the run.
"""

import argparse
import json
import math
import os
import resource
import shutil
import statistics
import subprocess
import tempfile
import unicodedata
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import pymupdf
from PIL import Image, ImageDraw, ImageFilter, ImageFont
from rapidfuzz.distance import Levenshtein
from scipy import ndimage

HERE = Path(__file__).parent
DPI = 144
VENDOR = Path(os.environ.get("LIBREJUSTICE_STATE_DIR", "")) / "vendor"


def prepare(corpus, work, pages):
    work.mkdir(parents=True, exist_ok=True)
    truth = {}
    for pdf in sorted(Path(corpus).glob("*.pdf")):
        if len(truth) == pages:
            break
        try:
            page = pymupdf.open(pdf)[0]
        except Exception:
            continue
        words = [w for w in page.get_text("words") if w[4].strip()]
        if len(words) < 50:
            continue
        key = f"{len(truth):04d}"
        pix = page.get_pixmap(dpi=DPI, colorspace=pymupdf.csGRAY)
        Image.frombytes("L", (pix.width, pix.height), pix.samples).save(work / f"{key}.png")
        scale = DPI / 72
        truth[key] = [[w[4], *(c * scale for c in w[:4])] for w in words]
    (work / "truth.json").write_text(json.dumps(truth))
    print(len(truth), "pages in", work)


def identity(x, y):
    return x, y


def rotate(image, degrees):
    """The page turned counterclockwise, with the maps of a point there and back."""
    out = image.rotate(degrees, resample=Image.BICUBIC, expand=True, fillcolor=255)
    (w, h), (W, H), a = image.size, out.size, math.radians(degrees)
    c, s = math.cos(a), math.sin(a)

    def there(x, y):
        dx, dy = x - w / 2, y - h / 2
        return W / 2 + dx * c + dy * s, H / 2 - dx * s + dy * c

    def back(x, y):
        dx, dy = x - W / 2, y - H / 2
        return w / 2 + dx * c - dy * s, h / 2 + dx * s + dy * c

    return out, there, back


def resample(image, dpi):
    s = dpi / DPI
    out = image.resize((round(image.width * s), round(image.height * s)), Image.LANCZOS)
    return out, lambda x, y: (x * s, y * s)


def arr(image):
    return np.asarray(image, dtype=np.float32)


def img(a):
    return Image.fromarray(np.clip(a, 0, 255).astype(np.uint8))


def jpeg(image, quality):
    with tempfile.NamedTemporaryFile(suffix=".jpg") as f:
        image.save(f.name, quality=quality)
        return Image.open(f.name).copy()


def noise(image, sigma, rng):
    return img(arr(image) + rng.normal(0, sigma, (image.height, image.width)))


def shadow(image):
    h, w = image.height, image.width
    y, x = np.mgrid[0:h, 0:w]
    light = 1.0 - 0.55 * (x / w) ** 1.5 * (0.6 + 0.4 * y / h)
    return img((arr(image) * 0.8 + 30) * light)


def photocopy(image, rng):
    a = arr(image.filter(ImageFilter.GaussianBlur(0.9)))
    a = np.where(a + rng.normal(0, 25, a.shape) < 150, 0, 255).astype(np.float32)
    speck = rng.random(a.shape)
    a[speck < 0.004] = 0
    a[speck > 0.996] = 255
    return img(a)


def smooth_noise(shape, rng, sigma):
    field = ndimage.gaussian_filter(rng.normal(0, 1, shape), sigma)
    return field / max(float(np.abs(field).max()), 1e-9)


def on_paper(image, paper):
    """The ink of the page laid on another background."""
    return img(45 + (paper - 45) * arr(image) / 255)


def tinted(image, rng):
    shape = (image.height, image.width)
    return on_paper(image, 212 + 14 * smooth_noise(shape, rng, 40) + rng.normal(0, 6, shape))


def patterned(image):
    h, w = image.height, image.width
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    lines = (0.5 + 0.5 * np.sin(x * 0.21 + 7 * np.sin(y * 0.018))) ** 6
    mark = Image.new("L", (w, h), 0)
    font = ImageFont.load_default(size=w // 7)
    ImageDraw.Draw(mark).text((w * 0.08, h * 0.4), "COPIE", fill=255, font=font)
    mark = arr(mark.rotate(35, fillcolor=0)) / 255
    return on_paper(image, 245 - 70 * lines - 60 * mark)


def homography(src, dst):
    rows = []
    for (x, y), (u, v) in zip(src, dst, strict=True):
        rows.append([x, y, 1, 0, 0, 0, -u * x, -u * y, -u])
        rows.append([0, 0, 0, x, y, 1, -v * x, -v * y, -v])
    return np.linalg.svd(np.array(rows))[2][-1].reshape(3, 3)


def photographed(image, rng):
    """The page shot on a dark table: perspective, uneven light, a phone's blur and compression."""
    w, h = image.size
    W, H = int(w * 1.16), int(h * 1.12)
    src = [(0, 0), (w, 0), (w, h), (0, h)]
    dst = [
        (W * fx + rng.uniform(-0.035, 0.035) * W, H * fy + rng.uniform(-0.03, 0.03) * H)
        for fx, fy in ((0.07, 0.05), (0.93, 0.06), (0.95, 0.95), (0.05, 0.94))
    ]
    forward, inverse = homography(src, dst), homography(dst, src)
    coeffs = (inverse / inverse[2, 2]).flatten()[:8]
    page = shadow(tinted(image, rng)).transform(
        (W, H), Image.PERSPECTIVE, coeffs, Image.BICUBIC, fillcolor=0
    )
    mask = Image.new("L", (w, h), 255).transform(
        (W, H), Image.PERSPECTIVE, coeffs, Image.BILINEAR, fillcolor=0
    )
    table = img(70 + 25 * smooth_noise((H, W), rng, 60))
    table.paste(page, (0, 0), mask)
    shot = noise(table.filter(ImageFilter.GaussianBlur(1.0)), 12, rng)

    def there(x, y):
        u, v, t = forward @ (x, y, 1)
        return u / t, v / t

    return jpeg(shot, 50), there


def degrade(name, image, rng):
    """The page as a scan would give it, and where a point of the clean page lands."""
    match name:
        case "clean":
            return image, identity
        case "blur":
            return image.filter(ImageFilter.GaussianBlur(1.4)), identity
        case "noise":
            return noise(image, 40, rng), identity
        case "jpeg":
            return jpeg(image, 12), identity
        case "shadow":
            return shadow(image), identity
        case "photocopy":
            return photocopy(image, rng), identity
        case "dpi100" | "dpi72":
            return resample(image, int(name[3:]))
        case "skew1" | "skew3" | "skew6":
            out, there, _ = rotate(image, int(name[4:]))
            return out, there
        case "fax":
            low, there = resample(image, 100)
            return photocopy(low, rng), there
        case "fax200":
            high, there = resample(image, 200)
            return photocopy(high, rng), there
        case "faxpdf":
            low, _ = resample(image, 100)
            return photocopy(low, rng).resize(image.size, Image.NEAREST), identity
        case "copypdf":
            high, _ = resample(image, 300)
            return photocopy(high, rng).resize(image.size, Image.BOX), identity
        case "bilevel":
            return img(np.where(arr(image) < 128, 0, 255)), identity
        case "tinted":
            return tinted(image, rng), identity
        case "pattern":
            return patterned(image), identity
        case "photo":
            return photographed(image, rng)
        case "dim":
            return jpeg(noise(img(95 + arr(image) * 0.33), 22, rng), 40), identity
        case "scan":
            out, there, _ = rotate(shadow(image.filter(ImageFilter.GaussianBlur(0.9))), 2)
            return jpeg(noise(out, 20, rng), 30), there
    raise SystemExit(f"unknown degradation {name}")


def flatten(image):
    a = arr(image)
    background = ndimage.uniform_filter(ndimage.grey_closing(a, size=(25, 25)), 25)
    out = a / np.maximum(background, 1) * 255
    lo, hi = np.percentile(out, (1, 99))
    return img((out - lo) / max(hi - lo, 1) * 255)


def skew_angle(image):
    ink = (arr(image.resize((image.width // 2, image.height // 2))) < 128).astype(np.float32)
    scores = {
        float(d): float(
            np.square(np.diff(ndimage.rotate(ink, d, reshape=False, order=0).sum(axis=1))).sum()
        )
        for d in np.arange(-8, 8.01, 0.25)
    }
    return max(scores, key=scores.get)


def preprocess(name, image):
    """The page handed to the reader, its parameters, and where a point it returns lies on the page."""
    params, backs = {}, []
    for step in name.split("+"):
        match step:
            case "none":
                pass
            case "sauvola":
                params["thresholding_method"] = "2"
            case "otsu-tiled":
                params["thresholding_method"] = "1"
            case "flatten":
                image = flatten(image)
            case "denoise":
                image = image.filter(ImageFilter.MedianFilter(3))
            case "deskew":
                angle = skew_angle(image)
                if abs(angle) >= 0.25:
                    image, _, back = rotate(image, angle)
                    backs.append(back)
            case s if s.startswith("despeckle"):
                image = despeckle(image, int(s[9:]))
            case "close":
                ink = arr(image) < 128
                image = img(np.where(ndimage.binary_dilation(ink, np.ones((2, 2))), 0, 255))
            case s if s.startswith("smooth"):
                image = image.filter(ImageFilter.GaussianBlur(float(s[6:])))
            case s if s.startswith("aa"):
                k, sigma = int(s[2]), float(s[3:])
                blurred = image.filter(ImageFilter.GaussianBlur(sigma))
                image = blurred.resize((image.width * k, image.height * k), Image.BICUBIC)
                backs.append(lambda x, y, k=k: (x / k, y / k))
            case s if s.startswith("up"):
                k = int(s[2:])
                image = image.resize((image.width * k, image.height * k), Image.BICUBIC)
                backs.append(lambda x, y, k=k: (x / k, y / k))
            case s if s.startswith("gamma"):
                image = img(255 * (arr(image) / 255) ** float(s[5:]))
            case s if s.startswith("psm"):
                params["tessedit_pageseg_mode"] = s[3:]
            case _:
                raise SystemExit(f"unknown preprocessing {step}")

    def back(x, y):
        for step in reversed(backs):
            x, y = step(x, y)
        return x, y

    return image, params, lambda boxes: [box_through(back, b) for b in boxes]


def despeckle(image, size):
    """Ink specks and paper holes of at most `size` pixels removed."""
    ink = arr(image) < 128
    for value in (True, False):
        labels, _ = ndimage.label(ink == value, np.ones((3, 3)))
        small = np.bincount(labels.ravel()) <= size
        small[0] = False
        ink[small[labels]] = not value
    return img(np.where(ink, 0, 255))


def prepare_scan():
    return Path(
        os.environ.get("LJ_PREPARE_SCAN", HERE.parents[3] / "target/release/examples/prepare_scan")
    )


def shipped(degraded):
    """What `lj-redact-core::scan` makes of the pages, and the time it took per page."""
    listing = []
    for source in degraded:
        source_pgm = source.with_suffix(".pgm")
        Image.open(source).save(source_pgm)
        listing.append(f"{source_pgm} {source.with_suffix('.ready.pgm')}")
    list_file = degraded[0].parent / "prepare.txt"
    list_file.write_text("\n".join(listing) + "\n")
    run = subprocess.run(
        [prepare_scan(), "prepare", list_file], capture_output=True, text=True, check=True
    )
    for source in degraded:
        Image.open(source.with_suffix(".ready.pgm")).save(source)

    def back(source):
        def boxes(found):
            run = subprocess.run(
                [prepare_scan(), "boxes", source.with_suffix(".pgm")],
                input=json.dumps(found),
                capture_output=True,
                text=True,
                check=True,
            )
            return json.loads(run.stdout)

        return boxes

    return {source: back(source) for source in degraded}, statistics.mean(
        map(float, run.stdout.split())
    )


def box_through(move, box):
    points = [move(x, y) for x in (box[0], box[2]) for y in (box[1], box[3])]
    xs, ys = [p[0] for p in points], [p[1] for p in points]
    return [min(xs), min(ys), max(xs), max(ys)]


def read_native(paths, params, tessdata, work):
    flags = [a for k, v in params.items() for a in ("-c", f"{k}={v}")]
    command = [
        "tesseract",
        "-",
        "-",
        "--tessdata-dir",
        str(tessdata),
        "-l",
        "fra",
        "--oem",
        "1",
        "--psm",
        "6",
        "-c",
        "tessedit_create_tsv=1",
        *flags,
    ]

    def one(path):
        run = subprocess.run(
            [*command[:1], path, *command[2:]],
            capture_output=True,
            text=True,
            env={**os.environ, "OMP_THREAD_LIMIT": "1"},
        )
        words = []
        for row in run.stdout.splitlines()[1:]:
            f = row.split("\t")
            if len(f) == 12 and f[0] == "5" and f[11].strip():
                x, y, w, h = map(int, f[6:10])
                words.append([f[11], x, y, x + w, y + h])
        return path, words

    with ThreadPoolExecutor(os.cpu_count()) as pool:
        return dict(pool.map(one, paths))


def read_wasm(paths, params, tessdata, work):
    listing, out = Path(paths[0]).parent / "list.txt", Path(paths[0]).parent / "read.jsonl"
    listing.write_text("\n".join(paths) + "\n")
    subprocess.run(
        ["node", "read.mjs", str(listing), str(out), str(tessdata)],
        check=True,
        cwd=HERE,
        env={**os.environ, "LJ_OCR_PARAMS": json.dumps(params)},
    )
    return {r["path"]: r["words"] for r in map(json.loads, out.read_text().splitlines())}


def norm(text):
    return unicodedata.normalize("NFC", text).replace("\u2019", "'").strip(".,;:()[]«»\"'")


def inside(a, b):
    x0, y0, x1, y1 = max(a[0], b[0]), max(a[1], b[1]), min(a[2], b[2]), min(a[3], b[3])
    return max(0, x1 - x0) * max(0, y1 - y0) / max((a[2] - a[0]) * (a[3] - a[1]), 1e-9)


def cpu_seconds():
    """The processor time of the finished readers: a shared machine stretches the wall clock."""
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    return usage.ru_utime + usage.ru_stime


def score(work, reader, degradation, pre, tessdata):
    truth = json.loads((work / "truth.json").read_text())
    rng = np.random.default_rng(7)
    staged = Path(tempfile.mkdtemp(dir=work))
    cases, params, prep_ms = {}, {}, 0.0
    for key, words in truth.items():
        degraded, there = degrade(degradation, Image.open(work / f"{key}.png"), rng)
        image, params, back = preprocess("none" if pre == "shipped" else pre, degraded)
        path = staged / f"{key}.png"
        image.save(path)
        cases[str(path)] = ([[w[0], *box_through(there, w[1:])] for w in words], back)
    if pre == "shipped":
        backs, prep_ms = shipped([Path(p) for p in cases])
        cases = {p: (words, backs[Path(p)]) for p, (words, _) in cases.items()}
    started = cpu_seconds()
    found = reader(list(cases), params, tessdata, work)
    read_s = cpu_seconds() - started
    errors = chars = hits = total = names = names_hit = placed = 0
    for path, (words, back) in cases.items():
        mapped = back([w[1:] for w in found[path]])
        got = [[w[0], *b] for w, b in zip(found[path], mapped, strict=True)]
        ref = " ".join(w[0] for w in words)
        errors += Levenshtein.distance(ref, " ".join(w[0] for w in got))
        chars += len(ref)
        boxes = {}
        for w in got:
            boxes.setdefault(norm(w[0]), []).append(w[1:])
        for w in words:
            text = norm(w[0])
            candidates = boxes.get(text, [])
            total += 1
            hits += bool(candidates)
            if len(text) >= 3 and text[0].isupper():
                names += 1
                names_hit += bool(candidates)
            placed += any(inside(b, w[1:]) >= 0.8 for b in candidates)
    shutil.rmtree(staged)
    return errors / chars, hits / total, names_hit / names, placed / total, prep_ms, read_s


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("command", choices=["prepare", "sweep"])
    parser.add_argument("corpus", nargs="?")
    parser.add_argument("--work", type=Path, default=Path("/dev/shm/ocr-bench"))
    parser.add_argument("--pages", type=int, default=80)
    parser.add_argument("--engine", choices=["native", "wasm"], default="native")
    parser.add_argument(
        "--degrade",
        default="clean,blur,noise,jpeg,shadow,photocopy,dpi100,dpi72,skew1,skew3,skew6,fax,fax200,faxpdf,bilevel,copypdf,scan,tinted,pattern,photo,dim",
    )
    parser.add_argument("--pre", default="none,sauvola,otsu-tiled,flatten,deskew,up2")
    parser.add_argument("--tessdata", type=Path)
    args = parser.parse_args()
    if args.command == "prepare":
        return prepare(args.corpus, args.work, args.pages)
    tessdata = args.tessdata or next(VENDOR.glob("tesseract-*/fra.traineddata")).parent
    reader = {"native": read_native, "wasm": read_wasm}[args.engine]
    print(
        f"{'degradation':12} {'preprocessing':24} {'CER':>6} {'words':>6} {'names':>6} {'placed':>7} {'prep ms':>8} {'read cpu s':>10}",
        flush=True,
    )
    for degradation in args.degrade.split(","):
        for pre in args.pre.split(","):
            cer, words, names, placed, prep, read = score(
                args.work, reader, degradation, pre, tessdata
            )
            print(
                f"{degradation:12} {pre:24} {cer:6.1%} {words:6.1%} {names:6.1%} {placed:7.1%} {prep:8.0f} {read:10.1f}",
                flush=True,
            )


if __name__ == "__main__":
    main()
