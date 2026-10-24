# LibreJustice autonomous API smoke kit

This kit verifies a real, independent **API core** from the Apache-2.0 LibreJustice repository at `ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75`. It is a bounded integration test, not a production deployment or a clone of the hosted legal corpus.

## What runs

The test adds an example entrypoint to a disposable checkout, calls the official database migrator and mounts the official `lj-api` router. **One upstream SQL migration is patched in that disposable checkout**, as documented below; the API library code is unchanged. It builds PostgreSQL 18 with the upstream pinned ParadeDB 0.25.6 and VectorChord 1.1.1 image recipe. The database is ephemeral and exposed only at `127.0.0.1:55432`; the API only at `127.0.0.1:18300`.

One **synthetic** document, explicitly marked as such, tests lexical retrieval. The official dummy embedder is used solely to avoid a model/API dependency in this test. It provides no validated semantic capability. Public search must refuse requests lacking an API key. The internal test API is reachable only on the runner loopback.

## Commands

On a host with Docker, Rust 1.98.1, Python 3, a C/C++ toolchain, CMake, pkg-config and libclang:

```sh
git clone https://github.com/librejustice/librejustice upstream
git -C upstream checkout --detach ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75
LJ_SOURCE_DIR="$PWD/upstream" bash sol-fable-autonomy/run-smoke.sh
```

The manual GitHub workflow requires the exact 40-character commit SHA of this reviewed kit. It uses a standard public runner with a 45-minute job limit and 35-minute execution limit. It never uploads artifacts, publishes images, imports user data, or uses account/provider credentials. Runner logs contain the test outcome. Containers and volumes are removed even on failure.

## Boundaries and current evidence

- Upstream standalone MCP client: 26 upstream tests passed locally on 2026-10-24.
- Shell syntax and Python compilation checked locally.
- API/database integration on 2026-10-24: **passed in an independent, disposable instance**. See [the successful bounded CI run](https://github.com/elgrande952/librejustice/actions/runs/40130965975).
- Assertions check health, retrieval of the synthetic marker and public API rejection without credentials. Full-text decision rendering, production OAuth, high-load behavior, semantic ranking, OCR, browser UI and complete corpus are not validated.
- `lj-ingest` is intentionally not built: its build script downloads a Qwen tokenizer. `mise run build-app` is not run because it fetches OCR/HTR model assets.
- Before compilation, the guard checks the resolved API dependency graph, excludes `lj-ingest` and rejects known model-download terms in reachable build-script entrypoints. This is not a full supply-chain security audit.

The successful run used upstream revision `ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75`, the SQL deviation documented below, and test-kit revision `6b1a7624fef10515c96913edd607fd37b71edba9`. Its 95 build-script entrypoints passed the targeted guard; the API example compiled in 2 minutes 41 seconds. All 338 migrations completed. The backfill executed twice while `legal_text` and `text_filiation` remained empty; then exactly one synthetic decision was inserted.

All three HTTP checks passed: health returned `ok` with the expected upstream revision, lexical retrieval found the unique synthetic marker, and public search without credentials returned HTTP 401. Runtime extension versions were `pg_search=0.25.6`, `vchord=1.1.1` and `vector=0.8.4`. The database snapshot was 244.6 MiB out of its 2 GiB limit; this tiny test is not production sizing. Cleanup completed successfully. No permanent instance or public endpoint remains.

The patch was prompted by [an earlier fresh-database failure](https://github.com/elgrande952/librejustice/actions/runs/40130233557): unmodified migration 0204 inserted a relationship for absent canonical text `BE/CODE-CIVIL`, causing SQLSTATE `23503` before HTTP startup.

## Explicit upstream migration deviation

`apply_migration_patch.py` checks migration `0204_text_filiation.sql` against upstream SHA-256 `4e633ba41b4c96794f7625b7948c0a0ab197f6ee9d4680fd6a470ac9e68264f0`, then replaces only that file in the disposable checkout. The reviewed replacement SHA-256 is `1a58353857de0b145f2cd4a96405dd84b163899710b01243678e099a5a3cc13d`. Cleanup restores the original bytes. Neither the upstream repository nor any existing production database is changed.

The replacement preserves the seven original filiation tuples byte for byte and keeps all foreign keys. The insert casts `fork_date` explicitly to `date` and selects a relationship only when both canonical `legal_text` identifiers already exist. It inserts **no legal text rows**. `backfill_text_filiation.sql` preserves the same tuples and uses `ON CONFLICT DO NOTHING`, so the operator can rerun it explicitly after ingestion without losing relations skipped on an empty corpus:

```sh
psql "$LIBREJUSTICE_DB_URL" -v ON_ERROR_STOP=1 -f sol-fable-autonomy/backfill_text_filiation.sql
```

The integration harness checks that `legal_text` stays empty and executes the backfill twice before inserting its synthetic decision. Static tests proved exact seed preservation, retained foreign keys, no legal-text insert, date conversion, conflict handling, guarded application, exact restoration and rejection of an unexpectedly changed original. A read-only review of migrations 0205–0338 found no further certain dependence on a preloaded corpus; this does not replace runtime validation.

## Production path

A persistent independent deployment still requires a dedicated database volume, backed-up and monitored storage, TLS/authentication, deliberate corpus selection and licensed source access. The upstream database configuration gives development sizing references (512 MiB shared buffers, 2 GiB effective cache, 3 GiB container shared-memory setting); these are not measured minimum hardware requirements. The tiny integration test cannot estimate full-corpus storage.

The minimal independent architecture has three explicit parts:

1. A stateless LibreJustice API service behind an authenticated TLS endpoint. The test binds only loopback; no new public API or ChatGPT connection is exposed by this kit.
2. A dedicated persistent PostgreSQL 18 database with the pinned search extensions and a separately backed-up volume. No suitable unassigned database/volume has been identified in the existing deployment inventory. Existing services and private session storage are not reassigned.
3. An intentionally selected legal corpus plus an optional, compatible embedding service. Lexical indexing/search needs no inference model, but upstream API startup still constructs an embedder. Its `dummy` option is a test aid, not production semantic search. A production lexical-only route must explicitly force lexical mode, reject semantic/automatic modes and disable AI reranking; this adapter has not been deployed. Full hybrid search needs a verified existing embedding endpoint producing the expected 1,024-dimensional vectors. A generation model being installed does not prove this compatibility.

The standard CI runner is an actual alternative to local Docker, but its database is destroyed at the end and is not permanent hosting. Successful synthetic tests therefore establish bounded execution only. No new hosting purchase, model download, API credit, production service, volume, or credential change is part of this kit. Storage and capacity for a real corpus remain to be measured before production allocation.

The existing direct legal API connections need not be replaced by an independent index. Official Légifrance production uses `https://api.piste.gouv.fr/dila/legifrance/lf-engine-app`, Judilibre `https://api.piste.gouv.fr/cassation/judilibre/v1.0`, with PISTE OAuth at `https://oauth.piste.gouv.fr/api/oauth/token`. The upstream ingestion code expects `LIBREJUSTICE_PISTE_CLIENT_ID` and `LIBREJUSTICE_PISTE_CLIENT_SECRET`; PISTE application subscriptions must authorize the requested APIs. No identifiers or tokens are included in this kit.

## Provenance

Source: https://github.com/librejustice/librejustice/tree/ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75 . Upstream license is Apache-2.0. The new harness, scripts and workflow are original Sol-Fable integration files, also distributed under Apache-2.0. No upstream application source is modified persistently by the harness.
