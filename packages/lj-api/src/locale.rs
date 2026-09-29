//! What the browser says of its user: every tag of `Accept-Language`, in its
//! order, and the country Cloudflare reads off the address. The id_token
//! `locale` claim rides along.

use axum::http::{header, HeaderMap};

use crate::error::Result;
use crate::state::AppState;

const MAX: usize = 16;
const LANGUES_MAX: usize = 8;

#[derive(Debug, Default, PartialEq)]
pub struct Navigateur {
    pub langues: Vec<String>,
    pub pays: Option<String>,
}

pub fn du_navigateur(entetes: &HeaderMap) -> Navigateur {
    let texte = |nom: &str| entetes.get(nom).and_then(|v| v.to_str().ok());
    let mut langues: Vec<String> = Vec::new();
    for etiquette in texte(header::ACCEPT_LANGUAGE.as_str())
        .unwrap_or("")
        .split(',')
        .filter_map(normaliser)
    {
        if !langues.contains(&etiquette) && langues.len() < LANGUES_MAX {
            langues.push(etiquette);
        }
    }
    Navigateur {
        langues,
        pays: texte("cf-ipcountry").and_then(pays),
    }
}

/// `XX` is Cloudflare's unknown and `T1` its Tor exit: neither is a country.
fn pays(brut: &str) -> Option<String> {
    let code = brut.trim().to_ascii_uppercase();
    (code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase()) && code != "XX")
        .then_some(code)
}

pub(crate) fn normaliser(brut: &str) -> Option<String> {
    let etiquette = brut.split(';').next()?.trim().to_ascii_lowercase();
    let forme = (2..=MAX).contains(&etiquette.len())
        && etiquette
            .split('-')
            .all(|bloc| !bloc.is_empty() && bloc.chars().all(|c| c.is_ascii_alphanumeric()));
    forme.then_some(etiquette)
}

/// The longest language list seen wins, so a row carrying only the first tag
/// of an older capture fills in at the next visit. The country and the
/// provider locale are set once.
pub async fn retenir(
    state: &AppState,
    sub: &str,
    navigateur: &Navigateur,
    fournisseur: Option<&str>,
) -> Result<()> {
    if navigateur == &Navigateur::default() && fournisseur.is_none() {
        return Ok(());
    }
    let langues = (!navigateur.langues.is_empty()).then_some(&navigateur.langues);
    let conn = state.pool.get().await?;
    conn.execute(
        "UPDATE users SET \
           browser_languages = CASE \
             WHEN cardinality($2::text[]) > coalesce(cardinality(browser_languages), 0) \
             THEN $2 ELSE browser_languages END, \
           country = coalesce(country, $3), \
           provider_locale = coalesce(provider_locale, $4) \
          WHERE sub = $1 \
            AND (cardinality($2::text[]) > coalesce(cardinality(browser_languages), 0) \
              OR (country IS NULL AND $3::text IS NOT NULL) \
              OR (provider_locale IS NULL AND $4::text IS NOT NULL))",
        &[&sub, &langues, &navigateur.pays, &fournisseur],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toute_la_liste_dans_son_ordre_sans_doublon() {
        let mut entetes = HeaderMap::new();
        entetes.insert(
            header::ACCEPT_LANGUAGE,
            "en-US,en;q=0.9,fr;q=0.8,*;q=0.5,EN".parse().unwrap(),
        );
        entetes.insert("cf-ipcountry", "fr".parse().unwrap());
        assert_eq!(
            du_navigateur(&entetes),
            Navigateur {
                langues: vec!["en-us".into(), "en".into(), "fr".into()],
                pays: Some("FR".into()),
            }
        );
    }

    #[test]
    fn entete_absent_ou_sans_etiquette() {
        assert_eq!(du_navigateur(&HeaderMap::new()), Navigateur::default());
        let mut entetes = HeaderMap::new();
        entetes.insert(header::ACCEPT_LANGUAGE, "*".parse().unwrap());
        assert_eq!(du_navigateur(&entetes), Navigateur::default());
    }

    #[test]
    fn un_pays_inconnu_ou_tor_n_est_pas_un_pays() {
        for brut in ["XX", "T1", "", "FRA"] {
            assert_eq!(pays(brut), None, "{brut:?}");
        }
        assert_eq!(pays("BE").as_deref(), Some("BE"));
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
