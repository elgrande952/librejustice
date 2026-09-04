//! Citations de jurisprudence : une occurrence = une ligne `case_citation`
//! `(decision_id, char_start, char_end, target_ref, target_decision_id,
//! extract_version)`. `target_ref` = clé pendante par famille ;
//! `target_decision_id` posé par la résolution SQL (à l'écriture puis relink
//! post-ingest — une cible peut arriver après ses citations).
//!
//! Écriture par le patron de `citations.rs` : garde jamais-dégrader, skip-diff des
//! sets inchangés (préserve les cibles déjà résolues), DELETE + COPY binaire.

use std::collections::{HashMap, HashSet};

use super::types::{CaseCitationRow, CitingDecisionRow};
use super::DecisionRepository;
use crate::error::{Result, StoreError};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;

/// Item d'écriture bulk : `(decision_id, citations)`. `None` = extraction
/// sans couche jurisprudence (écrit un set vide).
pub type CaseCitationWriteItem = (i64, Option<Vec<CaseCitationRow>>);

/// Empreinte d'une occurrence pour le skip-diff : `(char_start, char_end,
/// target_ref)` — `target_decision_id` est un état de résolution, pas une
/// donnée d'extraction, il ne re-déclenche pas d'écriture.
type CaseFingerprint = (i32, i32, String);

/// Couverture chronologie d'un type de juridiction : `(type, décisions,
/// décisions portant au moins un lien amont, liens émis, liens résolus)`.
pub type ChronoCoverageRow = (String, i64, i64, i64, i64);

/// Clé du verrou advisory transactionnel de la sonde inversée
/// ([`DecisionRepository::attract_pending_citations`]) — hors de la plage des
/// ids de décisions (le verrou par décision d'`apply_provenance` partage le
/// même keyspace `pg_advisory_xact_lock(bigint)`).
const ATTRACT_LOCK_KEY: i64 = 0x6c6a_5f61_7474_7261; // « lj_attra »

impl DecisionRepository<'_> {
    /// (Ré)écrit les citations de jurisprudence d'UNE décision — sucre sur
    /// [`Self::replace_case_citations_bulk`].
    pub async fn replace_case_citations(
        &self,
        decision_id: i64,
        citations: Option<&[CaseCitationRow]>,
    ) -> Result<Vec<i64>> {
        self.replace_case_citations_bulk(&[(decision_id, citations.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les citations de jurisprudence d'un lot de décisions : garde
    /// jamais-dégrader (version > `EXTRACT_VERSION` = révision manuelle, jamais
    /// remplacée), skip-diff des sets `(char_start, char_end, target_ref)` inchangés
    /// (un set inchangé garde ses cibles résolues), puis DELETE + COPY binaire. Les
    /// lignes réécrites repartent pendantes (`target_decision_id` NULL) — la résolution
    /// par famille les reprend. **La transaction appartient à l'appelant.** Idempotent.
    /// Rend les décisions RÉELLEMENT réécrites, pour que l'appelant date leur page du
    /// jour.
    #[tracing::instrument(name = "db.replace_case_citations_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_case_citations_bulk(
        &self,
        items: &[CaseCitationWriteItem],
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
                "SELECT decision_id, char_start, char_end, target_ref FROM case_citation
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<CaseFingerprint>> = HashMap::new();
        for row in &db_rows {
            current
                .entry(row.get(0))
                .or_default()
                .insert((row.get(1), row.get(2), row.get(3)));
        }
        let empty = HashSet::new();
        let changed: Vec<&CaseCitationWriteItem> = items
            .iter()
            .filter(|(id, citations)| {
                if protected.contains(id) {
                    return false;
                }
                let new_set: HashSet<CaseFingerprint> = citations
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|c| (c.char_start, c.char_end, c.target_ref.clone()))
                    .collect();
                current.get(id).unwrap_or(&empty) != &new_set
            })
            .collect();
        if changed.is_empty() {
            return Ok(Vec::new());
        }

        for (decision_id, citations) in &changed {
            let mut prev_end = i32::MIN;
            for c in citations.as_deref().unwrap_or(&[]) {
                if c.char_start >= c.char_end || c.char_start < prev_end {
                    return Err(StoreError::Invalid(format!(
                        "case_citations invalides (décision {decision_id}) : span \
                         [{}, {}) après end={prev_end} — tri/chevauchement violé",
                        c.char_start, c.char_end
                    )));
                }
                prev_end = c.char_end;
            }
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM case_citation WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let has_rows = changed
            .iter()
            .any(|(_, c)| !c.as_deref().unwrap_or(&[]).is_empty());
        let sink = self
            .conn
            .copy_in(
                "COPY case_citation (decision_id, char_start, char_end, target_ref, \
                 extract_version) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[Type::INT8, Type::INT4, Type::INT4, Type::TEXT, Type::INT2],
        );
        tokio::pin!(writer);
        for (decision_id, citations) in &changed {
            for c in citations.as_deref().unwrap_or(&[]) {
                writer
                    .as_mut()
                    .write(&[
                        decision_id,
                        &c.char_start,
                        &c.char_end,
                        &c.target_ref,
                        &lj_core::EXTRACT_VERSION,
                    ])
                    .await?;
            }
        }
        writer.finish().await?;
        if has_rows {
            self.resolve_case_citations_scoped(Some(&changed_ids))
                .await?;
        }
        Ok(changed_ids)
    }

    /// Résout en batch toutes les citations pendantes dont la cible est en base
    /// (relink post-ingest / passe hebdo — une cible peut arriver après ses citations).
    /// Renvoie le nombre de citations résolues.
    #[tracing::instrument(name = "db.resolve_pending_case_citations", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_case_citations(&self) -> Result<u64> {
        self.resolve_case_citations_scoped(None).await
    }

    /// Résolution par famille contre les `docket_numbers` des cibles (GIN), clé
    /// reformatée côté requête au format stocké. Match unique → `target_decision_id` ;
    /// CJUE multi-match → snap au document ARRÊT (type CELEX `J` du `source_uid`,
    /// jamais les conclusions AG) ; ambigu ou introuvable → reste pendant, relinkable.
    /// `ids` = portée at-write (lignes fraîchement écrites), `None` = tout le pendant.
    async fn resolve_case_citations_scoped(&self, ids: Option<&[i64]>) -> Result<u64> {
        let scope = match ids {
            Some(_) => ResolveScope::ByDecision,
            None => ResolveScope::All,
        };
        let mut resolved = 0u64;
        let mut sqls = resolve_case_family_sqls(scope);
        sqls.push(resolve_from_links_sql(scope));
        for sql in sqls {
            let n = match ids {
                Some(ids) => self.conn.execute(sql.as_str(), &[&ids]).await?,
                None => self.conn.execute(sql.as_str(), &[]).await?,
            };
            resolved += n;
        }
        Ok(resolved)
    }

    /// Résolution par CLÉ pendante (sonde inversée) : familles + pont, bornés
    /// aux `target_ref` d'un lot de clés candidates. Même sémantique d'unicité
    /// que le sweep (les SQL de famille font foi).
    async fn resolve_case_citations_by_key(&self, keys: &[String]) -> Result<u64> {
        let mut resolved = 0u64;
        let mut sqls = resolve_case_family_sqls(ResolveScope::ByKey);
        sqls.push(resolve_from_links_sql(ResolveScope::ByKey));
        for sql in sqls {
            resolved += self.conn.execute(sql.as_str(), &[&keys]).await?;
        }
        Ok(resolved)
    }

    /// (Ré)écrit les citations de jurisprudence d'un TEXTE du référentiel : DELETE +
    /// COPY au grain `owner_text_uid` (corps + tous ses articles d'un coup —
    /// l'extraction passe le texte entier). Les lignes repartent pendantes ; la
    /// résolution se lance en fin de passe
    /// ([`Self::resolve_pending_text_case_citations`]). Idempotent.
    pub async fn replace_text_case_citations(
        &self,
        owner_text_uid: &str,
        rows: &[super::types::TextCaseCitationRow],
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM text_case_citation WHERE owner_text_uid = $1",
                &[&owner_text_uid],
            )
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let sink = self
            .conn
            .copy_in(
                "COPY text_case_citation (owner_text_uid, owner_num_key, owner_date_debut, \
                 char_start, char_end, target_ref, extract_version) FROM STDIN (FORMAT binary)",
            )
            .await?;
        let writer = BinaryCopyInWriter::new(
            sink,
            &[
                Type::TEXT,
                Type::TEXT,
                Type::DATE,
                Type::INT4,
                Type::INT4,
                Type::TEXT,
                Type::INT2,
            ],
        );
        tokio::pin!(writer);
        for r in rows {
            writer
                .as_mut()
                .write(&[
                    &owner_text_uid,
                    &r.owner_num_key,
                    &r.owner_date_debut,
                    &r.char_start,
                    &r.char_end,
                    &r.target_ref,
                    &lj_core::EXTRACT_VERSION,
                ])
                .await?;
        }
        writer.finish().await?;
        Ok(())
    }

    /// Résout les `text_case_citation` pendantes : mêmes familles SQL que
    /// `case_citation` (colonnes cibles identiques par construction) — seul le nom de
    /// table change. Pas de pont `decision_links` (les textes n'ont pas de
    /// chronologie).
    #[tracing::instrument(name = "db.resolve_pending_text_case_citations", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_text_case_citations(&self) -> Result<u64> {
        let mut resolved = 0u64;
        for sql in resolve_case_family_sqls(ResolveScope::All) {
            let sql = sql.replace("case_citation", "text_case_citation");
            resolved += self.conn.execute(sql.as_str(), &[]).await?;
        }
        Ok(resolved)
    }

    /// Jumeau par CLÉ de [`Self::resolve_pending_text_case_citations`] (sonde
    /// inversée) — mêmes familles, table `text_case_citation`, sans pont.
    async fn resolve_text_case_citations_by_key(&self, keys: &[String]) -> Result<u64> {
        let mut resolved = 0u64;
        for sql in resolve_case_family_sqls(ResolveScope::ByKey) {
            let sql = sql.replace("case_citation", "text_case_citation");
            resolved += self.conn.execute(sql.as_str(), &[&keys]).await?;
        }
        Ok(resolved)
    }

    /// Sonde inversée des pendants (à l'arrivée d'un lot de décisions CIBLES) : `keys`
    /// = clés candidates par famille (`lj_extract::identity::pending_target_keys`,
    /// inverse des reformatages des SQL de résolution), `canonical_refs` = identités
    /// des cibles (clés de chronologie et clés `contested`). Sonde les index pendants
    /// (`*_pending_target_idx`), puis ne rejoue les résolveurs par clé que sur les
    /// hits : liens de chronologie d'abord, familles + pont ensuite, citations
    /// texte→décision enfin. Zéro hit (cas courant) = zéro UPDATE. **Tourne dans la
    /// transaction de l'appelant** ; sérialisée inter-workers par un verrou advisory
    /// transactionnel (les UPDATE par clé sont corpus-wide, deux lots croisés
    /// s'attendent au lieu de se deadlocker). Renvoie le nombre de pendants résolus.
    #[tracing::instrument(name = "db.attract_pending_citations", skip_all, fields(db.system = "postgresql", keys = keys.len(), refs = canonical_refs.len()))]
    pub async fn attract_pending_citations(
        &self,
        keys: &[String],
        canonical_refs: &[String],
    ) -> Result<u64> {
        // Clés candidates côté citations : familles + `contested`/chronologie
        // (une citation `contested` porte la clé canonical_ref de sa cible).
        let citation_keys: Vec<String> = keys.iter().chain(canonical_refs).cloned().collect();
        let case_hits = self.probe_pending("case_citation", &citation_keys).await?;
        let text_hits = self.probe_pending("text_case_citation", keys).await?;
        let link_hits = self.probe_pending("decision_links", canonical_refs).await?;
        if case_hits.is_empty() && text_hits.is_empty() && link_hits.is_empty() {
            return Ok(0);
        }
        self.conn
            .execute("SELECT pg_advisory_xact_lock($1)", &[&ATTRACT_LOCK_KEY])
            .await?;
        let mut resolved = 0u64;
        if !link_hits.is_empty() {
            resolved += self.resolve_decision_links_by_key(&link_hits).await?;
        }
        if !case_hits.is_empty() {
            resolved += self.resolve_case_citations_by_key(&case_hits).await?;
        }
        if !text_hits.is_empty() {
            resolved += self.resolve_text_case_citations_by_key(&text_hits).await?;
        }
        Ok(resolved)
    }

    /// Décisions dont le texte cite celle-ci, autorité d'abord puis date
    /// décroissante, comme les citantes d'un article. Une citante qui cite
    /// plusieurs fois ne sort qu'une fois. `exclues` sort les étapes de la
    /// chronologie, décision courante comprise. Rend `(total, page de tête)`.
    #[tracing::instrument(name = "db.decisions_citing", skip(self, exclues), fields(db.system = "postgresql", exclues = exclues.len()))]
    pub async fn decisions_citing(
        &self,
        decision_id: i64,
        exclues: &[i64],
        limite: i64,
    ) -> Result<(i64, Vec<CitingDecisionRow>)> {
        let rank = super::support::authority_rank_sql("d.publication_codes");
        let rows = self
            .conn
            .query(
                &format!(
                    "WITH citantes AS ( \
                         SELECT DISTINCT c.decision_id AS id FROM case_citation c \
                         WHERE c.target_decision_id = $1 \
                     ) \
                     SELECT d.id, d.public_id, d.jurisdiction_type, j.label, \
                            d.date_lecture::text, d.docket_numbers, \
                            d.publication_codes, d.summary, count(*) OVER () \
                     FROM citantes ci \
                     JOIN decisions d ON d.id = ci.id AND d.deleted_at IS NULL \
                     LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code \
                     WHERE NOT (d.id = ANY($2)) \
                     ORDER BY {rank}, d.date_lecture::text DESC NULLS LAST, d.id \
                     LIMIT $3"
                ),
                &[&decision_id, &exclues, &limite],
            )
            .await?;
        let total = rows.first().map_or(0, |r| r.get(8));
        Ok((
            total,
            rows.iter()
                .map(|r| CitingDecisionRow {
                    id: r.get(0),
                    public_id: r.get(1),
                    jurisdiction_type: r.get(2),
                    jurisdiction_name: r.get(3),
                    date_lecture: r.get(4),
                    docket_numbers: r.get(5),
                    publication_codes: r.get(6),
                    summary: r.get(7),
                    cited_num_keys: Vec::new(),
                })
                .collect(),
        ))
    }

    /// `source_uid` des décisions dont le greffe déclare lui-même la décision
    /// attaquée (`contested` complet). Cet oracle ne doit rien au texte, donc la
    /// voie texte se mesure contre lui sans circularité. Tri sur le `md5` du
    /// `source_uid` : échantillon stable d'un run à l'autre et non corrélé à la
    /// date, là où l'ordre naturel de l'uid Judilibre ne rendrait que les plus
    /// anciennes.
    #[tracing::instrument(name = "db.contested_oracle_uids", skip(self), fields(db.system = "postgresql"))]
    pub async fn contested_oracle_uids(
        &self,
        jurisdiction_type: &str,
        limite: i64,
    ) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT s.source_uid FROM decisions d \
                 JOIN decision_sources s ON s.decision_id = d.id AND s.deleted_at IS NULL \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_type = $1 \
                   AND s.source_fields #>> '{contested,jurisdiction}' IS NOT NULL \
                   AND s.source_fields #>> '{contested,number}' IS NOT NULL \
                   AND s.source_fields #>> '{contested,date}' IS NOT NULL \
                 ORDER BY md5(s.source_uid) LIMIT $2",
                &[&jurisdiction_type, &limite],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Panneau de couverture du banc, par type de juridiction.
    #[tracing::instrument(name = "db.chrono_coverage", skip(self), fields(db.system = "postgresql"))]
    pub async fn chrono_coverage(
        &self,
        jurisdiction_types: &[String],
    ) -> Result<Vec<ChronoCoverageRow>> {
        let rows = self
            .conn
            .query(
                "SELECT d.jurisdiction_type, count(DISTINCT d.id), \
                        count(DISTINCT l.decision_id), count(l.id), \
                        count(l.target_decision_id) \
                 FROM decisions d LEFT JOIN decision_links l ON l.decision_id = d.id \
                 WHERE d.deleted_at IS NULL AND d.jurisdiction_type = ANY($1) \
                 GROUP BY 1 ORDER BY 1",
                &[&jurisdiction_types],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
            .collect())
    }

    /// Clés pendantes effectivement présentes dans `table` parmi `keys` —
    /// index-only scan sur l'index partiel `(target_ref) WHERE
    /// target_decision_id IS NULL` (même prédicat que les résolveurs).
    async fn probe_pending(&self, table: &str, keys: &[String]) -> Result<Vec<String>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT DISTINCT target_ref FROM {table} \
                     WHERE target_decision_id IS NULL AND target_ref = ANY($1)"
                ),
                &[&keys],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }
}

/// Portée d'une passe de résolution : tout le pendant (sweep `reconcile`),
/// le pendant des décisions CITANTES d'un lot (at-write côté source, `$1` =
/// ids), ou le pendant par CLÉ (`$1` = `target_ref[]` — sonde inversée, à
/// l'arrivée de la cible).
#[derive(Clone, Copy, PartialEq)]
enum ResolveScope {
    All,
    ByDecision,
    ByKey,
}

/// Pont chronologie → citations : une citation pendante dont la clé est CELLE d'un
/// lien de chronologie résolu de la même décision hérite de sa cible. Les clés
/// `contested` (`ca|<loc>|<rg>|<date>`, `tj|…`, `tcom|…`) n'ont pas de famille
/// docket — seul ce pont les résout. Les liens sont écrits et résolus AVANT les
/// citations dans le flux d'écriture (`decisions.rs`).
fn resolve_from_links_sql(scope: ResolveScope) -> String {
    let scope = match scope {
        ResolveScope::All => "",
        ResolveScope::ByDecision => " AND c.decision_id = ANY($1)",
        ResolveScope::ByKey => " AND c.target_ref = ANY($1)",
    };
    format!(
        "UPDATE case_citation c \
         SET target_decision_id = dl.target_decision_id \
         FROM decision_links dl \
         WHERE dl.decision_id = c.decision_id \
           AND dl.target_ref = c.target_ref \
           AND dl.target_decision_id IS NOT NULL \
           AND c.target_decision_id IS NULL{scope}"
    )
}

/// Les huit UPDATE de résolution par famille, fonction pure (dumpable pour
/// validation psql). `scope` borne le pendant à un lot (`$1`).
fn resolve_case_family_sqls(scope: ResolveScope) -> Vec<String> {
    let scope = match scope {
        ResolveScope::All => "",
        ResolveScope::ByDecision => " AND p.decision_id = ANY($1)",
        ResolveScope::ByKey => " AND p.target_ref = ANY($1)",
    };
    [
            // CC : clé chiffres seuls (7) reformatée « 18-23.954 ».
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, \
                        substring(split_part(p.target_ref, '|', 2) from 1 for 2) || '-' || \
                        substring(split_part(p.target_ref, '|', 2) from 3 for 2) || '.' || \
                        substring(split_part(p.target_ref, '|', 2) from 5 for 3) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'cc|%' \
                   AND length(split_part(p.target_ref, '|', 2)) = 7 \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CC"),
            ),
            // CE / CEDH / CONSTIT : le numéro cité est le docket stocké.
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, split_part(p.target_ref, '|', 2) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'ce|%' \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CE"),
            ),
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, split_part(p.target_ref, '|', 2) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'cedh|%' \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CEDH"),
            ),
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, split_part(p.target_ref, '|', 2) AS docket \
                 FROM pending p WHERE p.target_ref LIKE 'constit|%' \
                 {RESOLVE_UNIQUE_BY_TYPE}",
                RESOLVE_UNIQUE_BY_TYPE = resolve_unique_by_type("CONSTIT"),
            ),
            // TC : le numéro stocké change d'époque (`00012` avant 2000, `3220`
            // jusqu'en 2004, `C4174` ensuite) et les praticiens écrivent l'une pour
            // l'autre. La clé porte l'ordinal nu (`tc_ordinal`), la sonde rejoue les
            // trois graphies contre l'index d'expression `lj_docket_keys`. La graphie
            // année-numéro des citations (`1403959`) ne résout pas, sa reconstitution
            // dépendant de l'époque.
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, \
                        ARRAY[split_part(p.target_ref, '|', 2), \
                              lpad(split_part(p.target_ref, '|', 2), 5, '0'), \
                              'c' || split_part(p.target_ref, '|', 2)] AS dockets \
                 FROM pending p WHERE p.target_ref LIKE 'tc|%' \
                 {RESOLVE_UNIQUE}",
                RESOLVE_UNIQUE = resolve_unique_matching(
                    "TC",
                    "lj_docket_keys(d.docket_numbers) && r.dockets",
                ),
            ),
            // RG (fond judiciaire) / AF (fond administratif TA/CAA) : même mécanique, scopée
            // au tribunal (jurisdiction_code de la clé). Les clés NUES `rg||NUM` (juridiction
            // non mappée) ne résolvent JAMAIS par construction — exclues d'emblée, sinon
            // chaque numéro sonde le GIN pour rien.
            resolve_by_jurisdiction_code("rg"),
            resolve_by_jurisdiction_code("af"),
            // CJUE : docket = l'AFFAIRE (arrêt + conclusions + ordonnances le partagent) —
            // match unique sinon snap au document arrêt (CELEX `6YYYY<cour>J…` du
            // source_uid). Clé nue (« aff. 6/64 ») → C-.
            format!(
                "{RESOLVE_HEAD} \
                 SELECT p.target_ref, \
                        CASE WHEN split_part(p.target_ref, '|', 2) LIKE '_-%' \
                             THEN upper(split_part(p.target_ref, '|', 2)) \
                             ELSE 'C-' || split_part(p.target_ref, '|', 2) END AS docket, \
                        CASE WHEN split_part(p.target_ref, '|', 2) LIKE '_-%' \
                             THEN upper(left(split_part(p.target_ref, '|', 2), 1)) \
                             ELSE 'C' END AS court \
                 FROM pending p WHERE p.target_ref LIKE 'cjue|%' \
                 ), resolved AS ( \
                   SELECT r.target_ref, \
                          (SELECT CASE \
                                    WHEN count(*) = 1 THEN min(c.id) \
                                    WHEN count(*) FILTER (WHERE c.is_arret) = 1 \
                                         THEN min(c.id) FILTER (WHERE c.is_arret) \
                                  END \
                           FROM (SELECT d.id, d.jurisdiction_type, EXISTS ( \
                                     SELECT 1 FROM decision_sources s \
                                     WHERE s.decision_id = d.id \
                                       AND s.source_uid LIKE 'cjue/6____' || r.court || 'J%') AS is_arret \
                                 FROM decisions d \
                                 WHERE d.deleted_at IS NULL \
                                   AND d.docket_numbers @> ARRAY[r.docket] \
                                 OFFSET 0) c \
                           WHERE c.jurisdiction_type = 'CJUE' \
                          ) AS tid \
                   FROM keys r){RESOLVE_TAIL}"
            ),
    ]
    .into_iter()
    .map(|family_sql| family_sql.replace("{SCOPE}", scope))
    .collect()
}

/// Tête commune des UPDATE de résolution : le pendant (scopé ou non) dédupliqué
/// par clé — une clé se résout une fois, quel que soit son nombre d'occurrences.
const RESOLVE_HEAD: &str = "WITH pending AS ( \
    SELECT DISTINCT p.target_ref FROM case_citation p \
    WHERE p.target_decision_id IS NULL{SCOPE} \
 ), keys AS (";

/// Queue commune : pose `target_decision_id` sur toutes les occurrences
/// pendantes de la clé résolue.
const RESOLVE_TAIL: &str = " \
 UPDATE case_citation c \
 SET target_decision_id = m.tid \
 FROM resolved m \
 WHERE c.target_ref = m.target_ref AND c.target_decision_id IS NULL \
   AND m.tid IS NOT NULL";

/// Corps « match unique par `jurisdiction_code` » : famille du fond (`rg`
/// judiciaire, `af` administratif) — la clé porte `{prefix}|{code}|{docket}`, on
/// résout le docket contre les décisions du MÊME `jurisdiction_code`. Clés nues
/// (`{prefix}||NUM`, code vide) exclues d'emblée (jamais résolubles).
///
/// Le lookup docket est clôturé (`OFFSET 0`) pour forcer le GIN seul : sans la
/// clôture, le planner le combine en `BitmapAnd` avec l'index de juridiction et
/// scanne des centaines de milliers d'entrées PAR clé. Le filtre juridiction
/// s'applique après, sur les quelques candidats du docket.
fn resolve_by_jurisdiction_code(prefix: &str) -> String {
    format!(
        "{RESOLVE_HEAD} \
         SELECT p.target_ref, split_part(p.target_ref, '|', 3) AS docket, \
                split_part(p.target_ref, '|', 2) AS code \
         FROM pending p WHERE p.target_ref LIKE '{prefix}|%' \
           AND split_part(p.target_ref, '|', 2) <> '' \
         ), resolved AS ( \
           SELECT r.target_ref, \
                  (SELECT min(x.id) FROM \
                     (SELECT d.id, d.jurisdiction_code FROM decisions d \
                      WHERE d.deleted_at IS NULL \
                        AND d.docket_numbers @> ARRAY[r.docket] \
                      OFFSET 0) x \
                   WHERE x.jurisdiction_code = r.code \
                   HAVING count(*) = 1) AS tid \
           FROM keys r){RESOLVE_TAIL}"
    )
}

fn resolve_unique_by_type(jurisdiction_type: &str) -> String {
    resolve_unique_matching(jurisdiction_type, "d.docket_numbers @> ARRAY[r.docket]")
}

/// Corps « match unique par `jurisdiction_type` ». `sonde` interroge l'index des
/// numéros depuis la ligne `r` de `keys` ; elle est clôturée (`OFFSET 0`) pour
/// que le planner s'en tienne au GIN.
fn resolve_unique_matching(jurisdiction_type: &str, sonde: &str) -> String {
    format!(
        "), resolved AS ( \
           SELECT r.target_ref, \
                  (SELECT min(x.id) FROM \
                     (SELECT d.id, d.jurisdiction_type FROM decisions d \
                      WHERE d.deleted_at IS NULL \
                        AND {sonde} \
                      OFFSET 0) x \
                   WHERE x.jurisdiction_type = '{jurisdiction_type}' \
                   HAVING count(*) = 1) AS tid \
           FROM keys r){RESOLVE_TAIL}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p lj-store resolve_case_family -- --nocapture` dumpe les
    /// huit UPDATE pour validation psql (table temporaire + clés sondes).
    #[test]
    fn resolve_case_family_sqls_cover_families_and_scope() {
        let all = resolve_case_family_sqls(ResolveScope::All);
        assert_eq!(all.len(), 8);
        for fam in [
            "'cc|%'",
            "'ce|%'",
            "'cedh|%'",
            "'constit|%'",
            "'tc|%'",
            "'rg|%'",
            "'af|%'",
            "'cjue|%'",
        ] {
            assert!(all.iter().any(|s| s.contains(fam)), "famille {fam} absente");
        }
        for s in &all {
            assert!(s.contains("UPDATE case_citation"), "pas un UPDATE : {s}");
            assert!(!s.contains("$1"), "portée $1 en mode global : {s}");
            println!("{s};\n");
        }
        let by_decision = resolve_case_family_sqls(ResolveScope::ByDecision);
        assert!(by_decision
            .iter()
            .all(|s| s.contains("p.decision_id = ANY($1)")));
        let by_key = resolve_case_family_sqls(ResolveScope::ByKey);
        assert!(by_key.iter().all(|s| s.contains("p.target_ref = ANY($1)")));
        assert!(resolve_from_links_sql(ResolveScope::ByKey).contains("c.target_ref = ANY($1)"));
    }
}
