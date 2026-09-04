use std::sync::LazyLock;

use regex::Regex;

use lj_core::annonces::{echeance_ts, parse_date_fr, parse_heure, AnnonceBrute};

use super::{apres_balise, dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::{Result, SourceError};

const BASE: &str = "https://eu.eu-supply.com/ctm/supplier/publictenders";
const BRANDING: &str = "SYNAPSE";
const MAX_PAGES: u64 = 60;

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    static LIEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)href="(/app/rfq/rwlentrance_s\.asp\?PID=(\d+)[^"]*)""#).unwrap()
    });
    static CELLULE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<td[^>]*>(.*?)</td>").unwrap());
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{2}/\d{2}/\d{4})(?:\s+(\d{2}:\d{2}))?").unwrap());

    let mut out = Vec::new();
    for bloc in html.split("<tr").skip(1).map(apres_balise) {
        let Some(lien) = LIEN.captures(bloc) else {
            continue;
        };
        let (href, pid) = (lien[1].replace("&amp;", "&"), lien[2].to_string());
        // Colonnes du listing CTM : identifiant, référence de l'acheteur,
        // objet, publication, limite, procédure, acheteur, pays.
        let cellules: Vec<String> = CELLULE
            .captures_iter(bloc)
            .map(|c| txt(&c[1]).trim().to_string())
            .collect();
        let cellule = |i: usize| cellules.get(i).filter(|c| !c.is_empty()).cloned();
        let jour = |i: usize| parse_date_fr(&DATE.captures(&cellule(i)?)?[1]);
        let horodate = |i: usize| {
            let texte = cellule(i)?;
            let c = DATE.captures(&texte)?;
            let heure = c.get(2).and_then(|h| parse_heure(h.as_str()));
            Some(echeance_ts(parse_date_fr(&c[1])?, heure))
        };
        out.push(AnnonceBrute {
            moteur: "synapse",
            source_uid: format!("synapse:{pid}"),
            cles_jointure: Vec::new(),
            reference: cellule(1),
            acheteur: cellule(6),
            siren: None,
            objet: cellule(2).unwrap_or_default().chars().take(300).collect(),
            nature: None,
            cpv: None,
            departement: None,
            date_publication: jour(3),
            echeance: horodate(4),
            url_avis: Some(format!("https://eu.eu-supply.com{href}")),
            url_rc: None,
            url_dce: Some(format!("https://eu.eu-supply.com{href}")),
            url_depot: None,
            procedure: cellule(5),
        });
    }
    out
}

/// Le branding d'un hôte de façade : son premier label, en capitales. Ni les
/// hôtes de l'éditeur ni ceux d'EU-Supply nu n'en portent — [`BRANDING`] les
/// couvre.
fn branding(hote: &str) -> Option<String> {
    if hote.ends_with("eu-supply.com") {
        return None;
    }
    let label = hote.trim_start_matches("www.").split('.').next()?;
    (!label.starts_with("synapse")).then(|| label.to_uppercase())
}

/// Ré-énumère la liste publique de l'éditeur et celle de chaque hôte recensé.
/// Pas de total déclaré exploitable. Un branding en échec vaut un branding
/// vide, pas l'échec de la moisson.
pub async fn enumerate(hotes: &[String]) -> Result<MoteurRun> {
    let client = super::client();
    let mut brandings: Vec<String> = hotes.iter().filter_map(|h| branding(h)).collect();
    brandings.sort();
    brandings.dedup();
    brandings.insert(0, BRANDING.to_string());

    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut echecs = 0;
    for b in &brandings {
        let mut vus = std::collections::HashSet::new();
        for p in 1..=MAX_PAGES {
            let html = match send_text(
                client.get(BASE).query(&[
                    ("B", b.clone()),
                    ("SearchFilter.PagingInfo.PageNumber", p.to_string()),
                ]),
                &format!("synapse {b} page {p}"),
            )
            .await
            {
                Ok(html) => html,
                Err(e) => {
                    echecs += 1;
                    tracing::warn!(branding = %b, page = p, error = %e, "synapse : page en échec, stock conservé");
                    break;
                }
            };
            let avant = vus.len();
            for r in parse_page(&html) {
                if vus.insert(r.source_uid.clone()) {
                    rows.push(r);
                }
            }
            if vus.len() == avant {
                break;
            }
        }
    }
    Ok(MoteurRun {
        annonce: None,
        echecs,
        hotes: Vec::new(),
        rows: dedup_source_uid(rows),
    })
}

/// Lit la fiche CTM. Le gabarit ne rend des codes CPV que quand l'acheteur en a
/// saisi : un libellé `ctm-content-label` suivi d'un `<p>` d'un `<span>` par
/// code, le principal en tête. Chaque fiche est servie dans la langue de son
/// acheteur — « Codes CPV » chez l'un, « CPV codes » chez l'autre au sein d'une
/// même session —, d'où l'ancrage sur le seul acronyme. La procédure n'est pas
/// de la fiche : le moteur la garde en colonne de listing (« Marché privé »,
/// « Appel d'offres ouvert », « Procédure Adaptée. »).
///
/// Sans `tenderInfoSection`, la page n'est pas une fiche : un PID retiré du
/// public rend un 200 « Access denied », qui est un échec de lecture et non un
/// avis muet.
fn parse_detail(html: &str) -> Result<DetailAnnonce> {
    static BLOC_CPV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)class="ctm-content-label"[^>]*>[^<]*CPV[^<]*</div>\s*<p>(.*?)</p>"#)
            .unwrap()
    });
    static CODE_CPV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{8}(?:-\d)?\b").unwrap());

    if !html.contains("tenderInfoSection") {
        return Err(SourceError::Invalid(
            "synapse détail: fiche non servie".into(),
        ));
    }
    Ok(DetailAnnonce {
        cpv: BLOC_CPV
            .captures(html)
            .and_then(|c| CODE_CPV.find(&txt(&c[1])).map(|m| m.as_str().to_string())),
        procedure: None,
        acheteur: None,
    })
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing.
///
/// `rwlentrance_s.asp` rebondit en 302 vers `/ctm/Supplier/PublicPurchase/<PID>`
/// et la chaîne ne se referme qu'une fois posé le cookie d'affinité du
/// répartiteur — sans jar elle boucle jusqu'à épuisement du compteur de
/// redirections, celui de [`super::client`] suffit.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let html = send_text(super::client().get(url_avis), "synapse détail").await?;
    parse_detail(&html)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le bloc CPV d'une fiche renseignée, dans les deux langues où le moteur
    /// le sert : le code retenu est le premier listé, complet (clé de contrôle
    /// comprise), et la fiche ne dit rien de la procédure.
    #[test]
    fn le_premier_code_cpv_est_lu_quelle_que_soit_la_langue_de_la_fiche() {
        let fr = r#"<div id="tenderInfoSection" class="ctm-section-box">
            <div class="ctm-content-label">Type de contrat:</div>
            <p>Fournitures</p>
            <div class="ctm-content-label">Date limite de réponse :</div>
            <p>30/09/2026 11:00:00</p>
            <div class="ctm-content-label">Codes CPV:</div>
            <p>
                    <span class="">34144400-2&nbsp;&nbsp;Véhicules d'entretien routier</span><br />
                    <span class="">39713410-0&nbsp;&nbsp;Machines de nettoyage des sols</span><br />
                    <span class="">34144000-8&nbsp;&nbsp;Véhicules automobiles à usage spécifique</span><br />
            </p>"#;
        assert_eq!(
            parse_detail(fr).unwrap(),
            DetailAnnonce {
                cpv: Some("34144400-2".into()),
                procedure: None,
                acheteur: None,
            }
        );

        let en = r#"<div id="tenderInfoSection" class="ctm-section-box">
            <div class="ctm-content-label">CPV codes:</div>
            <p>
                    <span class="">34144431-8&nbsp;&nbsp;Suction-sweeper vehicles</span><br />
                    <span class="">34144430-1&nbsp;&nbsp;Road-sweeping vehicles</span><br />
                    <span class="">34921100-0&nbsp;&nbsp;Road sweepers</span><br />
            </p>"#;
        assert_eq!(parse_detail(en).unwrap().cpv.as_deref(), Some("34144431-8"));
    }

    /// La fiche que servent les acheteurs qui n'ont rempli aucun CPV — le cas
    /// courant du branding : lue, muette, la ligne sort de la file.
    #[test]
    fn la_fiche_sans_bloc_cpv_est_muette() {
        let html = r#"<div id="tenderInfoSection" class="ctm-section-box">
            <div class="ctm-content-label">Description détaillée:</div>
            <p class="ctm-multi-line"></p>
            <div class="ctm-content-label">Date limite de réponse :</div>
            <p>02/10/2026 12:00:00</p>
            <div class="ctm-content-label">Donneur d'ordre:</div>
            <p><br />Cisn coop&#233;rative<br />44613<br />SAINT-NAZAIRE CEDEX<br />France</p>"#;
        assert_eq!(parse_detail(html).unwrap(), DetailAnnonce::default());
    }

    /// Un PID retiré du public rend un 200 sans fiche : c'est une lecture en
    /// échec, la ligne reste en file — pas un avis sans CPV.
    #[test]
    fn l_acces_refuse_n_est_pas_une_fiche_muette() {
        let html = r#"<div class="ctm-header-pane"><h1></h1></div>
            <div class="container-default">
                <div class="header"><h2>Information</h2></div>
                <div class="content"><div class="error-img">
                    <i class="icon-lock icon-2x yellow-icon"></i>
                    <div class="error-content"><h3 class="error">Access denied</h3><br />
                    You&nbsp;are&nbsp;not&nbsp;authorized&nbsp;to&nbsp;view the requested&nbsp;page!
                </div></div></div>
            </div>"#;
        assert!(parse_detail(html).is_err());
    }

    /// Le listing CTM tient ses champs en colonnes de position, chacune dans un
    /// `<div>` à infobulle : la référence de l'acheteur est la deuxième, le nom
    /// de l'acheteur la septième. Les lire au rang près est la seule façon de
    /// les distinguer — sur une ligne mise à plat, `2/2026` et `Amneville`
    /// n'ont aucune marque qui les sépare du reste.
    #[test]
    fn la_ligne_de_listing_rend_la_reference_et_l_acheteur_a_leur_rang() {
        let ligne = r#"<tr>
            <td><div class="js-tooltip-content" title="200418950">200418950</div></td>
            <td><div class="js-tooltip-content" title="2/2026">2/2026</div></td>
            <td><a href="/app/rfq/rwlentrance_s.asp?PID=458418&amp;PP=ctm/Supplier/publictenders&amp;B=AMNEVILLE&amp;PS=1" target="_blank">PROTECTION SOCIALE COMPLEMENTAIRE</a></td>
            <td><div class="js-tooltip-content" title="02/07/2026 15:32:00">02/07/2026</div></td>
            <td><div class="js-tooltip-content" title="10/09/2026 12:00:00">10/09/2026 12:00</div></td>
            <td><div class="js-tooltip-content" title="Appel d&#39;offres ouvert">Appel d&#39;offres ouvert</div></td>
            <td><div class="js-tooltip-content" title="Amneville">Amneville</div></td>
            <td><div class="js-tooltip-content" title="France">France</div></td>
        </tr>"#;

        let [row] = parse_page(ligne).try_into().expect("une ligne");
        assert_eq!(row.source_uid, "synapse:458418");
        assert_eq!(row.reference.as_deref(), Some("2/2026"));
        assert_eq!(row.acheteur.as_deref(), Some("Amneville"));
        assert_eq!(row.objet, "PROTECTION SOCIALE COMPLEMENTAIRE");
        assert_eq!(row.procedure.as_deref(), Some("Appel d'offres ouvert"));
        assert_eq!(
            row.date_publication.map(|d| d.to_string()).as_deref(),
            Some("2026-07-02")
        );
        assert!(row
            .echeance
            .is_some_and(|e| e.to_string().contains("2026-09-10")));
    }

    /// Le branding est le premier label de l'hôte de façade ; les hôtes de
    /// l'éditeur n'en portent pas, `B=SYNAPSE` les couvre déjà.
    #[test]
    fn le_branding_vient_du_premier_label_de_l_hote() {
        assert_eq!(
            branding("amneville.synapse-entreprises.com").as_deref(),
            Some("AMNEVILLE")
        );
        assert_eq!(
            branding("cisn.synapse-consult.com").as_deref(),
            Some("CISN")
        );
        assert_eq!(
            branding("www.duranceluberon.fr").as_deref(),
            Some("DURANCELUBERON")
        );
        assert_eq!(branding("synapse-entreprises.com"), None);
        assert_eq!(branding("synapse-ouest.com"), None);
        assert_eq!(branding("eu.eu-supply.com"), None);
    }
}
