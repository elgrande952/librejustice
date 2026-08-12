//! Champ `usage_terms` : matérialisation des sacs de n-grammes des contextes de
//! citation sur `legal_article`, par le job `lj-ingest usage-terms`. Le pool
//! d'identités est retourné à l'appelant, qui répartit les batches sur plusieurs
//! connexions (workers) — le coût est dominé par la lecture TOAST des
//! `full_text` échantillonnés, donc I/O-bound et parallélisable.

use super::DecisionRepository;
use crate::error::Result;

impl DecisionRepository<'_> {
    /// Vide la table des sacs (le job est un overwrite complet : une identité
    /// passée sous le seuil de citations ne garde pas un sac périmé).
    pub async fn usage_terms_reset(&self) -> Result<u64> {
        let n = self
            .conn
            .execute("DELETE FROM legal_article_usage", &[])
            .await?;
        Ok(n)
    }

    /// Pool des identités citées ≥ `min_citations` fois, trié `(uid, num)` — ordre
    /// déterministe, le découpage en chunks vit chez l'appelant. `legal_citation` au
    /// grain décision (spans jsonb) : élément positionnel `[char_start, char_end,
    /// ref_text_uid, ref_num_key, suivants]`.
    ///
    /// Le scan complet de `legal_citation` dépasse le `statement_timeout` du pool
    /// (30 s) : levé localement dans une transaction dédiée (même pattern que
    /// `iter_entities_for_sitemap`).
    pub async fn usage_terms_pool(&self, min_citations: i64) -> Result<Vec<(String, String)>> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<Vec<(String, String)>> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            let rows = self
                .conn
                .query(
                    "SELECT el->>2 AS uid, el->>3 AS num
                     FROM legal_citation lc, jsonb_array_elements(lc.spans) el
                     WHERE el->>3 IS NOT NULL
                     GROUP BY 1, 2
                     HAVING count(*) >= $1
                     ORDER BY 1, 2",
                    &[&min_citations],
                )
                .await?;
            Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
        }
        .await;
        match &result {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        result
    }

    /// Matérialise les sacs d'un batch d'identités (arrays parallèles
    /// `uids`/`nums`) : ≤ 300 contextes de citation (±200 chars autour des spans,
    /// tirage déterministe par hash du `decision_id` — reproductible, sans biais
    /// temporel), n-grammes uni+bi (bigrammes joints `_`), seuil df ≥ max(2, 3 % des
    /// contextes), tf log-compressé min(⌈2·log₂(df+1)⌉, 24). Ne garde que les
    /// identités qui ont une version VIGUEUR.
    ///
    /// Un batch prend plusieurs minutes sur le corpus de prod (lecture TOAST des
    /// `full_text`) et dépasse le `statement_timeout` du pool (30 s) : levé
    /// localement dans une transaction dédiée. Batch hebdo du cron, pas de chemin
    /// requête utilisateur.
    pub async fn usage_terms_fill_batch(&self, uids: &[String], nums: &[String]) -> Result<u64> {
        self.conn.batch_execute("BEGIN").await?;
        let result = self.fill_batch_inner(uids, nums).await;
        match &result {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        result
    }

    async fn fill_batch_inner(&self, uids: &[String], nums: &[String]) -> Result<u64> {
        self.conn
            .batch_execute("SET LOCAL statement_timeout = 0")
            .await?;
        let (valid_a, norm_a) = (
            super::referential::article_valid_now("a"),
            super::referential::article_is_norm("a"),
        );
        let n = self
            .conn
            .execute(
                &format!(
                    r#"
                WITH ctx AS (
                  SELECT p.uid, p.num,
                         row_number() OVER (PARTITION BY p.uid, p.num) AS rn,
                         lower(substr(d.full_text, greatest(1, (c.el->>0)::int - 200), 200) || ' ' ||
                               substr(d.full_text, (c.el->>1)::int + 1, 200)) AS t
                  FROM unnest($1::text[], $2::text[]) AS p(uid, num)
                  CROSS JOIN LATERAL (
                    SELECT lc.decision_id, el.el
                    FROM legal_citation lc
                    CROSS JOIN LATERAL jsonb_array_elements(lc.spans)
                      WITH ORDINALITY el(el, ord)
                    WHERE public.lj_cit_terms(lc.spans) @> ARRAY[p.uid || '|' || p.num]
                      AND el.el->>2 = p.uid AND el.el->>3 = p.num
                    ORDER BY hashtext(lc.decision_id::text), el.ord
                    LIMIT 300) c
                  JOIN decisions d ON d.id = c.decision_id
                  WHERE d.full_text IS NOT NULL
                ),
                nctx AS (SELECT uid, num, count(*) AS n FROM ctx GROUP BY 1, 2),
                words AS (
                  SELECT uid, num, rn,
                         array_remove(regexp_split_to_array(
                           regexp_replace(t, '[^a-zà-ÿ -]', ' ', 'g'), '\s+'), '') AS w
                  FROM ctx
                ),
                grams AS (
                  SELECT uid, num, rn, w[i] AS g
                  FROM words, generate_subscripts(w, 1) i WHERE length(w[i]) > 1
                  UNION ALL
                  SELECT uid, num, rn, w[i] || '_' || w[i + 1]
                  FROM words, generate_subscripts(w, 1) i WHERE i + 1 <= array_length(w, 1)
                ),
                df AS (
                  SELECT g.uid, g.num, g.g, count(DISTINCT g.rn) AS df, min(nc.n) AS n
                  FROM grams g JOIN nctx nc ON nc.uid = g.uid AND nc.num = g.num
                  GROUP BY 1, 2, 3
                ),
                kept AS (
                  SELECT uid, num, g,
                         least(ceil(2 * ln(df + 1) / ln(2))::int, 24) AS tf
                  FROM df WHERE df >= greatest(2, ceil(0.03 * n))
                ),
                bags AS (
                  SELECT uid, num, string_agg(repeat(g || ' ', tf::int), '' ORDER BY g) AS bag
                  FROM kept GROUP BY uid, num
                )
                INSERT INTO legal_article_usage (text_uid, num_key, terms)
                SELECT b.uid, b.num, b.bag FROM bags b
                WHERE EXISTS (SELECT 1 FROM legal_article a
                              WHERE a.text_uid = b.uid AND a.num_key = b.num
                                AND {valid_a} AND {norm_a})
                "#
                ),
                &[&uids, &nums],
            )
            .await?;
        Ok(n)
    }
}
