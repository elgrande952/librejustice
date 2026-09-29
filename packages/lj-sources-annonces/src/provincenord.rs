//! The procurement room of the Province Nord of New Caledonia, a WebForms
//! application matching no engine fingerprint. A territory ruling its own
//! procurement publishes at neither the BOAMP nor TED: nothing else reaches it.

use std::sync::LazyLock;

use jiff::civil::{Date, Time};
use regex::Regex;

use lj_annonces::zones::zones_departements;
use lj_annonces::{echeance_ts, parse_heure, AnnonceBrute, ContenuAnnonce, NatureMarche};
use lj_core::dates::parse_date_fr;

use super::{send_text, txt, HoteVerdict, MoteurRun};
use lj_sources::error::Result;

const BASE: &str = "https://marchespublics.province-nord.nc";
/// New Caledonia is a collectivity, not a département: `988` is its zone code.
const ZONE: &str = "988";
/// The grid serves ten rows a page and the room has held tens of consultations.
const PAGES_MAX: usize = 40;

fn salle() -> String {
    format!("{BASE}/sallemarche.aspx")
}

/// A WebForms postback replays every control of the page, the three tokens
/// included. Dropping the filter boxes makes the server answer a redirect
/// instead of the grid.
fn corps(page: &str, cible: &str) -> Vec<(String, String)> {
    let mut f: Vec<(String, String)> = vec![
        ("__EVENTTARGET", cible),
        ("__EVENTARGUMENT", ""),
        ("__VIEWSTATE", &champ_cache(page, "__VIEWSTATE")),
        (
            "__VIEWSTATEGENERATOR",
            &champ_cache(page, "__VIEWSTATEGENERATOR"),
        ),
        ("__EVENTVALIDATION", &champ_cache(page, "__EVENTVALIDATION")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (nom, valeur) in [
        ("txtFiltreNumConsultation", ""),
        ("txtFiltreIntituleConsultation", ""),
        ("cboFiltreOrganisme", "Tous"),
        ("cboFiltreService", "Tous"),
        ("cboFiltreType", "Tous"),
        ("cbb_mots_cles", "Tous"),
        ("cbb_procedure", "Toutes"),
        ("cboFiltreEtat", "Ouvert"),
    ] {
        f.push((
            format!("ctl00$ContentPlaceHolder_Mid${nom}"),
            valeur.to_string(),
        ));
    }
    f
}

fn champ_cache(page: &str, nom: &str) -> String {
    static CACHE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<input[^>]*\bid="([^"]+)"[^>]*\bvalue="([^"]*)""#).unwrap()
    });
    CACHE
        .captures_iter(page)
        .find(|c| &c[1] == nom)
        .map(|c| lj_sources::html_strip::strip_html(&c[2]))
        .unwrap_or_default()
}

/// The pager is the only place the grid states how far it goes.
fn pagination(page: &str) -> (usize, u64) {
    static PAGES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Page \d+ sur (\d+)").unwrap());
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Consultation \d+ \D{1,3} \d+ sur (\d+)").unwrap());
    let plat = txt(page);
    let pages = PAGES
        .captures(&plat)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(1);
    let total = TOTAL
        .captures(&plat)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(0);
    (pages, total)
}

/// The numbered page links name their own postback target, which the fixed
/// `ctlNN` numbering does not: it shifts with the page count.
fn cible_page(page: &str, n: usize) -> Option<String> {
    static LIEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"__doPostBack\(&#39;([^&]*?ctl03\$ctl01\$ctl\d+)&#39;,&#39;&#39;\)[^>]*>\s*<span>(\d+)</span>",
        )
        .unwrap()
    });
    LIEN.captures_iter(page)
        .find(|c| c[2].parse::<usize>() == Ok(n))
        .map(|c| c[1].to_string())
}

/// The grid keeps its data keys in the client-side init, in row order: reading
/// them there spares one postback per row to learn the notice id.
fn ids(page: &str) -> Vec<String> {
    static ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""ID_CONSULTATION":"(\d+)""#).unwrap());
    ID.captures_iter(page).map(|c| c[1].to_string()).collect()
}

fn nature(libelle: &str) -> Option<NatureMarche> {
    match libelle.trim().to_uppercase().as_str() {
        "TRAVAUX" => Some(NatureMarche::Travaux),
        "SERVICE" | "SERVICES" => Some(NatureMarche::Services),
        "FOURNITURE" | "FOURNITURES" => Some(NatureMarche::Fournitures),
        _ => None,
    }
}

/// Both date columns carry the hour, as `11/09/2026 à 15:00`.
fn horodate(cellule: &str) -> Option<(Date, Option<Time>)> {
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{2}/\d{2}/\d{4})").unwrap());
    static HEURE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,2}[h:]\d{2})").unwrap());
    let d = parse_date_fr(&DATE.captures(cellule)?[1])?;
    Some((d, HEURE.captures(cellule).and_then(|c| parse_heure(&c[1]))))
}

fn parse_rows(page: &str) -> Vec<AnnonceBrute> {
    static LIGNE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<tr[^>]*class="rg(?:Alt)?Row"[^>]*>(.*?)</tr>"#).unwrap()
    });
    static CELLULE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<td[^>]*>(.*?)</td>").unwrap());
    static PROCEDURE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<td title="([^"]*)""#).unwrap());

    let cles = ids(page);
    let mut out = Vec::new();
    for (rang, ligne) in LIGNE.captures_iter(page).enumerate() {
        let cellules: Vec<String> = CELLULE
            .captures_iter(&ligne[1])
            .map(|c| txt(&c[1]).trim().to_string())
            .collect();
        // Columns in order: expand button, buyer, service, reference, object,
        // kind, state, opening, closing. A shorter row is not a consultation.
        let [_, acheteur, _, reference, objet, forme, _, ouverture, cloture, ..] = &cellules[..]
        else {
            continue;
        };
        let Some(id) = cles.get(rang) else { continue };
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "provincenord",
            source_uid: format!("provincenord:{id}"),
            cles_jointure: Vec::new(),
            reference: Some(reference.clone()).filter(|r| !r.is_empty()),
            acheteur: Some(acheteur.clone()).filter(|a| !a.is_empty()),
            siren: None,
            objet: objet.clone(),
            nature: nature(forme),
            cpv: None,
            zones: zones_departements([ZONE]),
            date_publication: horodate(ouverture).map(|(d, _)| d),
            echeance: horodate(cloture).map(|(d, h)| echeance_ts(d, h)),
            url_avis: Some(format!("{BASE}/consultation.aspx?id_consultation={id}")),
            url_rc: None,
            url_dce: None,
            url_depot: None,
            procedure: PROCEDURE
                .captures(&ligne[1])
                .map(|c| lj_sources::html_strip::strip_html(&c[1])),
        });
    }
    out
}

/// Re-enumerates the open consultations of the room.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let host = BASE.trim_start_matches("https://").to_string();
    let premiere = match send_text(client.get(salle()), "provincenord salle").await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "provincenord: room unreachable");
            return Ok(MoteurRun {
                annonce: None,
                echecs: 0,
                hotes: vec![HoteVerdict {
                    host,
                    joignable: false,
                }],
                rows: Vec::new(),
            });
        }
    };
    let (pages, total) = pagination(&premiere);
    let mut rows = parse_rows(&premiere);
    let mut echecs = 0;
    // Every page is asked of the first one: the grid honours a postback built
    // on an already-spent `__VIEWSTATE`, which spares threading the last page.
    for n in 2..=pages.min(PAGES_MAX) {
        let Some(cible) = cible_page(&premiere, n) else {
            echecs += 1;
            continue;
        };
        match send_text(
            client
                .post(salle())
                .header("Referer", salle())
                .form(&corps(&premiere, &cible)),
            &format!("provincenord page {n}"),
        )
        .await
        {
            Ok(p) => rows.extend(parse_rows(&p)),
            Err(e) => {
                tracing::warn!(page = n, error = %e, "provincenord: page lost");
                echecs += 1;
            }
        }
    }
    Ok(MoteurRun {
        annonce: Some(total),
        echecs,
        hotes: vec![HoteVerdict {
            host,
            joignable: true,
        }],
        rows: super::dedup_source_uid(rows),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `rgRow` of `sallemarche.aspx`, with the client-side data keys the
    /// grid emits for it.
    const PAGE: &str = r#"
<script>var o = {"_clientKeyValues":{"0":{"ID_CONSULTATION":"3072"}}};</script>
<div class="rgWrap rgInfoPart"> Page 1 sur 2, Consultation 1 &agrave; 10 sur 11 </div>
<a href="javascript:__doPostBack(&#39;ctl00$ContentPlaceHolder_Mid$RadGrid_ListeConsultation$ctl00$ctl03$ctl01$ctl07&#39;,&#39;&#39;)"><span>2</span></a>
<tr class="rgRow">
<td class="rgExpandCol"><input type="button" /></td>
<td align="center">SIVM VKP</td>
<td align="center">SIVM VKP</td>
<td align="center">06-2026</td>
<td align="left">MARCHE DE COLLECTE 2027-2031 DES ORDURES MENAGERES</td>
<td align="center">Service</td>
<td align="center">Ouvert</td>
<td align="center">11/09/2026 &agrave; 15:00</td>
<td align="center">16/10/2026 &agrave; 12:00</td>
<td title="Appel d&#39;offres ouvert" align="center"><img src="img/x.png" /></td>
<td><input type="image" /></td>
</tr>"#;

    #[test]
    fn a_row_gives_the_buyer_the_object_the_reference_and_both_dates() {
        let rows = parse_rows(PAGE);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.source_uid, "provincenord:3072");
        assert_eq!(r.acheteur.as_deref(), Some("SIVM VKP"));
        assert_eq!(r.reference.as_deref(), Some("06-2026"));
        assert_eq!(r.nature, Some(NatureMarche::Services));
        assert_eq!(r.procedure.as_deref(), Some("Appel d'offres ouvert"));
        assert_eq!(
            r.url_avis.as_deref(),
            Some("https://marchespublics.province-nord.nc/consultation.aspx?id_consultation=3072")
        );
        assert!(r.date_publication.is_some() && r.echeance.is_some());
    }

    #[test]
    fn the_pager_gives_the_page_count_and_the_total() {
        assert_eq!(pagination(PAGE), (2, 11));
    }

    /// The fixed `ctlNN` numbering shifts with the page count, so the target is
    /// read from the link that carries the wanted number.
    #[test]
    fn the_page_target_is_read_from_its_link() {
        assert_eq!(
            cible_page(PAGE, 2).as_deref(),
            Some("ctl00$ContentPlaceHolder_Mid$RadGrid_ListeConsultation$ctl00$ctl03$ctl01$ctl07")
        );
        assert_eq!(cible_page(PAGE, 3), None);
    }
}
