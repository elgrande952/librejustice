use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_annonces::zones::zones_departements;
use lj_annonces::{echeance_ts, normalise_departement, parse_heure, AnnonceBrute, ContenuAnnonce};
use lj_core::dates::parse_date_fr;

use super::{apres_balise, dedup_source_uid, send_bytes, send_text, txt, DetailAnnonce, MoteurRun};
use lj_sources::error::{Result, SourceError};

const BASE: &str = "https://v2.marches-securises.fr";
const ENTR: &str = "https://v2.marches-securises.fr/entreprise/";
const PER_PAGE: u64 = 10;
const SESSIONS: u64 = 12;

static CLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"cle_dce=([A-Za-z0-9_\-]+)").expect("regex clé de DCE"));

fn url_aapc(cle: &str) -> String {
    format!("{ENTR}?module=script|dl_la_s&cle_dce={cle}")
}

/// Poste le formulaire de recherche `rch_consult_ent` — ses champs exacts : les
/// cinq cases de prestation valent `1`, `date_cloture_type=0` prend toutes les
/// dates, et `mots_cles` est le seul terme. La recherche est un état de session,
/// donc l'accueil d'abord, la requête ensuite.
async fn recherche(client: &wreq::Client, mots_cles: &str) -> Result<String> {
    send_text(client.get(format!("{BASE}/")), "atline accueil").await?;
    send_text(
        client
            .post(format!(
                "{ENTR}?module=consultation|script|selection_consultation_s"
            ))
            .header("Referer", format!("{BASE}/"))
            .form(&[
                ("module", "liste_consultations"),
                ("mots_cles", mots_cles),
                ("presta_tx", "1"),
                ("presta_se", "1"),
                ("presta_fo", "1"),
                ("presta_au", "1"),
                ("presta_et", "1"),
                ("date_cloture_type", "0"),
            ]),
        "atline recherche",
    )
    .await
}

/// Les consultations que la plateforme rend pour une référence de marché. Son
/// index de recherche et sa liste énumérable ne couvrent pas le même ensemble :
/// une consultation absente de la seconde sort de la première.
pub async fn chercher(reference: &str) -> Result<Vec<AnnonceBrute>> {
    let client = super::client();
    Ok(parse_page(&recherche(&client, reference).await?))
}

/// Arme la recherche : rend (client en session, première page, total annoncé).
async fn armer() -> Result<(wreq::Client, String, u64)> {
    let client = super::client();
    let html = recherche(&client, "").await?;
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\d[\d\s]{0,7})\s*consultations?").unwrap());
    let total = TOTAL
        .captures(&txt(&html))
        .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
        .unwrap_or(0);
    Ok((client, html, total))
}

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    // Le gabarit donne une `<tr>` par champ : `tr_pa` porte l'acheteur,
    // `tr_objet` l'objet. Le suffixe `(69)` du libellé d'acheteur est facultatif,
    // et le texte plat n'offre alors plus rien qui sépare l'acheteur de l'objet.
    static ACHETEUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<tr class="tr_pa[^"]*">(.*?)</tr>"#).unwrap());
    static DEPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\((\d{2,3})\)$").unwrap());
    // Zone géographique de la prestation (`tr_dept`), en repli quand le
    // libellé d'acheteur n'a pas de suffixe : première zone de la liste.
    static ZONE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<tr class="tr_dept[^"]*">.*?<td[^>]*>\s*(\d{2,3})\b"#).unwrap()
    });
    static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Référence\s+(\S+)").unwrap());
    // La fiche porte deux références : « Référence », la clé maison d'Atline
    // (`CSTB_77_A_20260730W2_3`), et « Référence interne », celle que l'acheteur
    // a donnée à son marché (`27R004`) — la seule que le BOAMP publie aussi,
    // donc la seule qui joigne les deux canaux.
    static REF_INTERNE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)Référence interne\s*</th>\s*<td[^>]*>(.*?)</td>"#).unwrap()
    });
    static OBJET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<tr class="tr_objet[^"]*">(.*?)</tr>"#).unwrap());
    static TYPE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Type de marché / Type de prestation\s*(.*?)\s*Procédure").unwrap()
    });
    static PROC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)Procédure\s+(.*?)\s*Zone\(s\) géo").unwrap());
    static PUB: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Date de publication sur le serveur\s*(.*?)\s*Date de clôture").unwrap()
    });
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Date de clôture\s*(?:\w+\s+)?(\d{1,2} \w+ \d{4})(?:\s*-\s*([\d:]+))?").unwrap()
    });

    let mut out = Vec::new();
    for bloc in html
        .split(r#"<table class="consultation "#)
        .skip(1)
        .map(apres_balise)
    {
        let Some(cle) = CLE.captures(bloc).map(|c| c[1].to_string()) else {
            continue;
        };
        // Le bloc porte du JS en queue : on coupe avant.
        let utile: String = match bloc.find("function objet_XMLHttpRequest") {
            Some(i) => bloc[..i].to_string(),
            None => bloc.chars().take(6_000).collect(),
        };
        let flat = txt(&utile);
        let echeance = LIM.captures(&flat).and_then(|c| {
            let d = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                d,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        });
        let nature = TYPE
            .captures(&flat)
            .and_then(|c| lj_annonces::NatureMarche::parse(&c[1]));
        let pa = ACHETEUR
            .captures(&utile)
            .map(|c| txt(&c[1]))
            .unwrap_or_default();
        let zones = zones_departements(
            DEPT.captures(&pa)
                .and_then(|c| normalise_departement(&c[1]))
                .or_else(|| {
                    ZONE.captures(&utile)
                        .and_then(|c| normalise_departement(&c[1]))
                }),
        );
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "atline",
            source_uid: format!("atline:{cle}"),
            cles_jointure: Vec::new(),
            reference: REF_INTERNE
                .captures(&utile)
                .map(|c| txt(&c[1]).trim().to_string())
                .filter(|r| !r.is_empty())
                .or_else(|| REF.captures(&flat).map(|c| c[1].to_string())),
            acheteur: Some(DEPT.replace(&pa, "").trim().to_string()).filter(|a| !a.is_empty()),
            siren: None,
            objet: OBJET
                .captures(&utile)
                .map(|c| txt(&c[1]).chars().take(400).collect::<String>())
                .unwrap_or_default(),
            nature,
            cpv: None,
            zones,
            date_publication: PUB.captures(&flat).and_then(|c| parse_date_fr(&c[1])),
            echeance,
            url_avis: Some(format!(
                "{ENTR}?module=consultation|details&cle_dce={cle}&version=QR"
            )),
            url_rc: Some(format!("{ENTR}?module=script|dl_la_s&cle_dce={cle}")),
            url_dce: Some(format!("{ENTR}?module=script|dl_dce_s&cle_dce={cle}")),
            url_depot: None,
            procedure: PROC.captures(&flat).map(|c| c[1].trim().to_string()),
        });
    }
    out
}

/// Une tranche de pages : chaque session doit ré-armer la recherche.
async fn tranche(lo: u64, hi: u64) -> Result<Vec<AnnonceBrute>> {
    let (client, premiere, _) = armer().await?;
    let mut rows = Vec::new();
    if lo == 1 {
        rows.extend(parse_page(&premiere));
    }
    for p in lo.max(2)..=hi {
        let html = send_text(
            client
                .get(ENTR)
                .query(&[("module", "liste_consultations"), ("page", &p.to_string())]),
            &format!("atline page {p}"),
        )
        .await?;
        let got = parse_page(&html);
        if got.is_empty() {
            break;
        }
        rows.extend(got);
    }
    Ok(rows)
}

/// Ré-énumère le stock actif.
pub async fn enumerate() -> Result<MoteurRun> {
    let (_, premiere, total) = armer().await?;
    let npages = total.div_ceil(PER_PAGE).max(1);
    let pas = npages.div_ceil(SESSIONS);
    let tranches: Vec<(u64, u64)> = (1..=npages)
        .step_by(pas as usize)
        .map(|i| (i, (i + pas - 1).min(npages)))
        .collect();
    let resultats: Vec<Result<Vec<AnnonceBrute>>> =
        futures::stream::iter(tranches.into_iter().map(|(lo, hi)| tranche(lo, hi)))
            .buffer_unordered(SESSIONS as usize)
            .collect()
            .await;
    let mut run = MoteurRun {
        annonce: Some(total),
        echecs: 0,
        hotes: Vec::new(),
        rows: parse_page(&premiere),
    };
    for res in resultats {
        match res {
            Ok(rows) => run.rows.extend(rows),
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(error = %e, "atline: slice failed, stock kept");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

/// The renderer stamps its header and its footer on every page, inside the
/// value the page break cut in two.
fn sans_ornement(texte: &str) -> String {
    static ORNEMENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(?:marches-securises\.fr|page \d+/\d+|avis de publicité(?: \d+-\d+)?|\d{2}/\d{2}/\d{4}|publication|établissement)$",
        )
        .expect("regex ornement de page")
    });
    texte
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !ORNEMENT.is_match(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The notice of a consultation, which the platform serves as the AAPC its
/// buyer published, and which carries everything the listing row does not.
///
/// No consultation page is read: `consultation|details`, like
/// `consultation|qr`, serves the anonymous DCE access page, the same for every
/// consultation and empty of any notice field. What the buyer files under the
/// AAPC is its own: a standard form half the time, and otherwise a scan, from
/// which poppler renders the page and nothing else.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let cle = CLE
        .captures(url_avis)
        .map(|c| c[1].to_string())
        .ok_or_else(|| {
            SourceError::Invalid(format!("atline: URL d'avis sans clé « {url_avis} »"))
        })?;
    let octets = send_bytes(super::client().get(url_aapc(&cle)), "atline avis").await?;
    tokio::task::spawn_blocking(move || lire_avis(&octets))
        .await
        .map_err(|e| SourceError::Invalid(format!("atline avis: {e}")))?
}

/// A buyer that published several pieces at once files them as one archive, of
/// which the notice is whichever reads as a form: the others are the rulebook,
/// the contract terms and the candidacy forms.
const PIECES: usize = 5;

fn lire_avis(octets: &[u8]) -> Result<DetailAnnonce> {
    if !octets.starts_with(b"PK\x03\x04") {
        // The door answers its refusal as 90 bytes of bare text, which poppler
        // reports as `pdftotext code Some(1)`, hiding what the server said.
        if !octets.starts_with(b"%PDF") {
            return Err(SourceError::Invalid(format!(
                "atline avis: non-PDF answer, {} bytes: {}",
                octets.len(),
                String::from_utf8_lossy(&octets[..octets.len().min(120)])
            )));
        }
        return lire_piece(octets);
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(octets))?;
    let mut premiere = DetailAnnonce::default();
    for i in 0..archive.len().min(PIECES) {
        let mut entree = archive.by_index(i)?;
        if entree.is_dir() || !entree.name().to_lowercase().ends_with(".pdf") {
            continue;
        }
        let mut brut = Vec::new();
        std::io::Read::read_to_end(&mut entree, &mut brut)?;
        let Ok(detail) = lire_piece(&brut) else {
            continue;
        };
        if detail.contenu.description.is_some() {
            return Ok(detail);
        }
        if premiere == DetailAnnonce::default() {
            premiere = detail;
        }
    }
    Ok(premiere)
}

fn lire_piece(octets: &[u8]) -> Result<DetailAnnonce> {
    let texte = lj_sources::pdf::pdftotext_extract(octets)?;
    Ok(super::formulaire::detail(&sans_ornement(&texte)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_annonces::zones::codes;

    /// A refusal served as bare text names what the door answered, where poppler
    /// only reported its exit code.
    #[test]
    fn a_non_pdf_answer_names_what_the_door_served() {
        let refus = b"Erreur interne, contactez le service technique si l'incident se reproduit (E_USER_WARNING)";
        let err = lire_avis(refus).expect_err("a bare-text refusal is not a notice");
        let dit = err.to_string();
        assert!(dit.contains("non-PDF answer"), "{dit}");
        assert!(dit.contains("E_USER_WARNING"), "{dit}");
    }

    /// The AAPC is read from the listing key, the notice URL never being stored.
    #[test]
    fn l_avis_se_tire_de_la_cle_de_l_url_de_consultation() {
        let ligne = &parse_page(LIGNE)[0];
        let cle = CLE
            .captures(ligne.url_avis.as_deref().expect("URL d'avis posée"))
            .expect("clé dans l'URL d'avis")[1]
            .to_string();
        assert_eq!(
            url_aapc(&cle),
            "https://v2.marches-securises.fr/entreprise/\
             ?module=script|dl_la_s&cle_dce=1786039549lgl7s3pll44g"
        );
    }

    /// Real page furniture of the renderer, stamped between the two halves of
    /// an object the page break cut in two.
    #[test]
    fn l_ornement_de_page_ne_coupe_pas_la_valeur_qu_il_traverse() {
        let texte = "Description succinte du marché : SYSTÈME D'ACQUISITION DYNAMIQUE\n\
	            \npage 1/3\n\nmarches-securises.fr\n\nAvis de publicité 25-96735\n\n24/09/2025\n\n\
	            POUR DES ÉTUDES ET PRESTATIONS TECHNIQUES\nClassification CPV : 71313000";
        let d = super::super::formulaire::detail(&sans_ornement(texte));
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "SYSTÈME D'ACQUISITION DYNAMIQUE \
                 POUR DES ÉTUDES ET PRESTATIONS TECHNIQUES"
            )
        );
        assert_eq!(d.cpv.as_deref(), Some("71313000"));
    }

    /// Ligne de listing dans son balisage réel, réduite aux `<tr>` que lit
    /// `parse_page`. L'acheteur n'a pas de suffixe `(971)` : la seule parenthèse du
    /// bloc est celle du type de marché, et le libellé « Référence interne » la suit.
    const LIGNE: &str = r#"<table class="consultation en_cours entreprise">
        <tr class="tr_pa tr_icone">
            <td colspan="5">Conseil Départemental de la Guadeloupe            <!-- added 1.1.0 --><a href="" onclick="addToFavoris('1786039549lgl7s3pll44g','')"><div class="icone_favoris" title="Ajouter ce dossier à mes favoris"></div></a>
            </td>
        </tr>
        <tr class="tr_objet">
            <td colspan="5" class="objet">
                <div style="display:block;float: left;margin-top:5px; max-width:800px;">REGIE ADMINISTRATIVE DANS LE CADRE DES MANIFESTATIONS CULTURELLES DEPARTEMENTALES DE VALORISATION DE LA LANGUE ET DE LA CULTURE CREOLES - LE MOIS DU KREYOL ÉDITION 2026</div>
                <div class="icone_dume" title="Dume"></div>
                <div class="icone_criteres_environnementaux" title="Critères environnementaux"></div>
            </td>
        </tr>
        <tr class="tr_identifiant">
            <th>Référence</th>
            <th colspan="4">CG_971_A_20260806W2_1</th>
        </tr>
        <tr class="tr_type">
            <th>Type de marché / Type de prestation</th>
            <td colspan="2">Public (CT et organismes assimilés) / Services</td>
            <th>Référence interne</th>
            <td>2026_22</span></td>
        </tr>
        <tr class="tr_dept">
            <th>Zone(s) g&eacute;o. de la prestation</th>
            <td colspan="4">971 Guadeloupe</td>
        </tr>
        <tr class="tr_actions">
            <td colspan="5"><a href="?module=consultation|details&amp;cle_dce=1786039549lgl7s3pll44g&version=QR" class="lien_dce">Accéder au DCE</a></td>
        </tr>
    </table>"#;

    const OBJET_LIGNE: &str = "REGIE ADMINISTRATIVE DANS LE CADRE DES MANIFESTATIONS CULTURELLES DEPARTEMENTALES DE VALORISATION DE LA LANGUE ET DE LA CULTURE CREOLES - LE MOIS DU KREYOL ÉDITION 2026";

    /// Des deux références de la fiche, on retient celle de l'acheteur — c'est
    /// la seule que le BOAMP publie aussi, donc la seule qui joigne les canaux.
    #[test]
    fn la_reference_retenue_est_celle_de_l_acheteur_pas_la_cle_maison() {
        assert_eq!(parse_page(LIGNE)[0].reference.as_deref(), Some("2026_22"));
        // Sans « Référence interne », la clé maison reste le seul identifiant.
        let sans = LIGNE.replace("Référence interne", "Autre chose");
        assert_eq!(
            parse_page(&sans)[0].reference.as_deref(),
            Some("CG_971_A_20260806W2_1")
        );
    }

    #[test]
    fn le_suffixe_de_departement_quitte_le_libelle_d_acheteur() {
        let html = LIGNE.replace(
            "Conseil Départemental de la Guadeloupe",
            "Ville de Chiconi (976)",
        );
        let r = &parse_page(&html)[0];
        assert_eq!(r.acheteur.as_deref(), Some("Ville de Chiconi"));
        assert_eq!(codes(&r.zones), ["976"]);
        assert_eq!(r.objet, OBJET_LIGNE);
    }

    #[test]
    fn l_acheteur_sans_departement_garde_son_libelle_et_son_objet() {
        let r = &parse_page(LIGNE)[0];
        assert_eq!(
            r.acheteur.as_deref(),
            Some("Conseil Départemental de la Guadeloupe")
        );
        assert_eq!(codes(&r.zones), ["971"]);
        assert_eq!(r.objet, OBJET_LIGNE);
        // Zone multiple : la première fait foi.
        let multi = LIGNE.replace("971 Guadeloupe", "60 Oise;02 Aisne;59 Nord");
        assert_eq!(codes(&parse_page(&multi)[0].zones), ["60"]);
        // Sans ligne `tr_dept` du tout : pas de département inventé.
        let sans = LIGNE.replace("971 Guadeloupe", "");
        assert!(parse_page(&sans)[0].zones.is_empty());
    }

    #[test]
    fn sans_reference_interne_l_objet_reste_lu() {
        let html = r#"<table class="consultation en_cours entreprise">
        <tr class="tr_pa tr_icone">
            <td colspan="5">Département-Région de Mayotte            <!-- added 1.1.0 --><a href="" onclick="addToFavoris('1785401036n1m7r7hps1gp','')"><div class="icone_favoris" title="Ajouter ce dossier à mes favoris"></div></a>
            </td>
        </tr>
        <tr class="tr_objet">
            <td colspan="5" class="objet">
                <div style="display:block;float: left;margin-top:5px; max-width:800px;">Accord-cadre relatif à des prestations de maintenance des outillages de manutention de conteneurs de l'EPPCM</div>
                <div class="icone_vide"></div>
            </td>
        </tr>
        <tr class="tr_identifiant">
            <th>Référence</th>
            <th colspan="4">CG_976_20260730W2_01</th>
        </tr>
        <tr class="tr_type">
            <th>Type de marché / Type de prestation</th>
            <td colspan="4">Public (CT et organismes assimilés) / Services</td>
        </tr>
        <tr class="tr_actions">
            <td colspan="5"><a href="?module=consultation|details&amp;cle_dce=1785401036n1m7r7hps1gp&version=QR" class="lien_dce">Accéder au DCE</a></td>
        </tr>
    </table>"#;
        let r = &parse_page(html)[0];
        assert_eq!(r.acheteur.as_deref(), Some("Département-Région de Mayotte"));
        assert!(r.zones.is_empty());
        assert_eq!(
            r.objet,
            "Accord-cadre relatif à des prestations de maintenance des outillages de manutention de conteneurs de l'EPPCM"
        );
    }
}
