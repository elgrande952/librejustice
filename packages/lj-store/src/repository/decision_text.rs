//! `decision_text`, one text per decision and language.

use super::DecisionRepository;
use crate::error::{Result, StoreError};
use serde_json::Value;

/// The ISO 639-1 language of a provenance's text, from the code its ingester
/// writes in `source_fields[field]`. A source that names none is French.
pub(super) fn text_lang(source_fields: &Value, field: &str) -> Result<Option<&'static str>> {
    let Some(code) = source_fields.get(field).and_then(Value::as_str) else {
        return Ok(None);
    };
    lj_vocab::languages::iso_639_1_of_eu(&code.to_ascii_uppercase())
        .map(Some)
        .ok_or_else(|| {
            StoreError::Invalid(format!("source_fields.{field}: unknown language {code}"))
        })
}

impl DecisionRepository<'_> {
    /// Writes a provenance's text in its language, when it is the first
    /// provenance of that language.
    pub(super) async fn put_text(
        &self,
        decision_id: i64,
        source_uid: &str,
        source_fields: &Value,
        text: &str,
    ) -> Result<()> {
        let lang = text_lang(source_fields, "lang")?.unwrap_or("fr");
        self.conn
            .execute(
                "SELECT decision_text_put($1, $2, $3, $4)",
                &[&decision_id, &source_uid, &lang, &text],
            )
            .await?;
        Ok(())
    }
}
