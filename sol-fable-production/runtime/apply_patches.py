#!/usr/bin/env python3
"""Apply only reviewed LibreJustice files, with both source and destination SHA checks."""
import argparse, hashlib, json
from pathlib import Path

HERE = Path(__file__).resolve().parent

def digest(data):
    return hashlib.sha256(data).hexdigest()

def apply(root):
    manifest = json.loads((HERE / "patch-manifest.json").read_text())
    writes = []
    for row in manifest["files"]:
        path = root / row["path"]
        previous = path.read_bytes()
        replacement = (HERE / "patched" / row["path"]).read_bytes()
        if digest(replacement) != row["patched_sha256"]:
            raise ValueError(f"bad patch content: {row['path']}")
        if digest(previous) == row["patched_sha256"]:
            continue
        if digest(previous) != row["original_sha256"]:
            raise ValueError(f"unexpected upstream bytes: {row['path']}")
        writes.append((path, replacement))
    # Check all files before the first write.
    for path, content in writes:
        path.write_bytes(content)
    print(json.dumps({"reviewed_patches": len(manifest["files"]), "applied":len(writes), "upstream":manifest["upstream"]}))

if __name__ == "__main__":
    p=argparse.ArgumentParser();p.add_argument("root",type=Path);args=p.parse_args()
    apply(args.root.resolve())
