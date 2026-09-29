//! Événements d'applicabilité (`article_scope`) : lecture des compteurs
//! candidats, versions cibles, écriture rejouable des événements. L'absence de
//! ligne = domaine par défaut de la juridiction du texte — la table ne porte
//! que l'implicite explicité (compteurs outre-mer, clauses Alsace-Moselle).

use super::DecisionRepository;
use crate::error::Result;
use chrono::NaiveDate;

/// Article compteur candidat (texte porteur du pivot d'applicabilité).
#[derive(Debug, Clone)]
pub struct CompteurArticleRow {
    pub text_uid: String,
    pub num: String,
    pub num_key: String,
    pub date_debut: NaiveDate,
    pub date_fin: Option<NaiveDate>,
    pub texte: String,
}

/// Ligne `article_scope` à écrire.
#[derive(Debug, Clone)]
pub struct ArticleScopeRow {
    pub text_uid: String,
    pub num_key: String,
    pub date_debut: NaiveDate,
    pub mode: String,
    pub scope: String,
    pub s_debut: NaiveDate,
    pub s_fin: Option<NaiveDate>,
    pub source: String,
}

impl DecisionRepository<'_> {
    /// Compteurs candidats : articles EN VIGUEUR dont le texte porte à la
    /// fois le pivot (« applicable(s) ») et le nom de la collectivité. Le
    /// parser pur (`lj_norm::compteurs`) fait le tri fin en aval.
    pub async fn compteur_candidates(&self, collectivite: &str) -> Result<Vec<CompteurArticleRow>> {
        let rows = self
            .conn
            .query(
                "SELECT text_uid, num, num_key, date_debut, date_fin, texte
                 FROM legal_article
                 WHERE status = 'VIGUEUR'
                   AND texte ~* 'applicables?'
                   AND texte ~* $1
                 ORDER BY text_uid, num_key",
                &[&collectivite],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| CompteurArticleRow {
                text_uid: r.get(0),
                num: r.get(1),
                num_key: r.get(2),
                date_debut: r.get(3),
                date_fin: r.get(4),
                texte: r.get(5),
            })
            .collect())
    }

    /// Versions de tous les articles d'un texte : `(num_key, date_debut,
    /// date_fin)`, pour la résolution d'ancre de rédaction et le dépliage
    /// des plages.
    pub async fn article_versions_of(
        &self,
        text_uid: &str,
    ) -> Result<Vec<(String, NaiveDate, Option<NaiveDate>)>> {
        let rows = self
            .conn
            .query(
                "SELECT num_key, date_debut, date_fin FROM legal_article
                 WHERE text_uid = $1 ORDER BY num_key, date_debut",
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Held version identities of a text: `(num_key, source_uid, status, start)`,
    /// the start date in ISO. Read by the chronicle audit.
    pub async fn article_version_uids_of(
        &self,
        text_uid: &str,
    ) -> Result<Vec<(String, String, String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT num_key, source_uid, status, to_char(date_debut, 'YYYY-MM-DD')
                 FROM legal_article WHERE text_uid = $1 ORDER BY num_key, date_debut",
                &[&text_uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }

    /// Remplace les événements issus d'une source donnée (rejouable) : DELETE des
    /// lignes dont `source` commence par le tag, puis INSERT. Renvoie
    /// `(supprimées, insérées)`.
    pub async fn replace_article_scopes(
        &self,
        source_tag: &str,
        rows: &[ArticleScopeRow],
    ) -> Result<(u64, u64)> {
        let pattern = format!("{}%", source_tag.replace('%', "\\%").replace('_', "\\_"));
        let deleted = self
            .conn
            .execute(
                "DELETE FROM article_scope WHERE source LIKE $1",
                &[&pattern],
            )
            .await?;
        let mut inserted = 0u64;
        for chunk in rows.chunks(500) {
            let mut sql = String::from(
                "INSERT INTO article_scope \
                 (text_uid, num_key, date_debut, mode, scope, s_debut, s_fin, source) VALUES ",
            );
            let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
            for (i, row) in chunk.iter().enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                let base = i * 8;
                sql.push_str(&format!(
                    "(${},${},${},${},${},${},${},${})",
                    base + 1,
                    base + 2,
                    base + 3,
                    base + 4,
                    base + 5,
                    base + 6,
                    base + 7,
                    base + 8
                ));
                params.push(&row.text_uid);
                params.push(&row.num_key);
                params.push(&row.date_debut);
                params.push(&row.mode);
                params.push(&row.scope);
                params.push(&row.s_debut);
                params.push(&row.s_fin);
                params.push(&row.source);
            }
            sql.push_str(" ON CONFLICT DO NOTHING");
            inserted += self.conn.execute(sql.as_str(), &params).await?;
        }
        Ok((deleted, inserted))
    }
}
