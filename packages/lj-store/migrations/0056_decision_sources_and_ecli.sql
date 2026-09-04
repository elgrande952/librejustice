-- Migration 0056 — Identité inter-sources & déduplication ECLI.
--
-- DDL bon marché uniquement. Le backfill des provenances existantes (une ligne
-- `decision_sources` par `decisions`, ~3M lignes) se fait HORS-MIGRATION via
-- `lj-ingest backfill-decision-sources` (batché par keyset, repris), pour ne pas
-- tenir une transaction longue qui bloquerait les écritures sur `decisions`
-- (base low-IOPS).

-- ── ECLI sur decisions ────────────────────────────────────────────────────
-- Colonne nullable (vide pour l'existant, peuplée au prochain re-extract / par
-- les nouveaux parsers). Aucune dérivation : valeur recopiée verbatim de la
-- source. Index partiel unique calqué sur `idx_decisions_public_id` (0004).
ALTER TABLE decisions
ADD COLUMN IF NOT EXISTS ecli TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_decisions_ecli
ON decisions (ecli)
WHERE ecli IS NOT NULL;

-- ── Provenances découplées ─────────────────────────────────────────────────
-- `source_uid` is duplicated here: intra-source idempotence stays carried by
-- `decisions.source_uid`.
CREATE TABLE decision_sources (
    id               BIGSERIAL PRIMARY KEY,
    decision_id      BIGINT   NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    source           TEXT     NOT NULL,   -- 'judilibre' | 'opendata' | 'dila-jade' | …
    source_uid       TEXT     NOT NULL UNIQUE,
    content_checksum TEXT     NOT NULL,   -- xxh3-64 du payload brut de CETTE provenance
    source_rank      SMALLINT NOT NULL,   -- gagnant = max (table de rang : lj-store)
    payload_format   TEXT     NOT NULL CHECK (payload_format IN ('xml','json','dila-xml','html')),
    ingested_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_decision_sources_decision_id ON decision_sources (decision_id);
