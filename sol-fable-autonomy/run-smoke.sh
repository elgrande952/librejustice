#!/usr/bin/env bash
set -Eeuo pipefail
: "${LJ_SOURCE_DIR:?Set LJ_SOURCE_DIR to the pinned upstream checkout}"
LJ_KIT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LJ_SOURCE_DIR="$(cd -- "$LJ_SOURCE_DIR" && pwd)"
LJ_EXPECTED=ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75
[[ "$(git -C "$LJ_SOURCE_DIR" rev-parse HEAD)" == "$LJ_EXPECTED" ]]
[[ -z "$(git -C "$LJ_SOURCE_DIR" status --porcelain)" ]]
[[ ! -e "$LJ_SOURCE_DIR/packages/lj-api/examples/sol_fable_smoke.rs" ]]
cd "$LJ_SOURCE_DIR"
mkdir -p smoke-evidence packages/lj-api/examples
cp "$LJ_KIT_DIR/smoke_api.rs" packages/lj-api/examples/sol_fable_smoke.rs
LJ_CONTAINER="lj-smoke-${GITHUB_RUN_ID:-local}-$$"
LJ_API_PID=''
cleanup() {
  LJ_EXIT_STATUS=$?
  if [[ -n "$LJ_API_PID" ]]; then kill "$LJ_API_PID" 2>/dev/null || true; fi
  docker logs "$LJ_CONTAINER" > smoke-evidence/database.log 2>&1 || true
  if (( LJ_EXIT_STATUS != 0 )); then
    echo 'Database state (no environment or credentials):'
    docker inspect --format '{{json .State}}' "$LJ_CONTAINER" || true
    echo 'Database startup log (last 120 lines):'
    tail -n 120 smoke-evidence/database.log || true
    if [[ -f smoke-evidence/api.log ]]; then
      echo 'API log (last 80 lines):'
      tail -n 80 smoke-evidence/api.log || true
    fi
  fi
  docker rm -fv "$LJ_CONTAINER" >/dev/null 2>&1 || true
  python3 "$LJ_KIT_DIR/apply_migration_patch.py" restore "$LJ_SOURCE_DIR" || true
  rm -f packages/lj-api/examples/sol_fable_smoke.rs
  return "$LJ_EXIT_STATUS"
}
trap cleanup EXIT
# Database image uses fixed upstream versions; it is never pushed to a registry.
docker build -f infra/Dockerfile.pg -t "lj-autonomy-smoke:${GITHUB_RUN_ID:-local}" . 2>&1 | tee smoke-evidence/database-build.log
LJ_TEST_PASSWORD="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
docker run -d --name "$LJ_CONTAINER" --memory=2g --cpus=2 --shm-size=1g \
  -p 127.0.0.1:55432:5432 \
  -e POSTGRES_USER=librejustice -e POSTGRES_DB=lj_smoke \
  -e POSTGRES_PASSWORD="$LJ_TEST_PASSWORD" \
  "lj-autonomy-smoke:${GITHUB_RUN_ID:-local}" \
  postgres -c 'shared_preload_libraries=pg_search,vchord,pg_cron,pg_stat_statements,pg_prewarm' \
  -c cron.database_name=postgres -c shared_buffers=256MB \
  -c max_connections=30 -c default_toast_compression=lz4 \
  -c paradedb.planner_warnings=off >/dev/null
for ((attempt=0; attempt<60; attempt++)); do
  if [[ "$(docker inspect --format '{{.State.Running}}' "$LJ_CONTAINER")" != true ]]; then
    echo 'Database stopped before readiness; see cleanup diagnostics.'
    exit 1
  fi
  if docker exec "$LJ_CONTAINER" pg_isready -h 127.0.0.1 -U librejustice -d lj_smoke >/dev/null 2>&1; then break; fi
  sleep 1
done
docker exec "$LJ_CONTAINER" pg_isready -h 127.0.0.1 -U librejustice -d lj_smoke
# The sole upstream deviation is a checksum-guarded empty-corpus SQL migration fix.
python3 "$LJ_KIT_DIR/apply_migration_patch.py" apply "$LJ_SOURCE_DIR"
# Build only after proving that the disposable database can start.
# lj-ingest/build.rs downloads a model tokenizer and is intentionally excluded.
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
cargo fetch --locked
python3 "$LJ_KIT_DIR/audit_build_scripts.py"
cargo build --locked -p lj-api --example sol_fable_smoke 2>&1 | tee smoke-evidence/build.log
# env -i prevents inherited provider credentials; no network model/backend or SMTP configured.
env -i PATH="$PATH" LANG=C.UTF-8 \
  LJ_SMOKE_BACKFILL_SQL="$LJ_KIT_DIR/backfill_text_filiation.sql" \
  LIBREJUSTICE_DB_URL=postgresql://librejustice@127.0.0.1:55432/lj_smoke \
  LIBREJUSTICE_PG_PASSWORD="$LJ_TEST_PASSWORD" \
  LIBREJUSTICE_EMBED_BACKEND=dummy \
  LIBREJUSTICE_API_BIND_HOST=127.0.0.1 LIBREJUSTICE_API_BIND_PORT=18300 \
  LIBREJUSTICE_API_POOL_MIN=1 LIBREJUSTICE_API_POOL_MAX=8 \
  LIBREJUSTICE_API_VERSION="$LJ_EXPECTED" \
  LIBREJUSTICE_API_DEPLOYMENT_ENVIRONMENT=dev \
  LIBREJUSTICE_API_PUBLIC_BASE_URL=http://127.0.0.1:18300 \
  LIBREJUSTICE_API_WEB_BASE_URL=http://127.0.0.1:18300 \
  LIBREJUSTICE_API_MCP_ALLOWED_HOSTS=127.0.0.1,localhost \
  ./target/debug/examples/sol_fable_smoke >smoke-evidence/api.log 2>&1 &
LJ_API_PID=$!
for ((attempt=0; attempt<180; attempt++)); do
  if curl -fsS --max-time 2 http://127.0.0.1:18300/internal/health >/dev/null 2>&1; then break; fi
  if ! kill -0 "$LJ_API_PID" 2>/dev/null; then cat smoke-evidence/api.log; exit 1; fi
  sleep 1
done
python3 "$LJ_KIT_DIR/check_api.py"
python3 -c 'from pathlib import Path; print("\n".join(line for line in Path("smoke-evidence/api.log").read_text().splitlines() if line.startswith(("MIGRATIONS_APPLIED=", "EMPTY_CORPUS_BACKFILL_PASSES=", "SYNTHETIC_FIXTURES="))))'
docker exec "$LJ_CONTAINER" psql -U librejustice -d lj_smoke -Atc \
  "SELECT extname || '=' || extversion FROM pg_extension ORDER BY extname" >smoke-evidence/extensions.txt
docker stats --no-stream --format '{{json .}}' "$LJ_CONTAINER" >smoke-evidence/database-resource-snapshot.json

cat smoke-evidence/extensions.txt
cat smoke-evidence/database-resource-snapshot.json
