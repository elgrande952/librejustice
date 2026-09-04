//! Liens de chronologie entre décisions : une ligne `decision_links` =
//! (décision qui attaque, type, `canonical_ref` de la décision attaquée, cible
//! résolue quand elle existe). Écrits par l'ingest (remplacement par décision,
//! comme les citations), résolus à l'écriture puis en batch
//! (`resolve_pending_decision_links`) — une décision qui arrive « attire » les
//! liens pendants qui la visent.

use std::collections::{HashMap, HashSet};

use super::types::DecisionLinkRow;
use super::DecisionRepository;
use crate::error::Result;

/// Item d'écriture bulk : `(decision_id, liens)`. `None` = extraction sans
/// couche chronologie (écrit un set vide).
pub type DecisionLinkWriteItem = (i64, Option<Vec<DecisionLinkRow>>);

impl DecisionRepository<'_> {
    /// (Ré)écrit les liens de chronologie d'UNE décision — sucre sur
    /// [`Self::replace_decision_links_bulk`].
    pub async fn replace_decision_links(
        &self,
        decision_id: i64,
        links: Option<&[DecisionLinkRow]>,
    ) -> Result<Vec<i64>> {
        self.replace_decision_links_bulk(&[(decision_id, links.map(<[_]>::to_vec))])
            .await
    }

    /// Écrit les liens d'un lot de décisions : garde jamais-dégrader
    /// (version > `EXTRACT_VERSION` = révision manuelle, jamais remplacée),
    /// skip des sets `(link_type, target_ref)` inchangés, puis DELETE + INSERT
    /// et résolution des cibles fraîchement écrites. Idempotent. Rend les
    /// décisions RÉELLEMENT réécrites, pour que l'appelant date leur page du
    /// jour.
    #[tracing::instrument(name = "db.replace_decision_links_bulk", skip(self, items), fields(db.system = "postgresql", items = items.len()))]
    pub async fn replace_decision_links_bulk(
        &self,
        items: &[DecisionLinkWriteItem],
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

        // Skip-diff : set (link_type, target_ref) identique → aucune écriture.
        let db_rows = self
            .conn
            .query(
                "SELECT decision_id, link_type, target_ref FROM decision_links
                 WHERE decision_id = ANY($1) AND extract_version <= $2",
                &[&ids, &lj_core::EXTRACT_VERSION],
            )
            .await?;
        let mut current: HashMap<i64, HashSet<(String, String)>> = HashMap::new();
        for row in &db_rows {
            current
                .entry(row.get(0))
                .or_default()
                .insert((row.get(1), row.get(2)));
        }
        let empty = HashSet::new();
        let changed: Vec<&DecisionLinkWriteItem> = items
            .iter()
            .filter(|(id, links)| {
                if protected.contains(id) {
                    return false;
                }
                let new_set: HashSet<(String, String)> = links
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|l| (l.link_type.clone(), l.target_ref.clone()))
                    .collect();
                current.get(id).unwrap_or(&empty) != &new_set
            })
            .collect();
        if changed.is_empty() {
            return Ok(Vec::new());
        }

        let changed_ids: Vec<i64> = changed.iter().map(|(id, _)| *id).collect();
        self.conn
            .execute(
                "DELETE FROM decision_links WHERE decision_id = ANY($1)",
                &[&changed_ids],
            )
            .await?;
        let mut decision_ids: Vec<i64> = Vec::new();
        let mut link_types: Vec<&str> = Vec::new();
        let mut target_refs: Vec<&str> = Vec::new();
        for (id, links) in &changed {
            for l in links.as_deref().unwrap_or(&[]) {
                decision_ids.push(*id);
                link_types.push(&l.link_type);
                target_refs.push(&l.target_ref);
            }
        }
        if !decision_ids.is_empty() {
            self.conn
                .execute(
                    "INSERT INTO decision_links
                       (decision_id, link_type, target_ref, target_decision_id, extract_version)
                     SELECT d, t, r, NULL, $4
                     FROM unnest($1::bigint[], $2::text[], $3::text[]) AS src(d, t, r)
                     ON CONFLICT (decision_id, link_type, target_ref) DO NOTHING",
                    &[
                        &decision_ids,
                        &link_types,
                        &target_refs,
                        &lj_core::EXTRACT_VERSION,
                    ],
                )
                .await?;
            self.conn
                .execute(RESOLVE_SQL_FOR_DECISIONS, &[&changed_ids])
                .await?;
            self.conn
                .execute(
                    resolve_parties_sql(" AND dl.decision_id = ANY($1)").as_str(),
                    &[&changed_ids],
                )
                .await?;
        }
        Ok(changed_ids)
    }

    /// Résout en batch tous les liens pendants dont la cible est arrivée en
    /// base : cible = décision **unique et active** au `canonical_ref` du lien
    /// (match multiple = sérielle, on ne devine pas). Renvoie le nombre de
    /// liens résolus. À appeler en fin d'ingest et par la passe hebdo.
    #[tracing::instrument(name = "db.resolve_pending_decision_links", skip(self), fields(db.system = "postgresql"))]
    pub async fn resolve_pending_decision_links(&self) -> Result<u64> {
        let mut n = self.conn.execute(RESOLVE_SQL_ALL, &[]).await?;
        n += self
            .conn
            .execute(resolve_parties_sql("").as_str(), &[])
            .await?;
        Ok(n)
    }

    /// Jumeau par CLÉ (sonde inversée) : liens pendants dont le `target_ref`
    /// est dans le lot, l'index partiel `decision_links_pending_target_idx`
    /// servant la sonde. Même sémantique d'unicité que le sweep. La famille
    /// `ca-parties|` en est absente : sa clé n'égale aucun `canonical_ref`.
    pub(super) async fn resolve_decision_links_by_key(&self, refs: &[String]) -> Result<u64> {
        let n = self.conn.execute(RESOLVE_SQL_FOR_KEYS, &[&refs]).await?;
        Ok(n)
    }
}

const RESOLVE_SQL_FOR_DECISIONS: &str = "
    UPDATE decision_links dl
    SET target_decision_id = (
        SELECT min(d.id) FROM decisions d
        WHERE d.canonical_ref = dl.target_ref AND d.deleted_at IS NULL
        HAVING count(*) = 1)
    WHERE dl.decision_id = ANY($1) AND dl.target_decision_id IS NULL";

const RESOLVE_SQL_FOR_KEYS: &str = "
    UPDATE decision_links dl
    SET target_decision_id = (
        SELECT min(d.id) FROM decisions d
        WHERE d.canonical_ref = dl.target_ref AND d.deleted_at IS NULL
        HAVING count(*) = 1)
    WHERE dl.target_ref = ANY($1) AND dl.target_decision_id IS NULL";

/// Famille `ca-parties|<cour>|<date>` : la cassation nomme la cour et la date de
/// l'arrêt attaqué, jamais son RG ; les parties tranchent entre les 42 arrêts que
/// cour et date rendent (1 100 justes, 24 faux sur 3 000, 3,5 ms le lien pendant).
///
/// Les personnes physiques sont hors de la jointure : le pseudonyme est
/// réattribué par décision, donc « Vincent X... » en cassation et
/// « Vincent X... » en appel sont deux personnes différentes. Le garde de
/// longueur seul ne suffit pas, il n'écarte que l'ère à deux lettres.
fn resolve_parties_sql(scope: &str) -> String {
    format!(
        "WITH pendant AS (
             SELECT dl.id AS lien, dl.decision_id AS amont,
                    split_part(dl.target_ref, '|', 2) AS cour,
                    split_part(dl.target_ref, '|', 3)::date AS jour
             FROM decision_links dl
             WHERE dl.target_decision_id IS NULL
               AND dl.target_ref LIKE 'ca-parties|%'{scope}
         ), candidat AS (
             SELECT p.lien, d.id AS cible
             FROM pendant p
             JOIN decisions d
               ON d.jurisdiction_code = p.cour
              AND d.date_lecture = p.jour
              AND d.deleted_at IS NULL
             WHERE EXISTS (
                 SELECT 1 FROM decision_party a
                 JOIN decision_party b
                   ON b.decision_id = d.id AND b.quality = 'party'
                  AND b.nature IS DISTINCT FROM 'physique'
                  AND ((a.entity_uid IS NOT NULL AND a.entity_uid = b.entity_uid)
                    OR (length(lj_party_fold(a.resolve_key)) >= 4
                        AND lj_party_fold(a.resolve_key) = lj_party_fold(b.resolve_key)))
                 WHERE a.decision_id = p.amont AND a.quality = 'party'
                   AND a.nature IS DISTINCT FROM 'physique')
         ), tranche AS (
             SELECT lien, min(cible) AS cible FROM candidat
             GROUP BY lien HAVING count(DISTINCT cible) = 1
         )
         UPDATE decision_links dl SET target_decision_id = t.cible
         FROM tranche t WHERE dl.id = t.lien"
    )
}

const RESOLVE_SQL_ALL: &str = "
    UPDATE decision_links dl
    SET target_decision_id = m.id
    FROM (
        SELECT p.id AS link_id,
               (SELECT min(d.id) FROM decisions d
                WHERE d.canonical_ref = p.target_ref AND d.deleted_at IS NULL
                HAVING count(*) = 1) AS id
        FROM decision_links p
        WHERE p.target_decision_id IS NULL
    ) m
    WHERE dl.id = m.link_id AND m.id IS NOT NULL";
