"""Apply/restore the sole reviewed SQL patch in the disposable CI checkout."""
import hashlib
import sys
from pathlib import Path

ORIGINAL_SHA256 = '4e633ba41b4c96794f7625b7948c0a0ab197f6ee9d4680fd6a470ac9e68264f0'
PATCHED_SHA256 = '1a58353857de0b145f2cd4a96405dd84b163899710b01243678e099a5a3cc13d'
RELATIVE = 'packages/lj-store/migrations/0204_text_filiation.sql'

def digest(data):
    return hashlib.sha256(data).hexdigest()

def run(mode, checkout):
    target = checkout / RELATIVE
    backup = checkout / 'smoke-evidence/0204-original.sql'
    if mode == 'apply':
        original = target.read_bytes()
        if digest(original) != ORIGINAL_SHA256:
            raise SystemExit('Refusing SQL patch: original migration checksum differs')
        patched = Path(__file__).with_name('0204_text_filiation_empty_corpus.sql').read_bytes()
        if digest(patched) != PATCHED_SHA256:
            raise SystemExit('Refusing SQL patch: reviewed replacement checksum differs')
        backup.parent.mkdir(parents=True, exist_ok=True)
        backup.write_bytes(original)
        target.write_bytes(patched)
        print(f'MIGRATION_PATCH=0204; ORIGINAL_SHA256={ORIGINAL_SHA256}; PATCHED_SHA256={PATCHED_SHA256}')
    elif mode == 'restore':
        if not backup.exists():
            return
        if digest(target.read_bytes()) not in (ORIGINAL_SHA256, PATCHED_SHA256):
            raise SystemExit('Refusing restore: migration changed outside the reviewed patch')
        original = backup.read_bytes()
        if digest(original) != ORIGINAL_SHA256:
            raise SystemExit('Refusing restore: original backup checksum differs')
        target.write_bytes(original)
    else:
        raise SystemExit('Usage: apply_migration_patch.py apply|restore CHECKOUT')

if __name__ == '__main__':
    if len(sys.argv) != 3:
        raise SystemExit('Usage: apply_migration_patch.py apply|restore CHECKOUT')
    run(sys.argv[1], Path(sys.argv[2]).resolve())
