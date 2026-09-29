//! Drift reads of the Annonces pot: what the end-of-chain invariants ask, and
//! nothing else. Each counts faulty rows whose expected count is zero.

use chrono::NaiveDate;

use crate::error::Result;

use super::DecisionRepository;

impl DecisionRepository<'_> {
    /// The last day each channel posted a row, the channel being the prefix of
    /// `source_uid` (`boamp:`, `ted:`, `decp:`, a platform engine). A channel
    /// missing from the answer has never posted anything.
    #[tracing::instrument(name = "db.derniere_moisson_par_canal", skip(self), fields(db.system = "postgresql"))]
    pub async fn derniere_moisson_par_canal(&self) -> Result<Vec<(String, NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "SELECT split_part(source_uid, ':', 1), max(first_seen_at)::date \
                 FROM annonce_source GROUP BY 1",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Markets carrying a `decp:` channel with no `details.resultat` block.
    /// `poser_resultats` writes both in one transaction, and the channel alone
    /// takes the market out of the matching queue with nothing to show.
    #[tracing::instrument(name = "db.marches_decp_sans_resultat", skip(self), fields(db.system = "postgresql"))]
    pub async fn marches_decp_sans_resultat(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT DISTINCT s.annonce_uid \
                 FROM annonce_source s JOIN annonce a ON a.uid = s.annonce_uid \
                 WHERE s.source_uid LIKE 'decp:%' AND NOT (a.details ? 'resultat')",
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }
}
