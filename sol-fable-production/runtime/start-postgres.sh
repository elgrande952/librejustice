#!/usr/bin/env bash
set -Eeuo pipefail
# The volume must be dedicated to this PostgreSQL instance.
exec docker-entrypoint.sh postgres \
  -c shared_preload_libraries=pg_search,vchord,pg_cron,pg_stat_statements,pg_prewarm \
  -c cron.database_name=postgres \
  -c shared_buffers=256MB -c max_connections=30 \
  -c default_toast_compression=lz4 -c paradedb.planner_warnings=off
