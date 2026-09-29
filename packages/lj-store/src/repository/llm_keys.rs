//! Pool de credentials (`llm_credential`), qui porte le secret lui-même, et quota
//! journalier du pool Gemini (`gemini_key_day`).

use super::DecisionRepository;
use crate::error::Result;
use chrono::{DateTime, Utc};

/// Gemini's free quota day, which resets at Pacific midnight and not at UTC.
const QUOTA_DAY: &str = "(now() AT TIME ZONE 'America/Los_Angeles')::date";

impl DecisionRepository<'_> {
    /// Gemini requests already spent today on this model, per fingerprint.
    #[tracing::instrument(
        name = "db.gemini_day_spent",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn gemini_day_spent(&self, model: &str) -> Result<Vec<(String, i32)>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT fingerprint, spent FROM gemini_key_day \
                     WHERE model = $1 AND quota_day = {QUOTA_DAY}"
                ), // sql-splice-ok: module constant
                &[&model],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Adds `deltas` to the day and returns the authoritative total, which another
    /// process on the same keys may have raised meanwhile.
    #[tracing::instrument(
        name = "db.bump_gemini_day_spent",
        skip(self, deltas),
        fields(db.system = "postgresql", keys = deltas.len())
    )]
    pub async fn bump_gemini_day_spent(
        &self,
        model: &str,
        deltas: &[(String, i32)],
    ) -> Result<Vec<(String, i32)>> {
        if deltas.is_empty() {
            return Ok(Vec::new());
        }
        let (prints, counts): (Vec<String>, Vec<i32>) = deltas.iter().cloned().unzip();
        let rows = self
            .conn
            .query(
                &format!(
                    "INSERT INTO gemini_key_day (fingerprint, model, quota_day, spent) \
                     SELECT t.f, $1, {QUOTA_DAY}, t.d \
                     FROM unnest($2::text[], $3::int[]) AS t(f, d) \
                     ON CONFLICT (fingerprint, model, quota_day) DO UPDATE \
                       SET spent = gemini_key_day.spent + EXCLUDED.spent, updated_at = now() \
                     RETURNING fingerprint, spent"
                ), // sql-splice-ok: module constant
                &[&model, &prints, &counts],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Live credentials of a pool, parked ones left out.
    #[tracing::instrument(name = "db.credentials", skip(self), fields(db.system = "postgresql"))]
    pub async fn credentials(&self, provider: &str) -> Result<Vec<Credential>> {
        let rows = self
            .conn
            .query(
                "SELECT fingerprint, provider, tenant, kind, secret, expires_at, rotated_at, account, \
                        disabled_until \
                 FROM llm_credential \
                 WHERE provider = $1 \
                   AND (disabled_until IS NULL OR disabled_until <= now()) \
                 ORDER BY tenant",
                &[&provider],
            )
            .await?;
        Ok(rows.iter().map(Credential::from_row).collect())
    }

    /// Every credential of a pool, parked ones included, for the operator's commands.
    #[tracing::instrument(
        name = "db.all_credentials",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn all_credentials(&self, provider: &str) -> Result<Vec<Credential>> {
        let rows = self
            .conn
            .query(
                "SELECT fingerprint, provider, tenant, kind, secret, expires_at, rotated_at, account, \
                        disabled_until \
                 FROM llm_credential \
                 WHERE provider = $1 \
                 ORDER BY tenant",
                &[&provider],
            )
            .await?;
        Ok(rows.iter().map(Credential::from_row).collect())
    }

    /// Fingerprints of a pool's live credentials, sorted, so that a holder can
    /// tell whether the pool moved without making the secrets travel.
    #[tracing::instrument(
        name = "db.pool_fingerprints",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn pool_fingerprints(&self, provider: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT fingerprint FROM llm_credential \
                 WHERE provider = $1 \
                   AND (disabled_until IS NULL OR disabled_until <= now()) \
                 ORDER BY fingerprint",
                &[&provider],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Live API keys of a pool, ready to hand to a client.
    pub async fn pool_api_keys(&self, provider: &str) -> Result<Vec<String>> {
        Ok(self
            .credentials(provider)
            .await?
            .iter()
            .filter_map(|c| c.api_key().map(str::to_string))
            .collect())
    }

    /// Adds a credential or refreshes what it carries. Returns true when new.
    #[tracing::instrument(
        name = "db.upsert_credential",
        skip(self, c),
        fields(db.system = "postgresql")
    )]
    pub async fn upsert_credential(&self, c: &NewCredential<'_>) -> Result<bool> {
        let rows = self
            .conn
            .query(
                "INSERT INTO llm_credential \
                   (fingerprint, provider, tenant, kind, secret, expires_at, account) \
                 VALUES ($1, $2, coalesce($3, $1), $4, $5, $6, $7) \
                 ON CONFLICT (fingerprint) DO UPDATE SET \
                   provider = EXCLUDED.provider, \
                   tenant = coalesce($3, llm_credential.tenant), kind = EXCLUDED.kind, \
                   secret = EXCLUDED.secret, expires_at = EXCLUDED.expires_at, \
                   account = coalesce(EXCLUDED.account, llm_credential.account), \
                   rotated_at = now() \
                 RETURNING (xmax = 0) AS inserted",
                &[
                    &c.fingerprint,
                    &c.provider,
                    &c.tenant,
                    &c.kind,
                    c.secret,
                    &c.expires_at,
                    &c.account,
                ],
            )
            .await?;
        Ok(rows.first().map(|r| r.get("inserted")).unwrap_or(false))
    }

    /// Writes a refreshed token only if `seen` is still the stored `rotated_at`.
    /// `None` means another process refreshed first and its tokens are the live
    /// ones: a rotating refresh token is single-use, so the loser rereads and
    /// never retries.
    #[tracing::instrument(
        name = "db.rotate_credential",
        skip(self, secret),
        fields(db.system = "postgresql")
    )]
    pub async fn rotate_credential(
        &self,
        fingerprint: &str,
        secret: &serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
        seen: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>> {
        let rows = self
            .conn
            .query(
                "UPDATE llm_credential SET secret = $2, expires_at = $3, rotated_at = now() \
                 WHERE fingerprint = $1 AND rotated_at = $4 \
                 RETURNING rotated_at",
                &[&fingerprint, secret, &expires_at, &seen],
            )
            .await?;
        Ok(rows.first().map(|r| r.get(0)))
    }

    /// Merges keys into a secret, leaving the rest as it stands. A rotated OAuth
    /// pair carries no `account_id`, and a whole-secret write would drop it.
    #[tracing::instrument(
        name = "db.merge_credential_secret",
        skip(self, secret),
        fields(db.system = "postgresql")
    )]
    pub async fn merge_credential_secret(
        &self,
        fingerprint: &str,
        secret: &serde_json::Value,
    ) -> Result<bool> {
        let rows = self
            .conn
            .query(
                "UPDATE llm_credential SET secret = secret || $2, rotated_at = now() \
                 WHERE fingerprint = $1 RETURNING fingerprint",
                &[&fingerprint, secret],
            )
            .await?;
        Ok(!rows.is_empty())
    }

    /// Names where a credential lives. `None` leaves a side as it stands, never
    /// clears it: the inventory arrives a few keys at a time.
    #[tracing::instrument(
        name = "db.name_credential",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn name_credential(
        &self,
        fingerprint: &str,
        tenant: Option<&str>,
        account: Option<&str>,
    ) -> Result<bool> {
        let rows = self
            .conn
            .query(
                "UPDATE llm_credential \
                 SET tenant = coalesce($2, tenant), account = coalesce($3, account) \
                 WHERE fingerprint = $1 RETURNING fingerprint",
                &[&fingerprint, &tenant, &account],
            )
            .await?;
        Ok(!rows.is_empty())
    }

    #[tracing::instrument(
        name = "db.forget_credential",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn forget_credential(&self, fingerprint: &str) -> Result<bool> {
        let rows = self
            .conn
            .execute(
                "DELETE FROM llm_credential WHERE fingerprint = $1",
                &[&fingerprint],
            )
            .await?;
        Ok(rows > 0)
    }

    /// Parks a credential. `None` takes the 2nd of next month, the monthly
    /// allowance resetting on the 1st and the extra day being margin.
    #[tracing::instrument(
        name = "db.disable_credential",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn disable_credential(
        &self,
        fingerprint: &str,
        until: Option<DateTime<Utc>>,
        last_status: i16,
        marked_by: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE llm_credential SET \
                   disabled_until = coalesce($2, date_trunc('month', now()) + interval '1 month 1 day'), \
                   last_status = $3, marked_by = $4 \
                 WHERE fingerprint = $1",
                &[&fingerprint, &until, &last_status, &marked_by],
            )
            .await?;
        Ok(())
    }

    /// Brings a parked credential back into its pool.
    #[tracing::instrument(
        name = "db.enable_credential",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn enable_credential(&self, fingerprint: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE llm_credential SET disabled_until = NULL WHERE fingerprint = $1",
                &[&fingerprint],
            )
            .await?;
        Ok(())
    }
}

/// What `upsert_credential` writes.
#[derive(Debug)]
pub struct NewCredential<'a> {
    pub fingerprint: &'a str,
    pub provider: &'a str,
    pub tenant: Option<&'a str>,
    pub kind: &'a str,
    pub secret: &'a serde_json::Value,
    pub expires_at: Option<DateTime<Utc>>,
    pub account: Option<&'a str>,
}

/// A pool credential. The fingerprint is xxh3-64 of the secret for an API key,
/// and of the account id for OAuth, whose secret rotates under a stable identity.
#[derive(Debug, Clone)]
pub struct Credential {
    pub fingerprint: String,
    pub provider: String,
    pub tenant: String,
    pub kind: String,
    pub secret: serde_json::Value,
    pub expires_at: Option<DateTime<Utc>>,
    pub rotated_at: DateTime<Utc>,
    pub account: Option<String>,
    pub disabled_until: Option<DateTime<Utc>>,
}

impl Credential {
    fn from_row(row: &tokio_postgres::Row) -> Self {
        Self {
            fingerprint: row.get(0),
            provider: row.get(1),
            tenant: row.get(2),
            kind: row.get(3),
            secret: row.get(4),
            expires_at: row.get(5),
            rotated_at: row.get(6),
            account: row.get(7),
            disabled_until: row.get(8),
        }
    }

    /// The API key, for a credential that carries one.
    pub fn api_key(&self) -> Option<&str> {
        self.secret.get("key")?.as_str()
    }

    /// The access token, for an OAuth credential.
    pub fn access_token(&self) -> Option<&str> {
        self.secret.get("access_token")?.as_str()
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.secret.get("refresh_token")?.as_str()
    }
}
