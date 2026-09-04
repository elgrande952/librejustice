//! Acteurs par décision : écriture au fil de l'eau par le patron de `cases.rs`
//! (garde jamais-dégrader, skip-diff, DELETE + COPY binaire versionné), rattrapage
//! par re-dérivation à l'identique (`parties-reconcile`), résolution des clés
//! pendantes vers le référentiel d'entités.
//!
//! L'appelant (lj-ingest) plie et calcule `resolve_key`/`nature`/spans en Rust
//! (`lj_extract::parties`) — le SQL ne replie jamais.

use std::collections::{HashMap, HashSet};

use super::types::DecisionPartyRow;
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Item d'écriture bulk : `(decision_id, acteurs)`. `None` = extraction sans
/// couche parties (écrit un set vide).
pub type DecisionPartyWriteItem = (i64, Option<Vec<DecisionPartyRow>>);

/// Empreinte d'un acteur pour le skip-diff : tout sauf `entity_uid` (état de
/// résolution, pas une donnée d'extraction — un set inchangé garde ses liens) et
/// `ord` (régénéré). `resolve_key` et `barreau` en font partie : la clé dépend du
/// contexte documentaire, pas seulement de la valeur. Spans en `Option` : un fonds
/// backfillé (NULL) diffère d'un set calculé vide.
type PartyFingerprint = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<Vec<i32>>,
    Option<Vec<i32>>,
);

fn fingerprint(r: &DecisionPartyRow) -> PartyFingerprint {
    (
        r.quality.clone(),
        r.side.clone(),
        r.value.clone(),
        r.resolve_key.clone(),
        r.nature.clone(),
        r.barreau.clone(),
        r.role.clone(),
        r.firm_ord,
        r.party_ord,
        r.sub_ord,
        Some(r.char_starts.clone()),
        Some(r.char_ends.clone()),
    )
}

const COPY_SQL: &str = "COPY decision_party (decision_id, ord, quality, side, value, \
     resolve_key, char_starts, char_ends, nature, barreau, role, firm_ord, party_ord, \
     sub_ord, extract_version) \
     FROM STDIN (FORMAT binary)";
const COPY_TYPES: &[Type] = &[
    Type::INT8,
    Type::INT4,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::INT4_ARRAY,
    Type::INT4_ARRAY,
    Type::TEXT,
    Type::TEXT,
    Type::TEXT,
    Type::INT4,
    Type::INT4,
    Type::INT4,
    Type::INT2,
];

impl DecisionRepository<'_> {
    /// (Ré)écrit les acteurs d'UNE décision — sucre sur
    /// [`Self::replace_decision_parties_bulk`].
    pub async fn replace_decision_parties(
        &self,
        decision_id: i64,
        parties: Option<&[DecisionPartyRow]>,
    ) -> Result<Vec<i64>> {
        self.replace_decision_parties_bulk(&[(decision_id, parties.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les acteurs d'un lot de décisions : garde jamais-dégrader (version >
    /// `EXTRACT_VERSION` = révision manuelle / gold, jamais remplacée), skip-diff des
    /// sets inchangés (un set inchangé garde ses `entity_uid` résolus), puis DELETE +
    /// COPY binaire et résolution scopée at-write
    /// ([`Self::resolve_pending_parties_scoped`]).
    /// **La transaction appartient à l'appelant.** Idempotent. Rend les décisions
    /// RÉELLEMENT réécrites, pour que l'appelant date leur page du jour.
    #[tracing::instrument(name = "db.replace_decision_parties_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_decision_parties_bulk(
        &self,
        items: &[DecisionPartyWriteItem],
    ) -> Result<Vec<i64>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let protected: HashSet<i64> = self
            .conn
            .query(
                "SELECT id FROM decisions WHERE id = ANY($1) AND extract_version > $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?
            .into_iter()
            .map(|r| r.get(0))
            .collect();

        let db_rows = self
            .conn
            .query(
                "SELECT decision_id, quality, side, value, resolve_key, nature, barreau, \
                        role, firm_ord, party_ord, sub_ord, char_starts, char_ends \
                 FROM decision_party \
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<PartyFingerprint>> = HashMap::new();
        for row in &db_rows {
            current.entry(row.get(0)).or_default().insert((
                row.get(1),
                row.get(2),
                row.get(3),
                row.get(4),
                row.get(5),
                row.get(6),
                row.get(7),
                row.get(8),
                row.get(9),
                row.get(10),
                row.get(11),
                row.get(12),
            ));
        }
        let empty = HashSet::new();
        let changed: Vec<&DecisionPartyWriteItem> = items
            .iter()
            .filter(|(id, parties)| {
                if protected.contains(id) {
                    return false;
                }
                let new_set: HashSet<PartyFingerprint> = parties
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(fingerprint)
                    .collect();
                current.get(id).unwrap_or(&empty) != &new_set
            })
            .collect();
        if changed.is_empty() {
            return Ok(Vec::new());
        }

        for (decision_id, parties) in &changed {
            for r in parties.as_deref().unwrap_or(&[]) {
                if r.char_starts.len() != r.char_ends.len() {
                    return Err(StoreError::Invalid(format!(
                        "decision_party invalide (décision {decision_id}) : tableaux de \
                         spans non parallèles ({} vs {})",
                        r.char_starts.len(),
                        r.char_ends.len()
                    )));
                }
                let mut prev_end = i32::MIN;
                for (s, e) in r.char_starts.iter().zip(&r.char_ends) {
                    if s >= e || *s < prev_end {
                        return Err(StoreError::Invalid(format!(
                            "decision_party invalide (décision {decision_id}) : span \
                             [{s}, {e}) après end={prev_end} — tri/chevauchement violé"
                        )));
                    }
                    prev_end = *e;
                }
            }
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM decision_party WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let sink = self.conn.copy_in(COPY_SQL).await?;
        let writer = BinaryCopyInWriter::new(sink, COPY_TYPES);
        tokio::pin!(writer);
        for (decision_id, parties) in &changed {
            for (ord, r) in parties.as_deref().unwrap_or(&[]).iter().enumerate() {
                writer
                    .as_mut()
                    .write(&[
                        decision_id,
                        &(ord as i32),
                        &r.quality,
                        &r.side,
                        &r.value,
                        &r.resolve_key,
                        &r.char_starts,
                        &r.char_ends,
                        &r.nature,
                        &r.barreau,
                        &r.role,
                        &r.firm_ord,
                        &r.party_ord,
                        &r.sub_ord,
                        &lj_core::EXTRACT_VERSION,
                    ])
                    .await?;
            }
        }
        writer.finish().await?;
        // Résolution at-write scopée au lot : les lignes réécrites se lient immédiatement.
        let has_rows = changed
            .iter()
            .any(|(_, p)| !p.as_deref().unwrap_or(&[]).is_empty());
        if has_rows {
            self.resolve_pending_parties_scoped(&changed_ids).await?;
        }
        Ok(changed_ids)
    }

    /// Résolution scopée au LOT (at-write) : mêmes périmètres et même sémantique
    /// d'unicité que [`Self::resolve_pending_parties`], bornés aux clés pendantes des
    /// décisions du lot, le registre sondé par INDEX au lieu d'être déplié — morales et
    /// cnb par le GIN `entity_folded_all_idx` (`@>`/`&&` par clé, LATERAL + `OFFSET 0`
    /// pour forcer le probe), patronymes composés par `entity_surname_key_idx`, oacc
    /// scopé tel quel (registre ~140). La passe « cabinets normalisés » n'a pas de
    /// forme indexable (la norme tokenisée n'est pas le fold) et reste aux passes
    /// pleines (`parties-relink` hebdo, `reconcile`). **Transaction de l'appelant.**
    /// Renvoie le nombre de lignes liées.
    #[tracing::instrument(name = "db.resolve_pending_parties_scoped", skip_all, fields(db.system = "postgresql", ids = decision_ids.len()))]
    pub async fn resolve_pending_parties_scoped(&self, decision_ids: &[i64]) -> Result<u64> {
        let mut linked = 0u64;
        for sql in SCOPED_PARTY_PASSES {
            linked += self.conn.execute(*sql, &[&decision_ids]).await?;
        }
        linked += self
            .conn
            .execute(
                cnb_fullname_sql(PartyScope::ByDecision).as_str(),
                &[&decision_ids],
            )
            .await?;
        linked += self
            .conn
            .execute(
                cnb_surname_sql(PartyScope::ByDecision).as_str(),
                &[&decision_ids],
            )
            .await?;
        Ok(linked)
    }

    /// Passe « cabinets normalisés » seule : le volet hebdo de `parties-relink` — la
    /// passe pleine SIRENE/RNA (dépliage des 29 M) est reléguée au one-shot post-reload
    /// de registre.
    pub async fn resolve_pending_cabinets(&self) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let r: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            Ok(self.conn.execute(CABINETS_SQL, &[]).await?)
        }
        .await;
        match r {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Sonde du delta de registre : les identités cnb nouvelles ou réactivées d'un
    /// reload attirent les clés pendantes qui les visaient. Les surfaces sondées
    /// inversent exactement ce que les passes matchent : dénominations pliées ±
    /// rotations, et patronyme composé (formes espace et tout-tirets). Rejoue ensuite
    /// les deux étages en portée par clé.
    pub async fn resolve_pending_counsel_for_added(&self, added_uids: &[String]) -> Result<u64> {
        if added_uids.is_empty() {
            return Ok(0);
        }
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT p.resolve_key \
                 FROM entity a, \
                      LATERAL unnest(lj_fold_all(a.denominations)) AS f(name), \
                      LATERAL (VALUES (f.name), \
                               (regexp_replace(f.name, '^(\\S+) (.*)$', '\\2 \\1')), \
                               (regexp_replace(f.name, '^(.*) (\\S+)$', '\\2 \\1'))) AS v(k) \
                 JOIN decision_party p ON p.resolve_key = v.k \
                 WHERE a.uid = ANY($1) \
                   AND p.entity_uid IS NULL AND p.quality = 'counsel_name' \
                 UNION \
                 SELECT DISTINCT p.resolve_key \
                 FROM entity a \
                 JOIN decision_party p \
                   ON p.resolve_key IN (a.surname_key, replace(a.surname_key, ' ', '-')) \
                 WHERE a.uid = ANY($1) AND a.surname_key IS NOT NULL \
                   AND p.entity_uid IS NULL AND p.quality = 'counsel_name'",
                &[&added_uids],
            )
            .await?;
        let keys: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        self.resolve_pending_counsel_by_keys(&keys).await
    }

    /// Rejoue les deux étages de résolution conseil sur un jeu de clés nommées.
    /// Périmètre de la sonde at-write (clés réveillées par une identité entrante) comme
    /// du filet (clés libérées par un dé-liage).
    pub async fn resolve_pending_counsel_by_keys(&self, keys: &[String]) -> Result<u64> {
        if keys.is_empty() {
            return Ok(0);
        }
        let mut linked = self
            .conn
            .execute(cnb_fullname_sql(PartyScope::ByKey).as_str(), &[&keys])
            .await?;
        linked += self
            .conn
            .execute(cnb_surname_sql(PartyScope::ByKey).as_str(), &[&keys])
            .await?;
        Ok(linked)
    }

    /// Dé-liage du delta de registre : une identité nouvelle peut effondrer la preuve
    /// d'unicité de liens existants — patronyme devenu ambigu à la date de la décision,
    /// ou nom complet homonyme désormais actif à cette date. Ces liens retombent
    /// pendants (`entity_uid = NULL`), prêts à se re-résoudre. Piloté par les identités
    /// ajoutées : ne touche que les liens de leurs voisines de patronyme ou de
    /// dénomination.
    pub async fn cnb_delta_unlink(&self, added_uids: &[String]) -> Result<u64> {
        if added_uids.is_empty() {
            return Ok(0);
        }
        // Liens posés par le sous-étage patronyme (resolve_key = patronyme).
        let s = self
            .conn
            .execute(
                "WITH touched AS ( \
                     SELECT DISTINCT a.surname_key FROM entity a \
                     WHERE a.uid = ANY($1) AND a.surname_key IS NOT NULL \
                 ), siblings AS ( \
                     SELECT e.uid, e.surname_key FROM entity e \
                     JOIN touched t ON t.surname_key = e.surname_key \
                     WHERE e.uid LIKE 'cnb:%' \
                 ) \
                 UPDATE decision_party p SET entity_uid = NULL \
                 FROM siblings s, decisions d \
                 WHERE p.entity_uid = s.uid \
                   AND d.id = p.decision_id \
                   AND translate(p.resolve_key, '-', ' ') = s.surname_key \
                   AND (SELECT count(DISTINCT e2.uid) FROM entity e2 \
                        WHERE e2.uid LIKE 'cnb:%' \
                          AND e2.surname_key = s.surname_key \
                          AND lj_active_at(e2.activity, d.date_lecture)) > 1",
                &[&added_uids],
            )
            .await?;
        // Liens posés par nom complet : un homonyme entrant actif à la date
        // casse l'unicité.
        let n = self
            .conn
            .execute(
                "WITH names AS ( \
                     SELECT DISTINCT f.name FROM entity a, \
                          LATERAL unnest(lj_fold_all(a.denominations)) AS f(name) \
                     WHERE a.uid = ANY($1) \
                 ), siblings AS ( \
                     SELECT DISTINCT h.uid, n.name FROM names n \
                     CROSS JOIN LATERAL ( \
                         SELECT e.uid FROM entity e \
                         WHERE lj_fold_all(e.denominations) @> ARRAY[n.name] \
                           AND e.uid LIKE 'cnb:%' \
                           AND NOT (e.uid = ANY($1)) \
                         OFFSET 0) h \
                 ) \
                 UPDATE decision_party p SET entity_uid = NULL \
                 FROM siblings s, decisions d \
                 WHERE p.entity_uid = s.uid \
                   AND d.id = p.decision_id \
                   AND p.resolve_key IN ( \
                       s.name, \
                       regexp_replace(s.name, '^(\\S+) (.*)$', '\\2 \\1'), \
                       regexp_replace(s.name, '^(.*) (\\S+)$', '\\2 \\1')) \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM ( \
                           SELECT e2.uid, e2.barreau FROM entity e2 \
                           WHERE lj_fold_all(e2.denominations) && ARRAY[ \
                               p.resolve_key, \
                               regexp_replace(p.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                               regexp_replace(p.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')] \
                             AND e2.uid LIKE 'cnb:%' \
                             AND lj_active_at(e2.activity, d.date_lecture)) c \
                       HAVING (count(DISTINCT c.uid) = 1 \
                               AND min(c.uid) = p.entity_uid) \
                           OR (count(DISTINCT c.uid) FILTER ( \
                                   WHERE c.barreau IS NOT DISTINCT FROM p.barreau) = 1 \
                               AND min(c.uid) FILTER ( \
                                   WHERE c.barreau IS NOT DISTINCT FROM p.barreau) \
                                   = p.entity_uid))",
                &[&added_uids],
            )
            .await?;
        Ok(s + n)
    }

    /// Filet plein du même invariant : **tout** lien `cnb:` doit être celui que la
    /// résolution reproduirait aujourd'hui. [`Self::cnb_delta_unlink`] ne voit que le
    /// voisinage des identités ajoutées ; une ambiguïté de stock (deux porteurs déjà
    /// présents, liés sous l'ancienne règle non datée) lui échappe. Rejouable, ~no-op
    /// en régime : les deux étages se restreignent d'abord aux clés réellement
    /// ambiguës. L'étage nom complet dépasse les 30 s du pool : l'appelant lève le
    /// `statement_timeout`.
    pub async fn cnb_unlink_ambiguous(&self) -> Result<Vec<String>> {
        // Patronyme : un patronyme porté par ≥ 2 avocats actifs à la date de
        // lecture ne prouve plus rien.
        let s = self
            .conn
            .query(
                "WITH ambigus AS ( \
                     SELECT surname_key AS key FROM entity \
                     WHERE uid LIKE 'cnb:%' AND surname_key IS NOT NULL \
                     GROUP BY 1 HAVING count(*) > 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = NULL \
                 FROM entity e, decisions d, ambigus a \
                 WHERE p.entity_uid = e.uid \
                   AND p.quality = 'counsel_name' \
                   AND d.id = p.decision_id \
                   AND e.surname_key = a.key \
                   AND translate(p.resolve_key, '-', ' ') = a.key \
                   AND (SELECT count(DISTINCT e2.uid) FROM entity e2 \
                        WHERE e2.uid LIKE 'cnb:%' \
                          AND e2.surname_key = a.key \
                          AND lj_active_at(e2.activity, d.date_lecture)) > 1 \
                 RETURNING p.resolve_key",
                &[],
            )
            .await?;
        // Nom complet : mêmes rotations et même départage par barreau que le résolveur —
        // un lien survit exactement s'il est celui qu'il poserait. CTE matérialisées :
        // inlinées, le planificateur re-sonde le GIN par triplet.
        let n = self
            .conn
            .query(
                "WITH ambig AS MATERIALIZED ( \
                     SELECT f.name FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) f(name) \
                     WHERE e.uid LIKE 'cnb:%' \
                     GROUP BY 1 HAVING count(DISTINCT e.uid) > 1 \
                 ), porteurs AS MATERIALIZED ( \
                     SELECT DISTINCT h.uid, a.name FROM ambig a \
                     CROSS JOIN LATERAL ( \
                         SELECT e.uid FROM entity e \
                         WHERE lj_fold_all(e.denominations) @> ARRAY[a.name] \
                           AND e.uid LIKE 'cnb:%' \
                         OFFSET 0) h \
                 ), triples AS MATERIALIZED ( \
                     SELECT DISTINCT p.resolve_key, p.barreau, p.entity_uid, \
                            d.date_lecture \
                     FROM porteurs s \
                     JOIN decision_party p ON p.entity_uid = s.uid \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.quality = 'counsel_name' \
                       AND p.resolve_key IN (s.name, \
                           regexp_replace(s.name, '^(\\S+) (.*)$', '\\2 \\1'), \
                           regexp_replace(s.name, '^(.*) (\\S+)$', '\\2 \\1')) \
                 ), cand AS MATERIALIZED ( \
                     SELECT k.resolve_key, h.uid, h.ebarreau, h.activity \
                     FROM (SELECT DISTINCT resolve_key FROM triples) k \
                     CROSS JOIN LATERAL ( \
                         SELECT e.uid, e.barreau AS ebarreau, e.activity \
                         FROM entity e \
                         WHERE lj_fold_all(e.denominations) && ARRAY[ \
                                 k.resolve_key, \
                                 regexp_replace(k.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                                 regexp_replace(k.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')] \
                           AND e.uid LIKE 'cnb:%' \
                         OFFSET 0) h \
                 ), bad AS MATERIALIZED ( \
                     SELECT t.resolve_key, t.barreau, t.entity_uid, t.date_lecture \
                     FROM triples t \
                     JOIN cand c ON c.resolve_key = t.resolve_key \
                     WHERE lj_active_at(c.activity, t.date_lecture) \
                     GROUP BY 1, 2, 3, 4 \
                     HAVING NOT ((count(DISTINCT c.uid) = 1 \
                                  AND min(c.uid) = t.entity_uid) \
                              OR (count(DISTINCT c.uid) FILTER ( \
                                      WHERE c.ebarreau IS NOT DISTINCT FROM t.barreau) = 1 \
                                  AND min(c.uid) FILTER ( \
                                      WHERE c.ebarreau IS NOT DISTINCT FROM t.barreau) \
                                      = t.entity_uid)) \
                 ) \
                 UPDATE decision_party p SET entity_uid = NULL \
                 FROM bad b, decisions d \
                 WHERE p.quality = 'counsel_name' \
                   AND p.resolve_key = b.resolve_key \
                   AND p.barreau IS NOT DISTINCT FROM b.barreau \
                   AND p.entity_uid = b.entity_uid \
                   AND d.id = p.decision_id \
                   AND d.date_lecture IS NOT DISTINCT FROM b.date_lecture \
                 RETURNING p.resolve_key",
                &[],
            )
            .await?;
        let mut keys: Vec<String> = s.iter().chain(n.iter()).map(|r| r.get(0)).collect();
        keys.sort_unstable();
        keys.dedup();
        Ok(keys)
    }

    /// Résout les clés pendantes vers `entity` par dénomination pliée NON AMBIGUË :
    /// une clé qui matche plusieurs entités distinctes reste pendante.
    ///
    /// Deux périmètres : morales (`party`, `law_firm`) contre `siren:`/`rna:` —
    /// complétées, pour les `law_firm` hors CC/CE, par l'égalité de forme normalisée
    /// restreinte à la catégorie `cabinets` —, et avocats (`counsel_name`) contre
    /// `cnb:` (clé nom complet ± rotations de tokens, homonymes départagés par la
    /// colonne `entity.barreau`, unicité évaluée À DATE parmi les actifs
    /// `lj_active_at`, décisions CC/CE exclues : avocats aux Conseils hors CNB), avec
    /// sous-étage patronyme composé unique national (`entity.surname_key`).
    ///
    /// Les seules décisions CC/CE se résolvent contre `oacc:` (`counsel_name` →
    /// avocats, `law_firm` → sociétés `oacc:firm-%` par sous-ensemble de tokens),
    /// unicité du hit exigée, sans discriminant barreau (registre minuscule).
    /// `intervenor` est gaté. Renvoie le nombre de lignes liées.
    #[tracing::instrument(name = "db.resolve_pending_parties", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_parties(&self) -> Result<u64> {
        // Jointure clés distinctes × 32 M dénominations : dépasse le `statement_timeout`
        // du pool (30 s) — levé localement, transaction dédiée. Idempotent.
        self.conn.batch_execute("BEGIN").await?;
        let updated: Result<u64> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            // Sélection matérialisée avant la pose. Sous un `UPDATE`, Postgres ne parallélise
            // rien : les 29 M entités se déplient sur un seul cœur. La même sélection en
            // `SELECT` planifie un `Gather Merge` avec `Parallel Seq Scan on entity`.
            self.conn
                .batch_execute(
                    "CREATE TEMP TABLE lj_hit_morales ON COMMIT DROP AS \
                     WITH keys AS ( \
                     SELECT DISTINCT resolve_key FROM decision_party \
                     WHERE entity_uid IS NULL \
                       AND quality IN ('party', 'law_firm') \
                 ), names AS ( \
                     SELECT e.uid, f.folded \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                     WHERE e.uid NOT LIKE 'cnb:%' \
                       AND e.uid NOT LIKE 'oacc:%' \
                 ) \
                 SELECT n.folded, min(n.uid) AS uid \
                 FROM names n \
                 JOIN keys k ON k.resolve_key = n.folded \
                 GROUP BY n.folded \
                 HAVING count(DISTINCT n.uid) = 1",
                )
                .await?;
            self.conn.batch_execute("ANALYZE lj_hit_morales").await?;
            let n = self
                .conn
                .execute(
                    "UPDATE decision_party p SET entity_uid = h.uid \
                 FROM lj_hit_morales h \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality IN ('party', 'law_firm') \
                   AND p.resolve_key = h.folded",
                    &[],
                )
                .await?;
            // Cabinets normalisés : les `law_firm` encore pendantes hors CC/CE se comparent en
            // forme normalisée (formes sociales, génériques du métier, conjonctions et tokens
            // < 2 chars jetés, particules de patronymes conservées, ordre conservé) contre les
            // dénominations — courantes et historiques — des entités de catégorie `cabinets`
            // uniquement. Clés à `[` exclues (placeholders d'anonymisation), unicité du hit
            // exigée. `lj_fold_all` peut émettre des folded dupliqués (périodes SIRENE,
            // variantes qui plient pareil) : dédup AVANT tokenisation, sinon `string_agg`
            // double les tokens (« actis actis ») et la norme est corrompue.
            let g = self.conn.execute(CABINETS_SQL, &[]).await?;
            // Registre `cnb:` déplié une fois pour les deux étages avocats. Au grain de la
            // passe pleine — clé × barreau × date × rotations — le hachage bat la sonde par
            // clé.
            self.conn
                .batch_execute(&format!(
                    "CREATE TEMP TABLE lj_reg_cnb ON COMMIT DROP AS {CNB_REG_SQL}"
                ))
                .await?;
            self.conn
                .batch_execute("CREATE INDEX ON lj_reg_cnb (name); ANALYZE lj_reg_cnb")
                .await?;
            // Chaque étage : sélection matérialisée puis analysée avant la pose. Les portées
            // scopées gardent la CTE et la sonde, leur `pend` tenant en quelques centaines de
            // lignes.
            self.conn
                .batch_execute(&format!(
                    "CREATE TEMP TABLE lj_pick_fullname ON COMMIT DROP AS {}",
                    cnb_fullname_pick_sql(PartyScope::All, Some("lj_reg_cnb"))
                ))
                .await?;
            self.conn.batch_execute("ANALYZE lj_pick_fullname").await?;
            let c = self
                .conn
                .execute(cnb_fullname_update_sql("lj_pick_fullname").as_str(), &[])
                .await?;
            // Sous-étage patronyme composé : une clé counsel encore pendante, multi-token une
            // fois les tirets normalisés en espaces, qui ne matche AUCUNE dénomination
            // complète cnb (as-is ni rotations — garde anti-collision prénom+nom ↔ patronyme
            // composé homonyme) et dont le patronyme est UNIQUE au registre
            // (`entity.surname_key`) → lie. Hors CC/CE comme l'étage nom complet.
            self.conn
                .batch_execute(&format!(
                    "CREATE TEMP TABLE lj_pick_surname ON COMMIT DROP AS {}",
                    cnb_surname_pick_sql(PartyScope::All, Some("lj_reg_cnb"))
                ))
                .await?;
            self.conn.batch_execute("ANALYZE lj_pick_surname").await?;
            let s = self
                .conn
                .execute(cnb_surname_update_sql("lj_pick_surname").as_str(), &[])
                .await?;
            // Avocats aux Conseils : le registre `oacc:` ne résout QUE les décisions
            // `jurisdiction_type IN ('CC', 'CE')`. Registre minuscule (~140), pas de
            // discriminant barreau : on exige l'unicité du hit dans `oacc:`. Deux volets :
            // `counsel_name` → avocats (nom complet ± rotations de tokens, ou nom-seul ==
            // patronyme d'un avocat) ; `law_firm` → sociétés (`oacc:firm-%`) par sous-ensemble
            // de tokens.
            let o = self
                .conn
                .execute(
                    "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key AS k \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'counsel_name' \
                       AND d.jurisdiction_type IN ('CC', 'CE') \
                 ), variant AS ( \
                     SELECT k, k AS v FROM pend \
                     UNION \
                     SELECT k, regexp_replace(k, '^(\\S+) (.*)$', '\\2 \\1') \
                       FROM pend WHERE strpos(k, ' ') > 0 \
                     UNION \
                     SELECT k, regexp_replace(k, '^(.*) (\\S+)$', '\\2 \\1') \
                       FROM pend WHERE strpos(k, ' ') > 0 \
                 ), av AS ( \
                     SELECT e.uid AS entity_uid, f.folded, \
                            split_part(f.folded, ' ', -1) AS surname \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                     WHERE e.uid LIKE 'oacc:%' AND e.uid NOT LIKE 'oacc:firm-%' \
                 ), cand AS ( \
                     SELECT v.k, a.entity_uid FROM variant v JOIN av a ON a.folded = v.v \
                     UNION \
                     SELECT p.k, a.entity_uid FROM pend p JOIN av a ON a.surname = p.k \
                       WHERE strpos(p.k, ' ') = 0 \
                 ), pick AS ( \
                     SELECT k, min(entity_uid) AS uid \
                     FROM cand GROUP BY k HAVING count(DISTINCT entity_uid) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = pk.uid \
                 FROM pick pk, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'counsel_name' \
                   AND p.resolve_key = pk.k \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type IN ('CC', 'CE')",
                    &[],
                )
                .await?;
            // Sous-ensemble de tokens : la clé pliée est tokenisée sur les non-alphanumériques,
            // débarrassée des mots de forme sociale, des génériques et des tokens < 2 chars
            // (initiales, placeholders anonymisés) ; elle lie si tous ses tokens restants
            // figurent dans la dénomination d'exactement un cabinet.
            let f = self
                .conn
                .execute(
                    "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key AS k \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'law_firm' \
                       AND d.jurisdiction_type IN ('CC', 'CE') \
                 ), ptok AS ( \
                     SELECT k, t.tok \
                     FROM pend, LATERAL regexp_split_to_table(k, '[^[:alnum:]]+') AS t(tok) \
                     WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
                         'scp','sarl','sarlu','sas','selas','selarl','selafa', \
                         'selasu','aarpi','societe','cabinet','avocat','avocats', \
                         'associe','associes','et','ou','de','du','des','la','le', \
                         'les','au','aux') \
                     GROUP BY k, t.tok \
                 ), nkey AS ( \
                     SELECT k, count(*) AS nt FROM ptok GROUP BY k \
                 ), ftok AS MATERIALIZED ( \
                     SELECT e.uid AS entity_uid, t.tok \
                     FROM entity e, \
                          LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded), \
                          LATERAL regexp_split_to_table(f.folded, '[^[:alnum:]]+') AS t(tok) \
                     WHERE e.uid LIKE 'oacc:firm-%' AND t.tok <> '' \
                     GROUP BY e.uid, t.tok \
                 ), hit AS ( \
                     SELECT pt.k, f.entity_uid \
                     FROM ptok pt \
                     JOIN ftok f ON f.tok = pt.tok \
                     JOIN nkey nk ON nk.k = pt.k \
                     GROUP BY pt.k, f.entity_uid, nk.nt \
                     HAVING count(*) = nk.nt \
                 ), pick AS ( \
                     SELECT k, min(entity_uid) AS uid \
                     FROM hit GROUP BY k HAVING count(DISTINCT entity_uid) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = pk.uid \
                 FROM pick pk, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'law_firm' \
                   AND p.resolve_key = pk.k \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type IN ('CC', 'CE')",
                    &[],
                )
                .await?;
            Ok(n + g + c + s + o + f)
        }
        .await;
        match updated {
            Ok(n) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Volumétrie de liaison : (total, résolues, dont `siren:`, dont `rna:`,
    /// dont `cnb:`, dont `oacc:`).
    pub async fn decision_party_stats(&self) -> Result<(i64, i64, i64, i64, i64, i64)> {
        let row = self
            .conn
            .query_one(
                "SELECT count(*), count(entity_uid), \
                        count(*) FILTER (WHERE entity_uid LIKE 'siren:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'rna:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'cnb:%'), \
                        count(*) FILTER (WHERE entity_uid LIKE 'oacc:%') \
                 FROM decision_party",
                &[],
            )
            .await?;
        Ok((
            row.get(0),
            row.get(1),
            row.get(2),
            row.get(3),
            row.get(4),
            row.get(5),
        ))
    }
}

/// Portée d'une passe cnb : passe pleine (one-shot), lot d'écriture
/// (`decision_ids`), ou sonde du delta de registre (`resolve_key`s pendantes
/// touchées par les identités nouvelles).
#[derive(Clone, Copy)]
pub(super) enum PartyScope {
    All,
    ByDecision,
    ByKey,
}

impl PartyScope {
    fn fragment(self) -> &'static str {
        match self {
            PartyScope::All => "",
            PartyScope::ByDecision => "AND p.decision_id = ANY($1) ",
            PartyScope::ByKey => "AND p.resolve_key = ANY($1) ",
        }
    }
}

/// Registre `cnb:` déplié — (`name`, `uid`, `ebarreau`, `activity`), une ligne par
/// dénomination repliée.
///
/// `lj_fold_all(e.denominations) @> ARRAY[k]` dit « `k` est membre du tableau » :
/// c'est exactement une égalité sur le tableau déplié. Déplier une fois et hacher
/// remplace donc les sondes par clé à sémantique identique, et gagne dès que les
/// clés sont nombreuses (cf. [`cnb_fullname_pick_sql`]).
pub(super) const CNB_REG_SQL: &str = "SELECT f.name, e.uid, e.barreau AS ebarreau, e.activity \
     FROM entity e, LATERAL unnest(lj_fold_all(e.denominations)) f(name) \
     WHERE e.uid LIKE 'cnb:%'";

/// Étage nom complet ± rotations → `cnb:`, **à date** : l'unicité s'évalue parmi
/// les candidats `lj_active_at(activity, date_lecture)` — un groupe par (clé,
/// barreau d'apposition, date de décision). Le discriminant barreau lit la colonne
/// `entity.barreau` (l'uid CNBF ne porte plus le barreau). Une seule source de
/// vérité pour les trois portées.
pub(super) fn cnb_fullname_sql(scope: PartyScope) -> String {
    format!(
        "WITH pick AS ({pick}) {upd}",
        pick = cnb_fullname_pick_sql(scope, None),
        upd = cnb_fullname_update_sql("pick")
    )
}

/// Sélection seule de l'étage nom complet : rend (`resolve_key`, `barreau`,
/// `date_lecture`, `uid`).
///
/// `reg` choisit la stratégie de rapprochement, et ce choix dépend du nombre de
/// clés. `None` sonde `entity_folded_all_idx` par variante : optimal pour les
/// portées scopées, qui en ont quelques centaines. `Some(rel)` joint le registre
/// déjà déplié : la passe pleine est très au-delà du point de bascule où le hachage
/// l'emporte sur la sonde.
pub(super) fn cnb_fullname_pick_sql(scope: PartyScope, reg: Option<&str>) -> String {
    let cand = match reg {
        Some(rel) => format!(
            "SELECT v.resolve_key, v.barreau, v.date_lecture, r.uid AS entity_uid, r.ebarreau \
             FROM variant v \
             JOIN {rel} r ON r.name = v.k \
             WHERE lj_active_at(r.activity, v.date_lecture)"
        ),
        None => "SELECT v.resolve_key, v.barreau, v.date_lecture, h.uid AS entity_uid, h.ebarreau \
             FROM variant v \
             CROSS JOIN LATERAL ( \
                 SELECT e.uid, e.barreau AS ebarreau FROM entity e \
                 WHERE lj_fold_all(e.denominations) @> ARRAY[v.k] \
                   AND e.uid LIKE 'cnb:%' \
                   AND lj_active_at(e.activity, v.date_lecture) \
                 OFFSET 0) h"
            .to_string(),
    };
    format!(
        "WITH pend AS ( \
             SELECT DISTINCT p.resolve_key, p.barreau, d.date_lecture \
             FROM decision_party p \
             JOIN decisions d ON d.id = p.decision_id \
             WHERE p.entity_uid IS NULL \
               AND p.quality = 'counsel_name' \
               AND strpos(p.resolve_key, ' ') > 0 \
               AND d.jurisdiction_type NOT IN ('CC', 'CE') \
               {scope}\
         ), variant AS ( \
             SELECT resolve_key, barreau, date_lecture, resolve_key AS k FROM pend \
             UNION \
             SELECT resolve_key, barreau, date_lecture, \
                    regexp_replace(resolve_key, '^(\\S+) (.*)$', '\\2 \\1') FROM pend \
             UNION \
             SELECT resolve_key, barreau, date_lecture, \
                    regexp_replace(resolve_key, '^(.*) (\\S+)$', '\\2 \\1') FROM pend \
         ), cand AS ({cand} \
         ) \
         SELECT resolve_key, barreau, date_lecture, \
                CASE \
                  WHEN count(DISTINCT entity_uid) = 1 THEN min(entity_uid) \
                  WHEN count(DISTINCT entity_uid) FILTER (WHERE ebarreau = barreau) = 1 \
                    THEN min(entity_uid) FILTER (WHERE ebarreau = barreau) \
                END AS uid \
         FROM cand GROUP BY resolve_key, barreau, date_lecture",
        scope = scope.fragment(),
        cand = cand
    )
}

/// Pose les liens de l'étage nom complet depuis une relation (`resolve_key`,
/// `barreau`, `date_lecture`, `uid`). Même motif que [`cnb_surname_update_sql`] :
/// en CTE la sélection n'est pas chiffrable, matérialisée et analysée elle l'est.
pub(super) fn cnb_fullname_update_sql(rel: &str) -> String {
    format!(
        "UPDATE decision_party p SET entity_uid = k.uid \
         FROM {rel} k, decisions d \
         WHERE p.entity_uid IS NULL \
           AND p.quality = 'counsel_name' \
           AND p.resolve_key = k.resolve_key \
           AND p.barreau IS NOT DISTINCT FROM k.barreau \
           AND k.uid IS NOT NULL \
           AND d.id = p.decision_id \
           AND d.date_lecture IS NOT DISTINCT FROM k.date_lecture \
           AND d.jurisdiction_type NOT IN ('CC', 'CE')"
    )
}

/// Sous-étage patronyme composé unique, **à date** : la garde anti-collision
/// nom-complet reste NON datée (conservatrice — un nom complet homonyme, même
/// inactif à la date, disqualifie la clé) ; l'unicité du patronyme s'évalue parmi
/// les actifs à la date.
///
/// La garde passe par le `OFFSET 0` de la maison : sans lui le planificateur
/// aplatit le `NOT EXISTS` en anti-jointure, matérialise tout le registre `cnb:` et
/// recalcule `lj_fold_all` par couple (clé, avocat) au lieu de sonder
/// `entity_folded_all_idx`.
pub(super) fn cnb_surname_sql(scope: PartyScope) -> String {
    format!(
        "WITH pick AS ({pick}) {upd}",
        pick = cnb_surname_pick_sql(scope, None),
        upd = cnb_surname_update_sql("pick")
    )
}

/// Sélection seule du sous-étage patronyme : rend les triplets (`resolve_key`,
/// `date_lecture`, `uid`) à poser. Isolée du `UPDATE` pour que la passe pleine
/// puisse la matérialiser et l'`ANALYZE` — cf. [`cnb_surname_update_sql`]. `reg`
/// arbitre sonde ou hachage, comme pour [`cnb_fullname_pick_sql`].
pub(super) fn cnb_surname_pick_sql(scope: PartyScope, reg: Option<&str>) -> String {
    let eligible = match reg {
        Some(rel) => format!(
            "knames AS ( \
                 SELECT DISTINCT k.resolve_key, n.name \
                 FROM (SELECT DISTINCT resolve_key FROM pend) k, \
                      LATERAL unnest(ARRAY[ \
                          k.resolve_key, \
                          regexp_replace(k.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                          regexp_replace(k.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')]) n(name) \
             ), porte AS ( \
                 SELECT DISTINCT kn.resolve_key FROM knames kn JOIN {rel} r ON r.name = kn.name \
             ), eligible AS ( \
                 SELECT k.resolve_key FROM (SELECT DISTINCT resolve_key FROM pend) k \
                 LEFT JOIN porte m ON m.resolve_key = k.resolve_key \
                 WHERE m.resolve_key IS NULL \
             )"
        ),
        None => "eligible AS ( \
             SELECT DISTINCT k.resolve_key FROM pend k \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM ( \
                     SELECT 1 FROM entity e \
                     WHERE lj_fold_all(e.denominations) && ARRAY[ \
                             k.resolve_key, \
                             regexp_replace(k.resolve_key, '^(\\S+) (.*)$', '\\2 \\1'), \
                             regexp_replace(k.resolve_key, '^(.*) (\\S+)$', '\\2 \\1')] \
                       AND e.uid LIKE 'cnb:%' \
                     OFFSET 0) h) \
         )"
        .to_string(),
    };
    format!(
        "WITH pend AS ( \
             SELECT DISTINCT p.resolve_key, d.date_lecture \
             FROM decision_party p \
             JOIN decisions d ON d.id = p.decision_id \
             WHERE p.entity_uid IS NULL \
               AND p.quality = 'counsel_name' \
               AND (strpos(p.resolve_key, ' ') > 0 \
                    OR strpos(p.resolve_key, '-') > 0) \
               AND d.jurisdiction_type NOT IN ('CC', 'CE') \
               {scope}\
         ), {eligible} \
         SELECT p2.resolve_key, p2.date_lecture, min(e.uid) AS uid \
         FROM pend p2 \
         JOIN eligible g ON g.resolve_key = p2.resolve_key \
         JOIN entity e ON e.surname_key = translate(p2.resolve_key, '-', ' ') \
         WHERE e.uid LIKE 'cnb:%' \
           AND lj_active_at(e.activity, p2.date_lecture) \
         GROUP BY p2.resolve_key, p2.date_lecture \
         HAVING count(DISTINCT e.uid) = 1",
        scope = scope.fragment(),
        eligible = eligible
    )
}

/// Pose les liens depuis une relation de triplets (`resolve_key`, `date_lecture`,
/// `uid`) — CTE `pick` pour les portées scopées, table temporaire analysée pour la
/// passe pleine.
///
/// En CTE, le planificateur estime la sélection à 1 ligne (elle sort d'un `HAVING
/// count(DISTINCT …) = 1`) et balaie toutes les lignes pendantes par filtre de
/// jointure. Matérialisée puis `ANALYZE`, elle est chiffrée juste et la pose
/// retombe à une sonde d'index par clé.
pub(super) fn cnb_surname_update_sql(rel: &str) -> String {
    format!(
        "UPDATE decision_party p SET entity_uid = pk.uid \
         FROM {rel} pk, decisions d \
         WHERE p.entity_uid IS NULL \
           AND p.quality = 'counsel_name' \
           AND p.resolve_key = pk.resolve_key \
           AND d.id = p.decision_id \
           AND d.date_lecture IS NOT DISTINCT FROM pk.date_lecture \
           AND d.jurisdiction_type NOT IN ('CC', 'CE')"
    )
}

/// Passe « cabinets normalisés » : forme tokenisée non indexable — pas de jumelle
/// at-write. Hebdo (`parties-relink`) et passe pleine one-shot partagent ce SQL.
const CABINETS_SQL: &str = "WITH pend AS ( \
                     SELECT DISTINCT p.resolve_key AS k \
                     FROM decision_party p \
                     JOIN decisions d ON d.id = p.decision_id \
                     WHERE p.entity_uid IS NULL \
                       AND p.quality = 'law_firm' \
                       AND d.jurisdiction_type NOT IN ('CC', 'CE') \
                       AND strpos(p.resolve_key, '[') = 0 \
                 ), knorm AS ( \
                     SELECT k, string_agg(tok, ' ' ORDER BY ord) AS norm \
                     FROM pend, LATERAL regexp_split_to_table(k, '[^[:alnum:]]+') \
                          WITH ORDINALITY AS t(tok, ord) \
                     WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
                         'scp','sarl','sarlu','sas','sasu','selarl','selarlu', \
                         'selas','selasu','selafa','sel','aarpi','sep', \
                         'societe','societes','cabinet','cabinets','etude', \
                         'office','avocat','avocats','associe','associes', \
                         'conseil','conseils','et','ou') \
                     GROUP BY k \
                 ), dnorm AS MATERIALIZED ( \
                     SELECT dd.entity_uid, string_agg(tok, ' ' ORDER BY ord) AS norm \
                     FROM (SELECT DISTINCT e.uid AS entity_uid, f.folded \
                           FROM entity e, \
                                LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
                           WHERE e.category = 'cabinets' AND e.uid LIKE 'siren:%') dd, \
                          LATERAL regexp_split_to_table(dd.folded, '[^[:alnum:]]+') \
                          WITH ORDINALITY AS t(tok, ord) \
                     WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
                         'scp','sarl','sarlu','sas','sasu','selarl','selarlu', \
                         'selas','selasu','selafa','sel','aarpi','sep', \
                         'societe','societes','cabinet','cabinets','etude', \
                         'office','avocat','avocats','associe','associes', \
                         'conseil','conseils','et','ou') \
                     GROUP BY dd.entity_uid, dd.folded \
                 ), pick AS ( \
                     SELECT h.k, min(h.entity_uid) AS uid \
                     FROM (SELECT kn.k, dn.entity_uid \
                           FROM knorm kn JOIN dnorm dn ON dn.norm = kn.norm \
                           GROUP BY kn.k, dn.entity_uid) h \
                     GROUP BY h.k HAVING count(*) = 1 \
                 ) \
                 UPDATE decision_party p SET entity_uid = pk.uid \
                 FROM pick pk, decisions d \
                 WHERE p.entity_uid IS NULL \
                   AND p.quality = 'law_firm' \
                   AND p.resolve_key = pk.k \
                   AND d.id = p.decision_id \
                   AND d.jurisdiction_type NOT IN ('CC', 'CE')";

/// Les trois passes NON-cnb de la résolution SCOPÉE
/// ([`DecisionRepository::resolve_pending_parties_scoped`], `$1` = `decision_ids`
/// du lot) — jumelles indexées des passes pleines de
/// [`DecisionRepository::resolve_pending_parties`] : mêmes périmètres, même
/// unicité, mêmes queues d'UPDATE corpus-wide par clé. Les deux passes cnb vivent
/// dans les builders [`cnb_fullname_sql`] / [`cnb_surname_sql`], une source pour
/// les trois portées. La passe « cabinets normalisés » n'a pas de jumelle (registre
/// tokenisé non indexable).
const SCOPED_PARTY_PASSES: &[&str] = &[
    // Morales (`party`, `law_firm`) → `siren:`/`rna:` non ambigu : probe GIN par clé
    // au lieu du dépliage plein-registre.
    "WITH keys AS ( \
         SELECT DISTINCT p.resolve_key FROM decision_party p \
         WHERE p.entity_uid IS NULL \
           AND p.quality IN ('party', 'law_firm') \
           AND p.decision_id = ANY($1) \
     ), hit AS ( \
         SELECT k.resolve_key AS folded, min(h.uid) AS uid \
         FROM keys k \
         CROSS JOIN LATERAL ( \
             SELECT e.uid FROM entity e \
             WHERE lj_fold_all(e.denominations) @> ARRAY[k.resolve_key] \
               AND e.uid NOT LIKE 'cnb:%' \
               AND e.uid NOT LIKE 'oacc:%' \
             OFFSET 0) h \
         GROUP BY k.resolve_key \
         HAVING count(DISTINCT h.uid) = 1 \
     ) \
     UPDATE decision_party p SET entity_uid = h.uid \
     FROM hit h \
     WHERE p.entity_uid IS NULL \
       AND p.quality IN ('party', 'law_firm') \
       AND p.resolve_key = h.folded",
    // Avocats aux Conseils CC/CE : registre `oacc:` minuscule (~140) — scope seul, le
    // dépliage reste trivial. La sonde par namespace exige `entity_uid_pattern_idx`,
    // sans quoi `LIKE 'oacc:%'` seq-scanne les 29 M lignes d'`entity`.
    "WITH pend AS ( \
         SELECT DISTINCT p.resolve_key AS k \
         FROM decision_party p \
         JOIN decisions d ON d.id = p.decision_id \
         WHERE p.entity_uid IS NULL \
           AND p.quality = 'counsel_name' \
           AND d.jurisdiction_type IN ('CC', 'CE') \
           AND p.decision_id = ANY($1) \
     ), variant AS ( \
         SELECT k, k AS v FROM pend \
         UNION \
         SELECT k, regexp_replace(k, '^(\\S+) (.*)$', '\\2 \\1') \
           FROM pend WHERE strpos(k, ' ') > 0 \
         UNION \
         SELECT k, regexp_replace(k, '^(.*) (\\S+)$', '\\2 \\1') \
           FROM pend WHERE strpos(k, ' ') > 0 \
     ), av AS ( \
         SELECT e.uid AS entity_uid, f.folded, \
                split_part(f.folded, ' ', -1) AS surname \
         FROM entity e, \
              LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded) \
         WHERE e.uid LIKE 'oacc:%' AND e.uid NOT LIKE 'oacc:firm-%' \
     ), cand AS ( \
         SELECT v.k, a.entity_uid FROM variant v JOIN av a ON a.folded = v.v \
         UNION \
         SELECT p.k, a.entity_uid FROM pend p JOIN av a ON a.surname = p.k \
           WHERE strpos(p.k, ' ') = 0 \
     ), pick AS ( \
         SELECT k, min(entity_uid) AS uid \
         FROM cand GROUP BY k HAVING count(DISTINCT entity_uid) = 1 \
     ) \
     UPDATE decision_party p SET entity_uid = pk.uid \
     FROM pick pk, decisions d \
     WHERE p.entity_uid IS NULL \
       AND p.quality = 'counsel_name' \
       AND p.resolve_key = pk.k \
       AND d.id = p.decision_id \
       AND d.jurisdiction_type IN ('CC', 'CE')",
    // Sociétés d'avocats aux Conseils CC/CE : sous-ensemble de tokens contre
    // `oacc:firm-%` (registre minuscule) — scope seul, sonde de namespace par
    // `entity_uid_pattern_idx`.
    "WITH pend AS ( \
         SELECT DISTINCT p.resolve_key AS k \
         FROM decision_party p \
         JOIN decisions d ON d.id = p.decision_id \
         WHERE p.entity_uid IS NULL \
           AND p.quality = 'law_firm' \
           AND d.jurisdiction_type IN ('CC', 'CE') \
           AND p.decision_id = ANY($1) \
     ), ptok AS ( \
         SELECT k, t.tok \
         FROM pend, LATERAL regexp_split_to_table(k, '[^[:alnum:]]+') AS t(tok) \
         WHERE length(t.tok) >= 2 AND t.tok NOT IN ( \
             'scp','sarl','sarlu','sas','selas','selarl','selafa', \
             'selasu','aarpi','societe','cabinet','avocat','avocats', \
             'associe','associes','et','ou','de','du','des','la','le', \
             'les','au','aux') \
         GROUP BY k, t.tok \
     ), nkey AS ( \
         SELECT k, count(*) AS nt FROM ptok GROUP BY k \
     ), ftok AS MATERIALIZED ( \
         SELECT e.uid AS entity_uid, t.tok \
         FROM entity e, \
              LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded), \
              LATERAL regexp_split_to_table(f.folded, '[^[:alnum:]]+') AS t(tok) \
         WHERE e.uid LIKE 'oacc:firm-%' AND t.tok <> '' \
         GROUP BY e.uid, t.tok \
     ), hit AS ( \
         SELECT pt.k, f.entity_uid \
         FROM ptok pt \
         JOIN ftok f ON f.tok = pt.tok \
         JOIN nkey nk ON nk.k = pt.k \
         GROUP BY pt.k, f.entity_uid, nk.nt \
         HAVING count(*) = nk.nt \
     ), pick AS ( \
         SELECT k, min(entity_uid) AS uid \
         FROM hit GROUP BY k HAVING count(DISTINCT entity_uid) = 1 \
     ) \
     UPDATE decision_party p SET entity_uid = pk.uid \
     FROM pick pk, decisions d \
     WHERE p.entity_uid IS NULL \
       AND p.quality = 'law_firm' \
       AND p.resolve_key = pk.k \
       AND d.id = p.decision_id \
       AND d.jurisdiction_type IN ('CC', 'CE')",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec : les builders cnb portent la résolution à date et le discriminant barreau
    /// par colonne, dans les trois portées.
    #[test]
    fn cnb_sqls_cover_scopes_and_date_rules() {
        for (name, sql) in [
            ("fullname", cnb_fullname_sql(PartyScope::All)),
            ("surname", cnb_surname_sql(PartyScope::All)),
        ] {
            assert!(sql.contains("lj_active_at"), "{name}: unicité à date");
            assert!(sql.contains("d.date_lecture"), "{name}: date au groupe");
            assert!(!sql.contains("$1"), "{name}: All sans paramètre");
            assert!(!sql.contains("split_part"), "{name}: plus de barreau d'uid");
        }
        assert!(cnb_fullname_sql(PartyScope::All).contains("ebarreau = barreau"));
        assert!(cnb_fullname_sql(PartyScope::ByDecision).contains("AND p.decision_id = ANY($1)"));
        assert!(cnb_surname_sql(PartyScope::ByKey).contains("AND p.resolve_key = ANY($1)"));
        let surname = cnb_surname_sql(PartyScope::All);
        let eligible = surname
            .split("eligible AS")
            .nth(1)
            .unwrap()
            .split("SELECT p2.resolve_key")
            .next()
            .unwrap();
        assert!(!eligible.contains("lj_active_at"));
        assert!(cnb_surname_update_sql("lj_pick_surname").contains("FROM lj_pick_surname pk"));
        assert!(!cnb_surname_pick_sql(PartyScope::All, None).contains("UPDATE"));
        assert!(!cnb_fullname_pick_sql(PartyScope::All, None).contains("UPDATE"));
        for scope in [PartyScope::All, PartyScope::ByKey, PartyScope::ByDecision] {
            let sonde = cnb_fullname_pick_sql(scope, None);
            let hachage = cnb_fullname_pick_sql(scope, Some("lj_reg_cnb"));
            assert!(sonde.contains("lj_fold_all(e.denominations) @> ARRAY[v.k]"));
            assert!(!sonde.contains("lj_reg_cnb"));
            assert!(hachage.contains("JOIN lj_reg_cnb r ON r.name = v.k"));
            assert!(!hachage.contains("lj_fold_all"));
            assert!(sonde.contains("lj_active_at") && hachage.contains("lj_active_at"));
        }
        let hachage = cnb_surname_pick_sql(PartyScope::All, Some("lj_reg_cnb"));
        let garde = hachage
            .split("porte AS")
            .nth(1)
            .unwrap()
            .split("SELECT p2.resolve_key")
            .next()
            .unwrap();
        assert!(!garde.contains("lj_active_at"));
    }
}
