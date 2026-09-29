//! Noms d'usage et leur écriture : résolution de l'ancre {juridiction, date,
//! numéro} contre le fonds, puis pose sous le barème `lj_case_name_rank`. Tout
//! écrivain passe par là, la projection des renvois comme un catalogue.

use chrono::{Datelike, NaiveDate};

use super::{DecisionRepository, ElectedCaseName};
use crate::error::Result;

/// The registry fields as each source writes them: Judilibre keys `chamber` and
/// `formation` at the top of its record, the opendata export
/// `Audience.Formation_Jugement`, the JADE export `META_JURI_ADMIN.FORMATION`,
/// the other sources none.
const REGISTRY_FIELDS_SQL: &str = "\
    CASE ds.source WHEN 'judilibre' THEN ds.source_fields->>'chamber' END AS chamber, \
    CASE ds.source \
         WHEN 'judilibre' THEN ds.source_fields->>'formation' \
         WHEN 'opendata' THEN ds.source_fields->'Audience'->>'Formation_Jugement' \
         WHEN 'dila-jade' THEN ds.source_fields->'META_JURI_ADMIN'->>'FORMATION' \
    END AS formation";

/// The authority source of a decision, the one the extraction reads.
const AUTHORITY_ORDER_SQL: &str =
    "ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, (ds.lang = 'eng') IS TRUE DESC, ds.source_uid ASC";

/// One row of the seat vocabulary: a seat as written, the registry keys it came
/// with, and the months the corpus spans under them.
#[derive(Debug, Clone)]
pub struct SeatSample {
    pub seat: lj_core::chamber::Seat,
    pub chamber: Option<String>,
    pub formation: Option<String>,
    pub span: Option<(i32, i32)>,
}

impl DecisionRepository<'_> {
    /// Résout des ancres contre le fonds, dans l'ordre des entrées. Les zéros
    /// de tête ne comptent pas : la source écrit `1645` pour Dehaene, que le
    /// fonds porte en `01645`.
    pub async fn resolve_docket_anchors(
        &self,
        jurisdictions: &[&str],
        dates: &[NaiveDate],
        dockets: &[&str],
    ) -> Result<Vec<Option<i64>>> {
        let rows = self
            .conn
            .query(
                "SELECT t.n, (SELECT min(d.id) FROM decisions d
                              WHERE d.jurisdiction_code = t.jur
                                AND d.date_lecture = t.lue
                                AND d.deleted_at IS NULL
                                AND EXISTS (
                                    SELECT 1
                                    FROM unnest(lj_docket_keys(d.docket_numbers)) k
                                    WHERE ltrim(k, '0') = ltrim(lower(t.num), '0')))
                 FROM unnest($1::text[], $2::date[], $3::text[])
                      WITH ORDINALITY AS t(jur, lue, num, n)
                 ORDER BY t.n",
                &[&jurisdictions, &dates, &dockets],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(1)).collect())
    }

    /// Pose des noms d'usage sans rien effacer : la forme affichée ne bouge que
    /// si le rang de la source atteint celui de l'écrivain en place, et les
    /// clés s'additionnent toujours. Une entrée sans nom n'apporte que ses
    /// clés. Rejouer n'écrit rien.
    ///
    /// Les clés d'une décision voyagent en une chaîne séparée par des virgules,
    /// que `case_name_key` ne peut produire : un tableau à deux dimensions
    /// exigerait qu'elles soient toutes en même nombre.
    pub async fn put_case_names(&self, elected: &[ElectedCaseName]) -> Result<u64> {
        let ids: Vec<i64> = elected.iter().map(|e| e.decision_id).collect();
        let names: Vec<Option<&str>> = elected.iter().map(|e| e.name.as_deref()).collect();
        let keys: Vec<String> = elected.iter().map(|e| e.keys.join(",")).collect();
        let sources: Vec<&str> = elected.iter().map(|e| e.source).collect();
        let poses = self
            .conn
            .execute(
                "WITH cible AS (
                     SELECT d.id, e.nom, e.src,
                            e.nom IS NOT NULL
                                AND lj_case_name_rank(d.case_name_source)
                                    <= lj_case_name_rank(e.src) AS gagne,
                            (SELECT array_agg(DISTINCT k ORDER BY k)
                             FROM unnest(coalesce(d.case_name_keys, '{}')
                                         || coalesce(string_to_array(nullif(e.cles, ''), ','),
                                                     '{}')) k) AS cles,
                            d.case_name AS nom_pose,
                            d.case_name_source AS src_pose,
                            d.case_name_keys AS cles_posees
                     FROM decisions d
                     JOIN unnest($1::int8[], $2::text[], $3::text[], $4::text[])
                          AS e(id, nom, cles, src) ON d.id = e.id
                 )
                 UPDATE decisions d SET
                     case_name = CASE WHEN c.gagne THEN c.nom ELSE c.nom_pose END,
                     case_name_source = CASE WHEN c.gagne THEN c.src ELSE c.src_pose END,
                     case_name_keys = c.cles
                 FROM cible c
                 WHERE d.id = c.id
                   AND (c.cles_posees IS DISTINCT FROM c.cles
                        OR (c.gagne AND (c.nom_pose IS DISTINCT FROM c.nom
                                         OR c.src_pose IS DISTINCT FROM c.src)))",
                &[&ids, &names, &keys, &sources],
            )
            .await?;
        Ok(poses)
    }

    /// Every case name key, deduplicated: 23 807 for 30 190 named decisions,
    /// 0.26 s. Exactly the domain of [`Self::case_name_match`], deletion filter
    /// included, so a query touching none of them names nothing.
    pub async fn case_name_key_set(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT k.cle
                 FROM decisions d, unnest(d.case_name_keys) AS k(cle)
                 WHERE d.case_name_keys IS NOT NULL AND d.deleted_at IS NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Parmi ces clés, celles qu'au moins `seuil` décisions emploient. Ce que le
    /// fonds répète n'est pas un nom : « Société » sort dans 1 617 388 décisions
    /// et « Ville de Paris » au-delà de 10 000, quand « Czabaj » en fait 331 et
    /// « Société Tropic Travaux Signalisation » 18. Une clé de plusieurs mots
    /// devient une phrase à slop 2, les mots-outils occupant une position dans
    /// l'index : `cour d appel` sans slop rend 209 décisions au lieu de 1 324 508.
    /// `case_name_key` ne laisse passer qu'alphanumériques et espaces, donc la
    /// guillemeter ne peut rien injecter.
    ///
    /// Le `LIMIT` fait la question binaire, et c'est lui qui la rend tenable :
    /// compter les 1,6 million de « Société » prend 1,06 s, s'arrêter au seuil
    /// 0,16 s. Par lots de 60, soit 11 s, le `statement_timeout` du pool étant
    /// à 30 s et les 1 169 mots d'une passe valant 3,6 min en tout.
    pub async fn frequent_names(&self, cles: &[&str], seuil: i64) -> Result<Vec<String>> {
        let mut courants = Vec::new();
        for lot in cles.chunks(60) {
            let rows = self
                .conn
                .query(
                    "SELECT t.cle
                     FROM unnest($1::text[]) AS t(cle)
                     WHERE (SELECT count(*) FROM
                              (SELECT 1 FROM decision_text d
                               WHERE d.full_text @@@ CASE
                                       WHEN t.cle LIKE '% %' THEN '\"' || t.cle || '\"~2'
                                       ELSE t.cle END
                               LIMIT $2) x) >= $2",
                    &[&lot, &seuil],
                )
                .await?;
            courants.extend(rows.iter().map(|r| r.get::<_, String>(0)));
        }
        Ok(courants)
    }

    /// The texts writing a case name key followed by a roman numeral I to V,
    /// one phrase query per numeral on the BM25 index: 26 ms and 36 texts for
    /// « beziers », a name a mall shares.
    pub async fn ordinal_citation_texts(&self, key: &str, romans: &[&str]) -> Result<Vec<String>> {
        let phrases: Vec<Vec<String>> = romans
            .iter()
            .map(|r| {
                key.split(' ')
                    .chain(std::iter::once(*r))
                    .map(str::to_string)
                    .collect()
            })
            .collect();
        let should: Vec<String> = (1..=phrases.len())
            .map(|i| format!("paradedb.phrase('full_text', ${i}::text[])"))
            .collect();
        let sql = format!(
            "SELECT d.full_text FROM decision_text d
             WHERE d.id @@@ paradedb.boolean(should => ARRAY[{}])
               AND d.deleted_at IS NULL
             LIMIT 300",
            should.join(", ")
        );
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = phrases
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let rows = self.conn.query(sql.as_str(), &params).await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// `(id, jurisdiction_code, docket keys, reading day ISO)` of decisions.
    pub async fn decision_anchors(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(i64, Option<String>, Vec<String>, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT d.id, d.jurisdiction_code, lj_docket_keys(d.docket_numbers),
                        to_char(d.date_lecture, 'YYYY-MM-DD')
                 FROM decisions d WHERE d.id = ANY($1)",
                &[&ids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Les noms qu'un corps peut citer : clé de nom, puis identité de la
    /// décision nommée, dont l'appelant tire la clé pendante. Une clé portée par
    /// plusieurs décisions n'en sort pas, le corps ne disant pas laquelle :
    /// 10 059 lignes sur 30 270, dont « GISTI » et « Commune de Béziers ».
    pub async fn unambiguous_case_names(
        &self,
    ) -> Result<Vec<(String, Option<String>, Vec<String>, Option<String>)>> {
        let rows = self
            .conn
            .query(
                "SELECT cle, jurisdiction_code, docket_numbers,
                        to_char(date_lecture, 'YYYY-MM-DD')
                 FROM (SELECT k.cle, d.jurisdiction_code,
                              d.docket_numbers, d.date_lecture,
                              count(*) OVER (PARTITION BY k.cle) AS porteurs
                       FROM decisions d, unnest(d.case_name_keys) AS k(cle)
                       WHERE d.case_name_keys IS NOT NULL
                         AND d.deleted_at IS NULL) s
                 WHERE porteurs = 1",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// The corpus room vocabulary: the seat as WRITTEN, and the registry fields
    /// that produced it. One row per distinct seat and registry key of the
    /// authority source, the one `seat_rows_after` reads, so a decision with
    /// three sources speaks once; with it the first and last month the corpus
    /// wrote that row, as `year * 12 + month`.
    ///
    /// A full pass over decisions joined to their sources, 55 s measured on
    /// 04/09/2026, past the pool's 30 s: the timeout is lifted for the
    /// transaction. ParadeDB's aggregate custom scan returns silently wrong
    /// counts on a BM25 table, so it is cut for the same transaction.
    pub async fn seat_vocabulary(&self) -> Result<Vec<SeatSample>> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<Vec<tokio_postgres::Row>> = async {
            self.conn
                .batch_execute(
                    "SET LOCAL statement_timeout = 0; \
                     SET LOCAL paradedb.enable_aggregate_custom_scan = off",
                )
                .await?;
            let sql = format!(
                "SELECT seat, chamber, formation, min(date_lecture), max(date_lecture) FROM (
                     SELECT d.seat, {REGISTRY_FIELDS_SQL}, d.date_lecture,
                            row_number() OVER (PARTITION BY ds.decision_id
                                               ORDER BY {AUTHORITY_ORDER_SQL}) AS rn
                     FROM decisions d
                     JOIN decision_sources ds ON ds.decision_id = d.id
                     WHERE d.seat IS NOT NULL AND d.deleted_at IS NULL
                 ) s WHERE rn = 1 GROUP BY 1, 2, 3"
            );
            Ok(self.conn.query(sql.as_str(), &[]).await?)
        }
        .await;
        match &result {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        let rows = result?;
        let month = |d: NaiveDate| d.year() * 12 + d.month0() as i32;
        Ok(rows
            .iter()
            .map(|r| {
                let seat: tokio_postgres::types::Json<lj_core::chamber::Seat> = r.get(0);
                let first: Option<NaiveDate> = r.get(3);
                let last: Option<NaiveDate> = r.get(4);
                SeatSample {
                    seat: seat.0,
                    chamber: r.get(1),
                    formation: r.get(2),
                    span: first.zip(last).map(|(a, b)| (month(a), month(b))),
                }
            })
            .collect())
    }

    /// `(id, seat, registry chamber, registry formation, stored paths)`. The
    /// registry fields come from the authority source, the one the extraction
    /// reads. A row whose seat was written outside the extraction, the gold
    /// layer being the one, has never had a path computed and carries NULL
    /// where the rest carries an array. Same absence, so the read levels it.
    pub async fn seat_rows_after(
        &self,
        after: i64,
        limit: i64,
    ) -> Result<
        Vec<(
            i64,
            lj_core::chamber::Seat,
            Option<String>,
            Option<String>,
            Vec<String>,
        )>,
    > {
        let sql = format!(
            "SELECT d.id, d.seat, s.chamber, s.formation, coalesce(d.seat_paths, '{{}}')
             FROM decisions d
             LEFT JOIN LATERAL (
                 SELECT {REGISTRY_FIELDS_SQL}
                 FROM decision_sources ds WHERE ds.decision_id = d.id
                 ORDER BY {AUTHORITY_ORDER_SQL}
                 LIMIT 1
             ) s ON true
             WHERE d.id > $1 AND d.seat IS NOT NULL AND d.deleted_at IS NULL
             ORDER BY d.id LIMIT $2"
        );
        let rows = self.conn.query(sql.as_str(), &[&after, &limit]).await?;
        Ok(rows
            .iter()
            .map(|r| {
                let seat: tokio_postgres::types::Json<lj_core::chamber::Seat> = r.get(1);
                (r.get(0), seat.0, r.get(2), r.get(3), r.get(4))
            })
            .collect())
    }

    /// `updated_at` stays: a path is a filter key, not page content, and
    /// touching it would push the row to IndexNow and the sitemap.
    pub async fn put_seat_paths(&self, items: &[(i64, Vec<String>)]) -> Result<u64> {
        let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
        let paths: Vec<String> = items.iter().map(|(_, p)| p.join(",")).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE decisions d
                 SET seat_paths = string_to_array(v.paths, ',')
                 FROM unnest($1::int8[], $2::text[]) AS v(id, paths)
                 WHERE d.id = v.id
                   AND d.seat_paths IS DISTINCT FROM string_to_array(v.paths, ',')",
                &[&ids, &paths],
            )
            .await?)
    }

    /// Efface le nom des décisions que l'écrivain ne nomme plus. Il n'efface
    /// que le sien : un nom qu'une source porte survit à la projection.
    ///
    /// Deux formes, une par écrivain. Celui qui reprojette tout à chaque passe
    /// efface par le complément de son lot ; celui qui écrit décision par
    /// décision efface les siennes, que la ré-extraction laisserait autrement
    /// sur un nom que leur source ne donne plus.
    pub async fn clear_case_names_except(
        &self,
        sources: &[&str],
        kept: &[i64],
    ) -> Result<Vec<i64>> {
        self.clear(
            sources,
            "NOT EXISTS (SELECT 1 FROM unnest($2::int8[]) k(id) WHERE k.id = d.id)",
            kept,
        )
        .await
    }

    pub async fn clear_case_names(&self, source: &str, ids: &[i64]) -> Result<Vec<i64>> {
        self.clear(&[source], "d.id = ANY($2)", ids).await
    }

    /// Rend les décisions effacées : leur titre porte encore le nom, et seul
    /// l'appelant sait le recomposer.
    async fn clear(&self, sources: &[&str], portee: &str, ids: &[i64]) -> Result<Vec<i64>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "UPDATE decisions d
                     SET case_name = NULL, case_name_keys = NULL, case_name_source = NULL
                     WHERE d.case_name_source = ANY($1) AND {portee}
                     RETURNING d.id"
                ),
                &[&sources, &ids],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }
}
