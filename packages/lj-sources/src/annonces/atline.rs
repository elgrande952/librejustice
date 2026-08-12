//! Atline (`marches-securises.fr`).
//!
//! Un `POST ?module=consultation|script|selection_consultation_s` **arme** la
//! recherche en session et rend la première page ; la pagination se fait
//! ensuite en `GET ?module=liste_consultations&page=N`. Une ligne =
//! `<table class="consultation …">`, clé métier `cle_dce`, 10 par page.

use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_core::annonces::{
    echeance_ts, normalise_departement, parse_date_fr, parse_heure, AnnonceBrute,
};

use super::{apres_balise, dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::Result;

const BASE: &str = "https://www.marches-securises.fr";
const ENTR: &str = "https://www.marches-securises.fr/entreprise/";
const PER_PAGE: u64 = 10;
const SESSIONS: u64 = 12;

/// Arme la recherche : rend (client en session, première page, total annoncé).
async fn armer() -> Result<(reqwest::Client, String, u64)> {
    let client = super::client();
    send_text(client.get(format!("{BASE}/")), "atline accueil").await?;
    let html = send_text(
        client
            .post(format!(
                "{ENTR}?module=consultation|script|selection_consultation_s"
            ))
            .header("Referer", format!("{BASE}/"))
            .form(&[
                ("module", "consultation|script|selection_consultation_s"),
                ("objet", ""),
                ("mots_cles", ""),
                ("presta_tx", "travaux"),
                ("presta_se", "services"),
                ("presta_fo", "fournitures"),
                ("presta_au", "autres"),
                ("date_cloture_type", "0"),
                ("dep_liste", ""),
                ("type_marche", ""),
                ("type_procedure", ""),
                ("submit", "Rechercher"),
            ]),
        "atline recherche",
    )
    .await?;
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\d[\d\s]{0,7})\s*consultations?").unwrap());
    let total = TOTAL
        .captures(&txt(&html))
        .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
        .unwrap_or(0);
    Ok((client, html, total))
}

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    static CLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"cle_dce=([A-Za-z0-9_\-]+)").unwrap());
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
            .and_then(|c| lj_core::annonces::NatureMarche::parse(&c[1]));
        let pa = ACHETEUR
            .captures(&utile)
            .map(|c| txt(&c[1]))
            .unwrap_or_default();
        let departement = DEPT
            .captures(&pa)
            .and_then(|c| normalise_departement(&c[1]))
            .or_else(|| {
                ZONE.captures(&utile)
                    .and_then(|c| normalise_departement(&c[1]))
            });
        out.push(AnnonceBrute {
            moteur: "atline",
            source_uid: format!("atline:{cle}"),
            cles_jointure: Vec::new(),
            reference: REF.captures(&flat).map(|c| c[1].to_string()),
            acheteur: Some(DEPT.replace(&pa, "").trim().to_string()).filter(|a| !a.is_empty()),
            siren: None,
            objet: OBJET
                .captures(&utile)
                .map(|c| txt(&c[1]).chars().take(400).collect::<String>())
                .unwrap_or_default(),
            nature,
            cpv: None,
            departement,
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
        rows: parse_page(&premiere),
    };
    for res in resultats {
        match res {
            Ok(rows) => run.rows.extend(rows),
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(error = %e, "atline : tranche en échec, stock conservé");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

/// Lit les deux champs aux libellés du moteur — `Code CPV` et la ligne
/// `Procédure` du gabarit de consultation. La valeur est exigée : le libellé
/// seul (celui d'un champ de recherche) n'en est pas une.
fn parse_detail(html: &str) -> DetailAnnonce {
    static CPV: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)CPV\s*:?\s*(\d{8}(?:-\d)?)").unwrap());
    static PROC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Procédure\s+([^:]{3,80}?)\s*(?:Zone\(s\) géo|Date de|Télécharger|$)").unwrap()
    });
    let flat = txt(html);
    DetailAnnonce {
        cpv: CPV.captures(&flat).map(|c| c[1].to_string()),
        procedure: PROC.captures(&flat).map(|c| c[1].trim().to_string()),
        acheteur: None,
    }
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing.
///
/// Atline ne rend aucune fiche de consultation : `consultation|details` — comme
/// `consultation|qr` — sert la page d'accès anonyme au DCE, la même pour toutes
/// les consultations et vide de tout champ d'avis. Le CPV d'une consultation ne
/// vit que dans les PDF de `PiecesAAPC.zip`, et sa procédure est déjà lue en
/// listing par [`parse_page`]. La page est donc lue puis rendue muette, et c'est
/// l'aller-retour qui porte l'information : moteur injoignable = erreur et la
/// ligne repasse la nuit suivante, page servie = la ligne sort de la file.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let html = send_text(super::client().get(url_avis), "atline détail").await?;
    Ok(parse_detail(&html))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_page_d_acces_au_dce_ne_porte_ni_cpv_ni_procedure() {
        let html = r#"<ul id="espace_ok">
            <li class="stitre">Je poss&egrave;de d&eacute;j&agrave; un espace priv&eacute;</li>
            <li><form method="post" action="?module=pre|login_s"><ul class="actions">
                <li>Identifiant</li><li><input type="text" name="log" /></li>
                <li>Mot de passe</li><li><input type="password" name="pass" /></li>
            </ul></form></li></ul>
            <div id="bloc_infos"><div class="titre">Arr&ecirc;t&eacute; du 22 mars 2019</div>
            <p>Conform&eacute;ment &agrave; l'arr&ecirc;t&eacute; du 22 mars 2019, vous disposez
            d'un acc&egrave;s direct au t&eacute;l&eacute;chargement du dossier complet sans vous
            identifier. Pour t&eacute;l&eacute;charger anonymement le dossier complet au format
            compress&eacute; Zip, <a href="?module=script|dl_dce_s">cliquez sur ce lien</a>.</p>
            </div>"#;
        assert_eq!(parse_detail(html), DetailAnnonce::default());
    }

    #[test]
    fn le_libelle_seul_ne_fait_pas_une_valeur() {
        let html = r#"<tr class="tr_proc">
                <th>Proc&eacute;dure</th>
                <td colspan="4">Proc&eacute;dure Adapt&eacute;e Ouverte &gt; 90 k&euro;</td>
            </tr>
            <tr class="tr_dept">
                <th>Zone(s) g&eacute;o. de la prestation</th><td colspan="4">75 Paris</td>
            </tr>
            <tr><td style="width:110px">Code CPV :</td>
                <td><input type="text" name="cpv_et" /></td></tr>"#;
        assert_eq!(
            parse_detail(html),
            DetailAnnonce {
                cpv: None,
                procedure: Some("Procédure Adaptée Ouverte > 90 k€".into()),
                acheteur: None,
            }
        );
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

    #[test]
    fn le_suffixe_de_departement_quitte_le_libelle_d_acheteur() {
        let html = LIGNE.replace(
            "Conseil Départemental de la Guadeloupe",
            "Ville de Chiconi (976)",
        );
        let r = &parse_page(&html)[0];
        assert_eq!(r.acheteur.as_deref(), Some("Ville de Chiconi"));
        assert_eq!(r.departement.as_deref(), Some("976"));
        assert_eq!(r.objet, OBJET_LIGNE);
    }

    #[test]
    fn l_acheteur_sans_departement_garde_son_libelle_et_son_objet() {
        let r = &parse_page(LIGNE)[0];
        assert_eq!(
            r.acheteur.as_deref(),
            Some("Conseil Départemental de la Guadeloupe")
        );
        assert_eq!(r.departement.as_deref(), Some("971"));
        assert_eq!(r.objet, OBJET_LIGNE);
        // Zone multiple : la première fait foi.
        let multi = LIGNE.replace("971 Guadeloupe", "60 Oise;02 Aisne;59 Nord");
        assert_eq!(parse_page(&multi)[0].departement.as_deref(), Some("60"));
        // Sans ligne `tr_dept` du tout : pas de département inventé.
        let sans = LIGNE.replace("971 Guadeloupe", "");
        assert_eq!(parse_page(&sans)[0].departement, None);
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
        assert_eq!(r.departement, None);
        assert_eq!(
            r.objet,
            "Accord-cadre relatif à des prestations de maintenance des outillages de manutention de conteneurs de l'EPPCM"
        );
    }
}
