//! What the door a listing shows actually serves.
//!
//! A platform answers HTTP 200 to a link that no longer designates anything: a
//! landing page instead of the consultation, a rationing notice instead of the
//! file. The verdict is therefore read on the body, against the landing page of
//! the same host taken as a control.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use futures::StreamExt;
use regex::Regex;

use super::send_text_once;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// An attachment, or a body that is not a page.
    Fichier,
    /// A page of this consultation that offers the retrieval.
    Formulaire,
    /// A page of this consultation, with no retrieval offered.
    Fiche,
    /// The landing page of the platform: the identifier bought nothing.
    Accueil,
    /// The platform rationed the caller, so nothing is judged.
    Rationne,
    /// The answer says the link designates nothing.
    Mort,
    /// No answer at all: a timeout, a name that resolves nowhere, a redirect
    /// loop. Nothing is judged, and the link is not proven dead.
    Injoignable,
    /// The identifier lives in the fragment, which no server ever receives.
    Indecidable,
}

impl Verdict {
    pub fn nom(self) -> &'static str {
        match self {
            Verdict::Fichier => "fichier",
            Verdict::Formulaire => "formulaire",
            Verdict::Fiche => "fiche",
            Verdict::Accueil => "accueil",
            Verdict::Rationne => "rationne",
            Verdict::Mort => "mort",
            Verdict::Injoignable => "injoignable",
            Verdict::Indecidable => "indecidable",
        }
    }

    /// Whether a person following that link reaches the consultation.
    pub fn ouvre(self) -> bool {
        matches!(
            self,
            Verdict::Fichier | Verdict::Formulaire | Verdict::Fiche
        )
    }
}

/// Wordings by which a platform says it rations the caller rather than serving
/// the file, all three measured on the fund.
const RATIONNEMENTS: [&str; 3] = [
    "nombre maximum de téléchargement",
    "Accès temporairement restreint",
    "requêtes automatiques",
];

/// What a page that carries the retrieval names. The control comparison
/// already sets aside the landing page, which names some of these too.
const RETRAITS: [&str; 7] = [
    "télécharger",
    "telecharger",
    "retrait",
    "dossier de consultation",
    "déposer",
    "remettre",
    "répondre à la consultation",
];

pub struct Sonde<'a> {
    pub url: &'a str,
    pub statut: u16,
    pub content_type: Option<&'a str>,
    pub content_disposition: Option<&'a str>,
    pub corps: &'a str,
    /// Landing page of the host, fetched once.
    pub temoin: &'a str,
}

fn titre(html: &str) -> Option<&str> {
    static TITRE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("motif de titre"));
    TITRE.captures(html)?.get(1).map(|m| m.as_str().trim())
}

/// Same page as the control: same title, and a length within 2 %. A page of a
/// consultation differs from its landing page by its own content, so the two
/// never coincide this closely by accident.
fn meme_page(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if titre(a) != titre(b) {
        return false;
    }
    let (court, long) = if a.len() < b.len() {
        (a.len(), b.len())
    } else {
        (b.len(), a.len())
    };
    long - court <= long / 50
}

pub fn classer(s: &Sonde) -> Verdict {
    if s.statut == 0 {
        return Verdict::Injoignable;
    }
    if matches!(s.statut, 404 | 410) {
        return Verdict::Mort;
    }
    if RATIONNEMENTS.iter().any(|m| s.corps.contains(m)) {
        return Verdict::Rationne;
    }
    let piece = s
        .content_disposition
        .is_some_and(|d| d.contains("attachment"))
        || s.content_type
            .is_some_and(|c| !c.contains("text/html") && !c.contains("text/plain"));
    if piece {
        return Verdict::Fichier;
    }
    // The fragment stays on the caller's side: the answer is the same whatever
    // it holds, so the body says nothing about the identifier.
    if s.url.contains('#') {
        return Verdict::Indecidable;
    }
    if meme_page(s.corps, s.temoin) {
        return Verdict::Accueil;
    }
    let bas = s.corps.to_lowercase();
    if RETRAITS.iter().any(|m| bas.contains(m)) {
        Verdict::Formulaire
    } else {
        Verdict::Fiche
    }
}

#[derive(Debug)]
pub struct Tir {
    pub moteur: String,
    pub kind: String,
    pub url: String,
    pub verdict: Verdict,
}

/// A platform serves its own links one after another; the hosts go in
/// parallel. Same width as the census.
const HOTES_PARALLELES: usize = 8;

/// One try, short timeout: a door that hangs is not worth a chain step, and
/// the cookie jar carries the session a consultation page sets on entry.
fn client_lien() -> wreq::Client {
    super::avec_sortie(wreq::Client::builder())
        .emulation(super::NAVIGATEUR)
        .cookie_store(true)
        .redirect(wreq::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build()
        .expect("wreq client build")
}

async fn tirer(client: &wreq::Client, url: &str, temoin: &str) -> Verdict {
    let Ok(r) = client.get(url).send().await else {
        return Verdict::Injoignable;
    };
    let entete = |n: wreq::header::HeaderName| {
        r.headers()
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let content_type = entete(wreq::header::CONTENT_TYPE);
    let content_disposition = entete(wreq::header::CONTENT_DISPOSITION);
    let statut = r.status().as_u16();
    let page = content_type
        .as_deref()
        .is_none_or(|c| c.contains("text/") || c.contains("json"));
    let corps = if page {
        r.text().await.unwrap_or_default()
    } else {
        String::new()
    };
    classer(&Sonde {
        url,
        statut,
        content_type: content_type.as_deref(),
        content_disposition: content_disposition.as_deref(),
        corps: &corps,
        temoin,
    })
}

/// Probes the links given, one control fetch per host, without any escalation:
/// a rationed answer is a verdict of its own and never a retry. What the
/// budget does not reach is left unprobed rather than cut mid-flight.
pub async fn sonder(liens: &[(String, String, String)], budget: Duration) -> Vec<Tir> {
    let client = client_lien();
    let fin = Instant::now() + budget;
    let mut lots: BTreeMap<String, Vec<&(String, String, String)>> = BTreeMap::new();
    for lien in liens {
        let hote = super::discover::hote_de(&lien.2).unwrap_or_default();
        lots.entry(hote).or_default().push(lien);
    }
    let par_hote = lots.into_iter().map(|(hote, lot)| {
        let client = client.clone();
        async move {
            let racine = format!("https://{hote}/");
            let temoin = send_text_once(client.get(racine))
                .await
                .map(|(_, corps)| corps)
                .unwrap_or_default();
            let mut tirs = Vec::with_capacity(lot.len());
            for (moteur, kind, url) in lot {
                if Instant::now() >= fin {
                    break;
                }
                tirs.push(Tir {
                    moteur: moteur.clone(),
                    kind: kind.clone(),
                    url: url.clone(),
                    verdict: tirer(&client, url, &temoin).await,
                });
            }
            tirs
        }
    });
    futures::stream::iter(par_hote)
        .buffer_unordered(HOTES_PARALLELES)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{classer, Sonde, Verdict};

    fn sonde<'a>(url: &'a str, corps: &'a str, temoin: &'a str) -> Sonde<'a> {
        Sonde {
            url,
            statut: 200,
            content_type: Some("text/html; charset=UTF-8"),
            content_disposition: None,
            corps,
            temoin,
        }
    }

    const ACCUEIL: &str = "<html><head><title>Plateforme</title></head><body>Recherche de consultations. Avis d'attribution. Certificats de signature.</body></html>";

    #[test]
    fn la_page_d_accueil_servie_sous_un_identifiant_est_un_repli() {
        assert_eq!(
            classer(&sonde(
                "https://p.fr/entreprise/consultation/12",
                ACCUEIL,
                ACCUEIL
            )),
            Verdict::Accueil
        );
    }

    #[test]
    fn le_formulaire_de_retrait_se_distingue_de_la_fiche_muette() {
        let retrait = "<html><head><title>Consultation 12</title></head><body>Pour télécharger anonymement le dossier complet au format compressé Zip, cliquez ici.</body></html>";
        assert_eq!(
            classer(&sonde("https://p.fr/dce?id=12", retrait, ACCUEIL)),
            Verdict::Formulaire
        );
        let fiche = "<html><head><title>Consultation 12</title></head><body>Objet : réfection de la voirie. Date limite : 12/10/2026.</body></html>";
        assert_eq!(
            classer(&sonde("https://p.fr/avis?id=12", fiche, ACCUEIL)),
            Verdict::Fiche
        );
    }

    #[test]
    fn le_rationnement_du_serveur_n_est_pas_un_lien_mort() {
        let plafond = "<html><head><title>Plateforme</title></head><body>Le nombre maximum de téléchargement a été atteint. Veuillez contacter la hotline.</body></html>";
        assert_eq!(
            classer(&sonde("https://p.fr/dce?cle=abc", plafond, ACCUEIL)),
            Verdict::Rationne
        );
        assert!(!Verdict::Rationne.ouvre());
    }

    #[test]
    fn l_identifiant_en_fragment_ne_se_juge_pas_sur_la_reponse() {
        assert_eq!(
            classer(&sonde(
                "https://p.fr/#/salle/tender/36000",
                ACCUEIL,
                ACCUEIL
            )),
            Verdict::Indecidable
        );
    }

    #[test]
    fn l_absence_de_reponse_ne_prouve_pas_la_mort_du_lien() {
        let mut muette = sonde("https://p.fr/dce?id=12", "", ACCUEIL);
        muette.statut = 0;
        assert_eq!(classer(&muette), Verdict::Injoignable);
        assert!(!Verdict::Injoignable.ouvre());
    }

    #[test]
    fn le_fichier_se_lit_dans_l_entete_et_le_404_est_mort() {
        let mut s = sonde("https://p.fr/dce?id=12", "", ACCUEIL);
        s.content_type = Some("application/zip");
        assert_eq!(classer(&s), Verdict::Fichier);
        s.content_type = Some("text/html");
        s.content_disposition = Some("attachment; filename=\"dce.zip\"");
        assert_eq!(classer(&s), Verdict::Fichier);
        let mut mort = sonde("https://p.fr/dce?id=12", "Introuvable", ACCUEIL);
        mort.statut = 404;
        assert_eq!(classer(&mort), Verdict::Mort);
    }
}
