//! Langue de l'utilisateur : en-tête `Accept-Language` du navigateur, claim
//! `locale` de l'id_token. Posée à la première connexion qui la porte, jamais
//! réécrite.

use axum::http::{header, HeaderMap};

use crate::error::Result;
use crate::state::AppState;

const MAX: usize = 16;

pub fn du_navigateur(entetes: &HeaderMap) -> Option<String> {
    entetes
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())?
        .split(',')
        .next()
        .and_then(normaliser)
}

pub(crate) fn normaliser(brut: &str) -> Option<String> {
    let etiquette = brut.split(';').next()?.trim().to_ascii_lowercase();
    let forme = (2..=MAX).contains(&etiquette.len())
        && etiquette
            .split('-')
            .all(|bloc| !bloc.is_empty() && bloc.chars().all(|c| c.is_ascii_alphanumeric()));
    forme.then_some(etiquette)
}

pub async fn retenir(
    state: &AppState,
    sub: &str,
    navigateur: Option<&str>,
    fournisseur: Option<&str>,
) -> Result<()> {
    if navigateur.is_none() && fournisseur.is_none() {
        return Ok(());
    }
    let conn = state.pool.get().await?;
    conn.execute(
        "UPDATE users SET browser_locale = coalesce(browser_locale, $2), \
                          provider_locale = coalesce(provider_locale, $3) \
          WHERE sub = $1 \
            AND ((browser_locale IS NULL AND $2::text IS NOT NULL) \
              OR (provider_locale IS NULL AND $3::text IS NOT NULL))",
        &[&sub, &navigateur, &fournisseur],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premiere_etiquette_de_l_entete() {
        let mut entetes = HeaderMap::new();
        entetes.insert(
            header::ACCEPT_LANGUAGE,
            "fr-FR,fr;q=0.9,en;q=0.8".parse().unwrap(),
        );
        assert_eq!(du_navigateur(&entetes).as_deref(), Some("fr-fr"));
    }

    #[test]
    fn entete_absent_ou_sans_etiquette() {
        assert_eq!(du_navigateur(&HeaderMap::new()), None);
        let mut entetes = HeaderMap::new();
        entetes.insert(header::ACCEPT_LANGUAGE, "*".parse().unwrap());
        assert_eq!(du_navigateur(&entetes), None);
    }

    #[test]
    fn normalise_les_etiquettes_bien_formees() {
        for (brut, attendu) in [
            ("fr", "fr"),
            (" EN-gb ", "en-gb"),
            ("zh-Hant-TW", "zh-hant-tw"),
            ("es-419", "es-419"),
        ] {
            assert_eq!(normaliser(brut).as_deref(), Some(attendu), "{brut:?}");
        }
    }

    #[test]
    fn refuse_ce_qui_n_est_pas_une_etiquette() {
        for mauvais in [
            "",
            "*",
            "f",
            "fr-",
            "-fr",
            "fr_FR",
            ";q=0.9",
            &"a".repeat(MAX + 1),
        ] {
            assert_eq!(normaliser(mauvais), None, "{mauvais:?}");
        }
    }
}
