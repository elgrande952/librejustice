//! Hubs of the norms catalogue: catalogue aggregated by fond, years covered by a
//! fond, a year's page of texts, under a jurisdiction, the SSR crawl paths
//! `/norm-index/{juris}` → `/norm-index/{juris}/{fond}` →
//! `/norm-index/{juris}/{fond}/{year}`.
//!
//! The nature → fond taxonomy and the browsing date live in the two SQL fragments
//! below, the same expressions as the index serving them (the planner only matches
//! an expression index on the exact tree). Any change = a new migration that
//! reindexes. The fonds' labels and display order live in
//! `lj_norm::referential_labels`.

use super::types::{NormHubPages, NormTextRow};
use super::DecisionRepository;
use crate::error::Result;

/// Fond de parcours d'un texte — miroir exact de l'expression indexée.
pub(super) const NORM_FOND_EXPR: &str = "CASE \
    WHEN nature ILIKE 'code%' OR upper(nature) IN ('CODE', 'CONSTITUTION', 'ETAT_CIVIL') THEN 'codes' \
    WHEN jurisdiction = 'UE' OR upper(nature) IN ('DIRECTIVE', 'DIRECTIVE_EURO', 'REGLEMENTEUROPEEN', 'REGLEMENT_EURO', 'DECISION_EURO', 'AVISEURO', 'ARRETEURO', 'ARRETEEURO', 'INSTRUCTIONEURO', 'DELIBERATIONEURO', 'DECLARATIONEURO', 'LETTREEURO') THEN 'textes-ue' \
    WHEN jurisdiction = 'INTL' OR upper(nature) IN ('TRAITE', 'TI', 'PROTOCOLE') THEN 'traites' \
    WHEN upper(nature) IN ('LOI', 'LOI_ORGANIQUE', 'LOI_CONSTIT', 'LOI_PROGRAMME', 'DECRET_LOI', 'ORDONNANCE') THEN 'lois' \
    WHEN upper(nature) = 'DECRET' THEN 'decrets' \
    WHEN upper(nature) = 'ARRETE' THEN 'arretes' \
    WHEN upper(nature) IN ('IDCC', 'AVENANT', 'ACCORD', 'ACCORD_FONCTION_PUBLIQUE') THEN 'conventions-collectives' \
    WHEN upper(nature) IN ('CIRCULAIRE', 'INSTRUCTION') THEN 'circulaires' \
    WHEN upper(nature) = 'BOFIP' THEN 'bofip' \
    ELSE 'autres' \
  END";

/// Date de parcours : première date VALIDE entre `date_texte` et `date_publi`
/// (la DILA pose la sentinelle 2999-01-01 dans `date_texte` de certains JORF).
/// Miroir exact de l'expression indexée.
const NORM_BROWSE_DATE_EXPR: &str = "CASE \
    WHEN date_texte >= DATE '1500-01-01' AND date_texte < DATE '2100-01-01' THEN date_texte \
    WHEN date_publi >= DATE '1500-01-01' AND date_publi < DATE '2100-01-01' THEN date_publi \
  END";

/// Miroir du prédicat partiel de l'index.
pub(super) const NORM_SCOPE: &str = "slug IS NOT NULL";

impl DecisionRepository<'_> {
    /// Volumes per `(jurisdiction, fond)`. Served behind `lj-api`'s 12 h cache; the
    /// order and labels are set by the caller.
    #[tracing::instrument(name = "db.norm_catalogue", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_catalogue(&self) -> Result<Vec<(String, String, i64)>> {
        let sql = format!(
            "SELECT jurisdiction, {NORM_FOND_EXPR}, count(*) FROM legal_text \
             WHERE {NORM_SCOPE} GROUP BY 1, 2"
        );
        let rows = self.conn.query(&sql, &[]).await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Années couvertes par un fond, avec compteur, plus récente en tête ;
    /// l'entrée `None` (textes sans date de parcours) arrive en dernier
    /// (`/norm-index/{juris}/{fond}`). Vide si le fond est inconnu, le route
    /// handler en fait un 404.
    #[tracing::instrument(name = "db.norm_fond_years", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_years(
        &self,
        jurisdiction: &str,
        fond: &str,
    ) -> Result<Vec<(Option<i32>, i64)>> {
        let sql = format!(
            "SELECT extract(year FROM {NORM_BROWSE_DATE_EXPR})::int, count(*) \
             FROM legal_text WHERE {NORM_SCOPE} AND {NORM_FOND_EXPR} = $1 \
               AND jurisdiction = $2 \
             GROUP BY 1 ORDER BY 1 DESC NULLS LAST"
        );
        let rows = self.conn.query(&sql, &[&fond, &jurisdiction]).await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Page de textes d'un fond×année, date décroissante — ou du bucket
    /// « sans date » (`year = None`), ordre stable par id
    /// (`/norm-index/{juris}/{fond}/{year}?page=N`).
    #[tracing::instrument(name = "db.norm_fond_year_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_year_texts(
        &self,
        jurisdiction: &str,
        fond: &str,
        year: Option<i32>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<NormTextRow>> {
        let sql = match year {
            Some(_) => format!(
                "SELECT slug, title, ({NORM_BROWSE_DATE_EXPR})::text, jurisdiction \
                 FROM legal_text WHERE {NORM_SCOPE} AND {NORM_FOND_EXPR} = $1 \
                   AND jurisdiction = $5 \
                   AND {NORM_BROWSE_DATE_EXPR} >= make_date($2, 1, 1) \
                   AND {NORM_BROWSE_DATE_EXPR} < make_date($2 + 1, 1, 1) \
                 ORDER BY {NORM_BROWSE_DATE_EXPR} DESC, id LIMIT $3 OFFSET $4"
            ),
            None => format!(
                "SELECT slug, title, NULL::text, jurisdiction \
                 FROM legal_text WHERE {NORM_SCOPE} AND {NORM_FOND_EXPR} = $1 \
                   AND jurisdiction = $4 \
                   AND {NORM_BROWSE_DATE_EXPR} IS NULL \
                 ORDER BY id LIMIT $2 OFFSET $3"
            ),
        };
        let rows = match year {
            Some(y) => {
                self.conn
                    .query(&sql, &[&fond, &y, &limit, &offset, &jurisdiction])
                    .await?
            }
            None => {
                self.conn
                    .query(&sql, &[&fond, &limit, &offset, &jurisdiction])
                    .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| NormTextRow {
                slug: r.get(0),
                title: r.get(1),
                jurisdiction: r.get(3),
                date: r.get(2),
            })
            .collect())
    }

    /// Fond et année de parcours d'un texte, pour le maillage retour de la
    /// page du texte.
    #[tracing::instrument(name = "db.norm_fond_of_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_of_text(&self, text_uid: &str) -> Result<Option<(String, Option<i32>)>> {
        let sql = format!(
            "SELECT {NORM_FOND_EXPR}, extract(year FROM {NORM_BROWSE_DATE_EXPR})::int \
             FROM legal_text WHERE text_uid = $1 AND {NORM_SCOPE}"
        );
        let rows = self.conn.query(&sql, &[&text_uid]).await?;
        Ok(rows.first().map(|r| (r.get(0), r.get(1))))
    }

    /// Triples `(jurisdiction, fond, [(year token, page count)])` for the `normes`
    /// section of the sitemaps, recent years first. The `codes` fond is left out:
    /// its catalogue, without a year hub, is carried by the `pages` section.
    #[tracing::instrument(name = "db.iter_norm_hubs_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_norm_hubs_for_sitemap(&self, page_size: i64) -> Result<NormHubPages> {
        // `$1::bigint` avant le float8 : sous `$1::float8` seul, Postgres infère
        // le paramètre en float8 et refuse d'y sérialiser l'i64.
        let sql = format!(
            "SELECT jurisdiction, {NORM_FOND_EXPR}, \
                    extract(year FROM {NORM_BROWSE_DATE_EXPR})::int, \
                    ceil(count(*)::float8 / $1::bigint::float8)::bigint \
             FROM legal_text WHERE {NORM_SCOPE} \
             GROUP BY 1, 2, 3 ORDER BY 1, 2, 3 DESC NULLS LAST"
        );
        let rows = self.conn.query(&sql, &[&page_size]).await?;
        let mut out: NormHubPages = Vec::new();
        for r in rows {
            let jurisdiction: String = r.get(0);
            let fond: String = r.get(1);
            if fond == "codes" {
                continue;
            }
            let token = match r.get::<_, Option<i32>>(2) {
                Some(y) => y.to_string(),
                None => "sans-date".to_string(),
            };
            let bucket = (token, r.get(3));
            match out.last_mut() {
                Some((j, f, tokens)) if *j == jurisdiction && *f == fond => tokens.push(bucket),
                _ => out.push((jurisdiction, fond, vec![bucket])),
            }
        }
        Ok(out)
    }
}
