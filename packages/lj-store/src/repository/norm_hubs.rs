//! Hubs du catalogue des normes : catalogue agrégé par fond, années couvertes
//! par un fond, page de textes d'une année — les chemins de crawl SSR `/normes`
//! → `/normes/{fond}` → `/normes/{fond}/{annee}`.
//!
//! La taxonomie nature → fond et la date de parcours vivent dans les deux
//! fragments SQL ci-dessous, les mêmes expressions que l'index qui les sert (le
//! planner ne matche un index d'expression que sur l'arbre exact). Toute
//! évolution = nouvelle migration qui ré-indexe. Les libellés et l'ordre
//! d'affichage des fonds vivent dans `lj_core::referential_labels`.

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
    /// Volumes par fond (`/normes`). Servi derrière le cache 12 h de `lj-api` ;
    /// l'ordre et les libellés sont posés par l'appelant.
    #[tracing::instrument(name = "db.norm_catalogue", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_catalogue(&self) -> Result<Vec<(String, i64)>> {
        let sql = format!(
            "SELECT {NORM_FOND_EXPR}, count(*) FROM legal_text WHERE {NORM_SCOPE} GROUP BY 1"
        );
        let rows = self.conn.query(&sql, &[]).await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Années couvertes par un fond, avec compteur, plus récente en tête ;
    /// l'entrée `None` (textes sans date de parcours) arrive en dernier
    /// (`/normes/{fond}`). Vide si le fond est inconnu — le route handler en
    /// fait un 404.
    #[tracing::instrument(name = "db.norm_fond_years", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_years(&self, fond: &str) -> Result<Vec<(Option<i32>, i64)>> {
        let sql = format!(
            "SELECT extract(year FROM {NORM_BROWSE_DATE_EXPR})::int, count(*) \
             FROM legal_text WHERE {NORM_SCOPE} AND {NORM_FOND_EXPR} = $1 \
             GROUP BY 1 ORDER BY 1 DESC NULLS LAST"
        );
        let rows = self.conn.query(&sql, &[&fond]).await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Page de textes d'un fond×année, date décroissante — ou du bucket
    /// « sans date » (`year = None`), ordre stable par id
    /// (`/normes/{fond}/{annee}?page=N`).
    #[tracing::instrument(name = "db.norm_fond_year_texts", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_year_texts(
        &self,
        fond: &str,
        year: Option<i32>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<NormTextRow>> {
        let sql = match year {
            Some(_) => format!(
                "SELECT slug, title, ({NORM_BROWSE_DATE_EXPR})::text \
                 FROM legal_text WHERE {NORM_SCOPE} AND {NORM_FOND_EXPR} = $1 \
                   AND {NORM_BROWSE_DATE_EXPR} >= make_date($2, 1, 1) \
                   AND {NORM_BROWSE_DATE_EXPR} < make_date($2 + 1, 1, 1) \
                 ORDER BY {NORM_BROWSE_DATE_EXPR} DESC, id LIMIT $3 OFFSET $4"
            ),
            None => format!(
                "SELECT slug, title, NULL::text \
                 FROM legal_text WHERE {NORM_SCOPE} AND {NORM_FOND_EXPR} = $1 \
                   AND {NORM_BROWSE_DATE_EXPR} IS NULL \
                 ORDER BY id LIMIT $2 OFFSET $3"
            ),
        };
        let rows = match year {
            Some(y) => self.conn.query(&sql, &[&fond, &y, &limit, &offset]).await?,
            None => self.conn.query(&sql, &[&fond, &limit, &offset]).await?,
        };
        Ok(rows
            .iter()
            .map(|r| NormTextRow {
                slug: r.get(0),
                title: r.get(1),
                date: r.get(2),
            })
            .collect())
    }

    /// Fond et année de parcours d'un texte, pour le maillage retour de la
    /// page `/texte/{slug}`.
    #[tracing::instrument(name = "db.norm_fond_of_text", skip(self), fields(db.system = "postgresql"))]
    pub async fn norm_fond_of_text(&self, text_uid: &str) -> Result<Option<(String, Option<i32>)>> {
        let sql = format!(
            "SELECT {NORM_FOND_EXPR}, extract(year FROM {NORM_BROWSE_DATE_EXPR})::int \
             FROM legal_text WHERE text_uid = $1 AND {NORM_SCOPE}"
        );
        let rows = self.conn.query(&sql, &[&text_uid]).await?;
        Ok(rows.first().map(|r| (r.get(0), r.get(1))))
    }

    /// Couples `(fond, [(token année, nombre de pages)])` pour la section
    /// `normes` des sitemaps, années récentes en tête. Le fond `codes` sort
    /// ici : son catalogue `/normes/codes`, sans hub année, est porté par la
    /// section `pages`.
    #[tracing::instrument(name = "db.iter_norm_hubs_for_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn iter_norm_hubs_for_sitemap(&self, page_size: i64) -> Result<NormHubPages> {
        // `$1::bigint` avant le float8 : sous `$1::float8` seul, Postgres infère
        // le paramètre en float8 et refuse d'y sérialiser l'i64.
        let sql = format!(
            "SELECT {NORM_FOND_EXPR}, extract(year FROM {NORM_BROWSE_DATE_EXPR})::int, \
                    ceil(count(*)::float8 / $1::bigint::float8)::bigint \
             FROM legal_text WHERE {NORM_SCOPE} \
             GROUP BY 1, 2 ORDER BY 1, 2 DESC NULLS LAST"
        );
        let rows = self.conn.query(&sql, &[&page_size]).await?;
        let mut out: NormHubPages = Vec::new();
        for r in rows {
            let fond: String = r.get(0);
            if fond == "codes" {
                continue;
            }
            let token = match r.get::<_, Option<i32>>(1) {
                Some(y) => y.to_string(),
                None => "sans-date".to_string(),
            };
            let bucket = (token, r.get(2));
            match out.last_mut() {
                Some((f, tokens)) if *f == fond => tokens.push(bucket),
                _ => out.push((fond, vec![bucket])),
            }
        }
        Ok(out)
    }
}
