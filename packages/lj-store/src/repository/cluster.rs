//! Health of the Postgres engine itself, read from SQL: what an end-of-chain
//! check must know of a cluster incident without the host journal, which the
//! `cron` container does not have.

use super::DecisionRepository;
use crate::error::Result;
use chrono::{DateTime, Utc};

impl DecisionRepository<'_> {
    /// When the cluster crash-recovered since it started, if it did. Postgres
    /// discards its statistics on a crash, so a `stats_reset` later than the
    /// postmaster start is the only SQL trace: `pg_postmaster_start_time` stays
    /// put when the father outlives the death of its children, and that is the
    /// shape the incident takes.
    pub async fn derniere_recovery_a_chaud(&self) -> Result<Option<DateTime<Utc>>> {
        let row = self
            .conn
            .query_one(
                "SELECT CASE
                          WHEN c.stats_reset
                               > pg_postmaster_start_time() + interval '1 minute'
                          THEN c.stats_reset
                        END
                   FROM pg_stat_checkpointer c",
                &[],
            )
            .await?;
        Ok(row.get(0))
    }
}
