# LibreJustice autonomous API smoke kit

This kit verifies a real, independent **API core** from the Apache-2.0 LibreJustice repository at `ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75`. It is a bounded integration test, not a production deployment or a clone of the hosted legal corpus.

## What runs

The test adds an example entrypoint to a disposable checkout, calls the official database migrator and mounts the official `lj-api` router. It builds PostgreSQL 18 with the upstream pinned ParadeDB 0.25.6 and VectorChord 1.1.1 image recipe. The database is ephemeral and exposed only at `127.0.0.1:55432`; the API only at `127.0.0.1:18300`.

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
- API/database integration: **pending the actual workflow result**, never inferred from a green health endpoint alone.
- Assertions check health, retrieval of the synthetic marker and public API rejection without credentials. Full-text decision rendering, production OAuth, high-load behavior, semantic ranking, OCR, browser UI and complete corpus are not validated.
- `lj-ingest` is intentionally not built: its build script downloads a Qwen tokenizer. `mise run build-app` is not run because it fetches OCR/HTR model assets.
- Before compilation, the guard checks the resolved API dependency graph, excludes `lj-ingest` and rejects known model-download terms in reachable build-script entrypoints. This is not a full supply-chain security audit.

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
