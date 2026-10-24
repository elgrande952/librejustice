#!/usr/bin/env bash
set -Eeuo pipefail
: "${LJ_SOURCE_DIR:?pinned disposable LibreJustice source checkout required}"
LJ_KIT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
LJ_SOURCE_DIR="$(cd -- "$LJ_SOURCE_DIR" && pwd)"
[[ "$(git -C "$LJ_SOURCE_DIR" rev-parse HEAD)" == ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75 ]]
[[ -z "$(git -C "$LJ_SOURCE_DIR" status --porcelain)" ]]
cd "$LJ_SOURCE_DIR"
LJ_CONTAINER="lj-smoke-lexical-${GITHUB_RUN_ID:-local}-$$"
LJ_API_PID=''
mkdir -p lexical-evidence packages/lj-api/examples
cleanup() {
 LJ_STATUS=$?
 if [[ -n "$LJ_API_PID" ]]; then kill "$LJ_API_PID" 2>/dev/null || true; fi
 if (( LJ_STATUS != 0 )); then
  docker logs --tail 100 "$LJ_CONTAINER" || true
  test ! -f lexical-evidence/api.log || tail -100 lexical-evidence/api.log
 fi
 docker rm -fv "$LJ_CONTAINER" >/dev/null 2>&1 || true
 return "$LJ_STATUS"
}
trap cleanup EXIT
docker build -f "$LJ_KIT_DIR/Dockerfile.pg" -t "lj-lexical-pg:${GITHUB_RUN_ID:-local}" "$LJ_KIT_DIR"
LJ_TEST_PASSWORD="$(python3 -c 'import secrets;print(secrets.token_hex(24))')"
export LJ_ACCESS_TOKEN="$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
docker run -d --name "$LJ_CONTAINER" --memory=2g --cpus=2 --shm-size=1g \
 -p 127.0.0.1:55432:5432 -e POSTGRES_USER=librejustice -e POSTGRES_DB=lj_lexical \
 -e POSTGRES_PASSWORD="$LJ_TEST_PASSWORD" "lj-lexical-pg:${GITHUB_RUN_ID:-local}" >/dev/null
for ((n=0;n<60;n++)); do
 [[ "$(docker inspect --format '{{.State.Running}}' "$LJ_CONTAINER")" == true ]]
 if docker exec "$LJ_CONTAINER" pg_isready -h 127.0.0.1 -U librejustice -d lj_lexical >/dev/null 2>&1; then break; fi
 sleep 1
done
docker exec "$LJ_CONTAINER" pg_isready -h 127.0.0.1 -U librejustice -d lj_lexical
python3 "$LJ_KIT_DIR/runtime/apply_patches.py" "$LJ_SOURCE_DIR"
cp "$LJ_KIT_DIR/runtime/sol_fable_lexical.rs" packages/lj-api/examples/sol_fable_lexical.rs
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
cargo fetch --locked
python3 "$LJ_KIT_DIR/runtime/audit_build_scripts.py"
cargo test --locked -p lj-api --example sol_fable_lexical
cargo build --locked -p lj-api --example sol_fable_lexical
export LIBREJUSTICE_DB_URL=postgresql://librejustice@127.0.0.1:55432/lj_lexical
export LIBREJUSTICE_PG_PASSWORD="$LJ_TEST_PASSWORD" LJ_ALLOW_SCHEMA_MIGRATIONS=1
export LIBREJUSTICE_API_BIND_HOST=127.0.0.1 LIBREJUSTICE_API_BIND_PORT=18300
export LIBREJUSTICE_API_POOL_MIN=1 LIBREJUSTICE_API_POOL_MAX=8
export LIBREJUSTICE_EMBED_BACKEND=disabled
# No provider credentials or endpoint keys enter the API subprocess.
env -i PATH="$PATH" LANG=C.UTF-8 LJ_ACCESS_TOKEN="$LJ_ACCESS_TOKEN" \
 LIBREJUSTICE_DB_URL="$LIBREJUSTICE_DB_URL" LIBREJUSTICE_PG_PASSWORD="$LIBREJUSTICE_PG_PASSWORD" \
 LIBREJUSTICE_API_BIND_HOST=127.0.0.1 LIBREJUSTICE_API_BIND_PORT=18300 \
 LIBREJUSTICE_API_POOL_MIN=1 LIBREJUSTICE_API_POOL_MAX=8 \
 LJ_ALLOW_SCHEMA_MIGRATIONS=1 LJ_BOOTSTRAP_CORPUS="$LJ_KIT_DIR/corpus/initial-judilibre.jsonl" \
 ./target/debug/examples/sol_fable_lexical boot >lexical-evidence/api.log 2>&1 &
LJ_API_PID=$!
for ((n=0;n<180;n++)); do
 if curl -fsS --max-time 2 http://127.0.0.1:18300/health >/dev/null 2>&1; then break; fi
 kill -0 "$LJ_API_PID" 2>/dev/null
 sleep 1
done
python3 "$LJ_KIT_DIR/tests/check_runtime.py"
# Re-import the same snapshots; all ten must be skipped, no duplicate rows.
./target/debug/examples/sol_fable_lexical import "$LJ_KIT_DIR/corpus/initial-judilibre.jsonl" | tee lexical-evidence/reimport.log
python3 -c 'from pathlib import Path; assert "IMPORT_CREATED=0; IMPORT_UPDATED=0; IMPORT_SKIPPED=10;" in Path("lexical-evidence/reimport.log").read_text()'
docker exec "$LJ_CONTAINER" psql -U librejustice -d lj_lexical -Atc "SELECT count(*) FROM decision_chunks" | tee lexical-evidence/chunks.txt
[[ "$(cat lexical-evidence/chunks.txt)" == 0 ]]
docker exec "$LJ_CONTAINER" psql -U librejustice -d lj_lexical -Atc "SELECT extname || '=' || extversion FROM pg_extension ORDER BY extname"
docker stats --no-stream --format '{{json .}}' "$LJ_CONTAINER"
python3 -c 'from pathlib import Path; print("\n".join(s for s in Path("lexical-evidence/api.log").read_text().splitlines() if s.startswith(("MIGRATIONS_APPLIED=","IMPORT_CREATED=","SERVICE="))))'
ldd ./target/debug/examples/sol_fable_lexical
du -h ./target/debug/examples/sol_fable_lexical
