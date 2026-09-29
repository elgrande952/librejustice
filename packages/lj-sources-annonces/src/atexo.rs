use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;
use serde::Deserialize;

use lj_annonces::zones::{zones_departements, Zone};
use lj_annonces::{parse_ts_iso, AnnonceBrute, ContenuAnnonce};
use lj_core::dates::parse_date_fr;

use super::{dedup_source_uid, fiche, send_text, txt, DetailAnnonce, MoteurRun};
use lj_sources::error::{Result, SourceError};

const PER_PAGE: u64 = 250;
/// Pages parallèles par hôte.
const PAGES_PARALLELES: usize = 6;
/// Hôtes énumérés de front.
const HOTES_PARALLELES: usize = 4;

#[derive(Deserialize)]
struct Hydra {
    #[serde(rename = "hydra:member")]
    member: Vec<serde_json::Value>,
    #[serde(rename = "hydra:totalItems", default)]
    total_items: u64,
}

async fn page(client: &wreq::Client, base: &str, n: u64, par_page: u64) -> Result<Hydra> {
    let body = send_text(
        client
            .get(format!("{base}/api/v2/consultations"))
            .header("Accept", "application/ld+json")
            .query(&[("itemsPerPage", par_page), ("page", n)]),
        &format!("atexo {base} page {n}"),
    )
    .await?;
    serde_json::from_str(&body)
        .map_err(|e| SourceError::Invalid(format!("atexo {base}: réponse non-Hydra: {e}")))
}

/// Énumère une instance : sonde à un élément pour le total, puis pages pleines
/// en parallèle, avec repli élément par élément sur les instances empoisonnées.
async fn enumerate_host(client: &wreq::Client, host: &str) -> super::HoteRun {
    // L'hôte nu peut ne pas répondre (PLACE exige `www.`) : on retente en
    // préfixé avant de déclarer l'hôte en échec.
    let mut base = format!("https://{host}");
    let sonde = match page(client, &base, 1, 1).await {
        Ok(p) => p,
        Err(_) if !host.starts_with("www.") => {
            base = format!("https://www.{host}");
            page(client, &base, 1, 1).await?
        }
        Err(e) => return Err(e),
    };
    // Une instance peut porter des consultations que l'API n'arrive pas à
    // sérialiser : leur page rend 500. À 250 par page, chaque page en contient une
    // et l'instance entière est perdue — on l'énumère alors élément par élément.
    let (par_page, premiere) = match page(client, &base, 1, PER_PAGE).await {
        Ok(p) => (PER_PAGE, p),
        Err(_) => (1, sonde),
    };
    let canon = host.trim_start_matches("www.");
    let total = premiere.total_items;
    let mut rows: Vec<AnnonceBrute> = premiere.member.iter().map(|m| row(m, canon)).collect();
    let npages = total.div_ceil(par_page);
    let mut pages = futures::stream::iter((2..=npages).map(|n| page(client, &base, n, par_page)))
        .buffer_unordered(PAGES_PARALLELES);
    let mut perdues = 0;
    while let Some(p) = pages.next().await {
        match p {
            Ok(p) => rows.extend(p.member.iter().map(|m| row(m, canon))),
            Err(_) if par_page == 1 => perdues += 1,
            Err(e) => return Err(e),
        }
    }
    if perdues > 0 {
        tracing::warn!(
            host,
            perdues,
            total,
            "atexo: consultations not serialisable"
        );
    }
    Ok((total, rows))
}

/// The consultations an instance serves for a market reference.
/// `referenceUtilisateur` filters on the reference the buyer typed, exactly: a
/// single answer designates, zero proves absence. Reading the search page
/// instead served, on PLACE, its sidebar and nothing else.
pub async fn chercher(host: &str, reference: &str) -> Result<Vec<AnnonceBrute>> {
    let client = super::client();
    // L'hôte nu peut ne pas répondre (PLACE exige `www.`), comme à l'énumération.
    let requete = |base: &str| {
        client
            .get(format!("{base}/api/v2/consultations"))
            .header("Accept", "application/ld+json")
            .query(&[("referenceUtilisateur", reference), ("itemsPerPage", "2")])
    };
    let base = format!("https://{host}");
    let body = match send_text(requete(&base), &format!("atexo recherche {host}")).await {
        Ok(b) => b,
        Err(_) if !host.starts_with("www.") => {
            send_text(
                requete(&format!("https://www.{host}")),
                &format!("atexo recherche www.{host}"),
            )
            .await?
        }
        Err(e) => return Err(e),
    };
    let hydra: Hydra = serde_json::from_str(&body)
        .map_err(|e| SourceError::Invalid(format!("atexo {host}: réponse non-Hydra: {e}")))?;
    let canon = host.trim_start_matches("www.");
    Ok(if hydra.total_items == 1 {
        hydra.member.iter().map(|m| row(m, canon)).collect()
    } else {
        Vec::new()
    })
}

/// Ré-énumère toutes les instances recensées (`profil_acheteur`).
pub async fn enumerate(hosts: &[String]) -> Result<MoteurRun> {
    let client = super::client();
    let resultats: Vec<(String, super::HoteRun)> = futures::stream::iter(hosts.iter().map(|h| {
        let client = &client;
        async move { (h.clone(), enumerate_host(client, h).await) }
    }))
    .buffer_unordered(HOTES_PARALLELES)
    .collect()
    .await;
    let mut run = MoteurRun {
        annonce: Some(0),
        echecs: 0,
        hotes: Vec::new(),
        rows: Vec::new(),
    };
    for (host, res) in resultats {
        let joignable = res.is_ok();
        match res {
            Ok((total, rows)) => {
                run.annonce = run.annonce.map(|a| a + total);
                run.rows.extend(rows);
            }
            Err(e) => tracing::warn!(host, error = %e, "atexo: host failed"),
        }
        run.hotes.push(super::HoteVerdict { host, joignable });
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

fn row(m: &serde_json::Value, host: &str) -> AnnonceBrute {
    let texte = |k: &str| {
        m.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let id = m
        .get("id")
        .and_then(|v| v.as_i64())
        .map(|i| i.to_string())
        .or_else(|| texte("reference"))
        .unwrap_or_default();
    // `organismeDenomination` is the flat label, and the only key most instances
    // fill; the three others are objects or, on PLACE, a referential URI behind
    // JWT, which leaves its own consultations buyer-less.
    let acheteur = [
        "organismeDenomination",
        "entiteePublique",
        "organisme",
        "acheteur",
    ]
    .iter()
    .find_map(|k| {
        let v = m.get(*k)?;
        v.get("denomination")
            .and_then(|d| d.as_str())
            .or_else(|| {
                v.as_str()
                    .filter(|s| !s.starts_with('/') && !s.starts_with("/api"))
            })
            .map(txt)
            .filter(|s| !s.is_empty())
    });
    let lieux: Vec<String> = m
        .get("lieuxExecution")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|l| {
            l.as_str().map(str::to_string).or_else(|| {
                l.get("libelle")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            })
        })
        .collect();
    let mut zones = zones_departements(&lieux);
    // `FR` is the place of performance of a national framework agreement, which
    // has no département and must not drop out of a departmental filter.
    if lieux.iter().any(|l| l.trim().eq_ignore_ascii_case("FR")) {
        zones.push(Zone::FranceEntiere);
    }
    let url_avis = texte("urlConsultation")
        .unwrap_or_else(|| format!("https://www.{host}/entreprise/consultation/{id}"));
    let url_dce = retrait_dce(&url_avis, &id);
    AnnonceBrute {
        contenu: ContenuAnnonce {
            description: texte("objet"),
            ..ContenuAnnonce::default()
        },
        descripteurs: Vec::new(),
        moteur: "atexo",
        source_uid: format!("atexo:{host}:{id}"),
        cles_jointure: Vec::new(),
        reference: texte("reference"),
        acheteur,
        siren: None,
        objet: texte("intitule")
            .or_else(|| texte("objet"))
            .unwrap_or_default(),
        nature: None,
        cpv: texte("codeCpvPrincipal"),
        zones,
        date_publication: texte("dateMiseEnLigneCalcule").and_then(|s| parse_date_fr(&s)),
        echeance: texte("dateLimiteRemiseOffres")
            .or_else(|| texte("datefinAffichage"))
            .and_then(|s| parse_ts_iso(&s)),
        url_avis: Some(url_avis),
        url_rc: None,
        url_dce,
        url_depot: None,
        procedure: None,
    }
}

/// Le formulaire de retrait du DCE, que MPE sert sans compte et qui propose le
/// téléchargement anonyme. Sans `orgAcronyme` l'entité n'est pas résolue et la
/// page ne rend rien : pas de lien plutôt qu'un lien mort.
fn retrait_dce(url_avis: &str, id: &str) -> Option<String> {
    static ACRONYME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"orgAcronyme=([A-Za-z0-9]+)").unwrap());
    let hote = super::discover::hote_de(url_avis)?;
    let acronyme = ACRONYME.captures(url_avis)?.get(1)?.as_str();
    Some(format!(
        "https://{hote}/index.php?page=Entreprise.EntrepriseDemandeTelechargementDce&id={id}&orgAcronyme={acronyme}"
    ))
}

/// `Intitulé` is the title the listing carries; `Objet` describes the market.
const DESCRIPTIONS: [&str; 1] = ["objet"];

/// The MPE notice, one template across every white label: one `<li>` a field,
/// `<label>Libellé :</label><div>valeur</div>`. An absence is normal and yields
/// an empty detail: the procedure `<div>` is empty on most notices, the
/// `Code CPV` block disappears where the instance leaves `AffichageCodeCpv`
/// off, and PLACE serves a SPA that carries none of these fields.
fn parse_detail(html: &str) -> DetailAnnonce {
    // Le principal est le seul code suffixé « (Code principal) » ; les secondaires
    // suivent dans le même bloc, sans suffixe. L'attribut porte le code tel que
    // l'instance le stocke, clé de radical comme complet.
    static CPV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)data-code-cpv="([^"]+)"[^>]*>[^<]*\(Code principal\)"#)
            .expect("regex CPV principal valide")
    });
    // Ancrer sur le `</label>` : « Recherche d'une procédure restreinte » du
    // menu précède le champ sur chaque page.
    static PROCEDURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)Procédure\s*:\s*</label>\s*<div[^>]*>(.*?)</div>")
            .expect("regex procédure valide")
    });
    static ENTITE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)Entité\s+publique\s*:\s*</label>\s*<div[^>]*>(.*?)</div>")
            .expect("regex entité publique valide")
    });
    let champs = fiche::champs(html);
    DetailAnnonce {
        contenu: fiche::contenu(&champs, fiche::premier(&champs, &DESCRIPTIONS)),
        zones: Vec::new(),
        cpv: CPV.captures(html).map(|c| c[1].trim().to_string()),
        procedure: PROCEDURE
            .captures(html)
            .map(|c| txt(&c[1]))
            .filter(|s| !s.is_empty()),
        acheteur: ENTITE
            .captures(html)
            .map(|c| txt(&c[1]))
            .filter(|s| !s.is_empty()),
    }
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let html = send_text(
        super::client().get(url_avis),
        &format!("atexo détail {url_avis}"),
    )
    .await?;
    Ok(parse_detail(&html))
}

#[cfg(test)]
mod tests {
    use super::{parse_detail, retrait_dce, row, ContenuAnnonce, DetailAnnonce};

    fn zones_de(lieux: serde_json::Value) -> Vec<String> {
        let m = serde_json::json!({ "id": 1, "lieuxExecution": lieux });
        lj_annonces::zones::codes(&row(&m, "exemple.fr").zones)
    }

    #[test]
    fn les_lieux_d_execution_couvrent_l_outre_mer_et_le_national() {
        assert_eq!(zones_de(serde_json::json!(["75", "2A"])), ["2A", "75"]);
        assert_eq!(zones_de(serde_json::json!(["NC.SU"])), ["988"]);
        assert_eq!(zones_de(serde_json::json!(["PF"])), ["987"]);
        assert_eq!(zones_de(serde_json::json!(["FR"])), [lj_vocab::ZONE_FRANCE]);
        assert!(zones_de(serde_json::json!([])).is_empty());
    }

    #[test]
    fn the_title_is_the_intitule_and_the_objet_describes_the_market() {
        let r = row(
            &serde_json::json!({
                "id": 1,
                "intitule": "SAD bornes interactives d'accueil",
                "objet": "La présente consultation a pour objet la mise en place d'un SAD",
            }),
            "exemple.fr",
        );
        assert_eq!(r.objet, "SAD bornes interactives d'accueil");
        assert_eq!(
            r.contenu.description.as_deref(),
            Some("La présente consultation a pour objet la mise en place d'un SAD")
        );
        let sans_intitule = row(
            &serde_json::json!({"id": 2, "objet": "Voirie"}),
            "exemple.fr",
        );
        assert_eq!(sans_intitule.objet, "Voirie");
    }

    #[test]
    fn l_acheteur_se_lit_sur_le_libelle_plat_avant_l_uri_de_referentiel() {
        let acheteur = |m: serde_json::Value| row(&m, "exemple.fr").acheteur;
        assert_eq!(
            acheteur(serde_json::json!({
                "id": 1,
                "organismeDenomination": "Service Départemental d&#039;Incendie",
                "organisme": "/api/v2/referentiels/organismes/ad_228",
            })),
            Some("Service Départemental d'Incendie".to_string())
        );
        assert_eq!(
            acheteur(serde_json::json!({
                "id": 1,
                "organismeDenomination": "",
                "entiteePublique": { "denomination": "Ville de Paris" },
            })),
            Some("Ville de Paris".to_string())
        );
        assert_eq!(
            acheteur(serde_json::json!({
                "id": 1,
                "organisme": "/api/v2/referentiels/organismes/g7h",
            })),
            None
        );
    }

    const FICHE_RECIA: &str = r#"
                                   href="/?page=Entreprise.EntrepriseAdvancedSearch&amp;searchAnnCons&type=restreinte">
                                    <span class="fa fa-chevron-right"></span>
                                    Recherche d&#039;une procédure restreinte
                                </a>
                            <li class="clearfix">
                                <label class="col-md-4 col-xs-12 spacing-helper-m">
                                    Procédure :
                                </label>
                                <div class="col-md-8 col-xs-12">
                                                                        Procédure adaptée &gt;  90 k EUR HT
                                                                    </div>
                            </li>
                                    <li class="clearfix">
                                    <label class="col-md-4 col-xs-12 spacing-helper-m">
                                        Code CPV :
                                    </label>
                                    <div class="col-md-8 col-xs-12">
                                        <span data-code-cpv="45233140">45233140 (Code principal)</span>
                                                                                    <br/>
                                                                                        <span data-code-cpv=""></span>
                                                                                </div>
                                </li>
"#;

    const FICHE_MAXIMILIEN: &str = r#"
                            <li class="clearfix">
                                <label class="col-md-4 col-xs-12 spacing-helper-m">
                                    Procédure :
                                </label>
                                <div class="col-md-8 col-xs-12">

                                                                    </div>
                            </li>
                                    <li class="clearfix">
                                    <label class="col-md-4 col-xs-12 spacing-helper-m">
                                        Code CPV :
                                    </label>
                                    <div class="col-md-8 col-xs-12">
                                        <span data-code-cpv="33168000">33168000 (Code principal)</span>
                                                                                    <br/>
                                                                                        <span data-code-cpv="33162000">33162000</span>
                                                                <span data-code-cpv="33100000">33100000</span>
                                                                <span data-code-cpv="50421000">50421000</span>
                                                                                </div>
                                </li>
"#;

    #[test]
    fn la_fiche_rend_le_cpv_principal_et_la_procedure() {
        assert_eq!(
            parse_detail(FICHE_RECIA),
            DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: Some("45233140".into()),
                procedure: Some("Procédure adaptée > 90 k EUR HT".into()),
                acheteur: None,
            }
        );
    }

    const FICHE_ENTITE: &str = r#"
                            <li class="clearfix">
                                <label class="col-md-4 col-xs-12 spacing-helper-m">
                                    Entité publique :
                                </label>
                                <div class="col-md-8 col-xs-12">
                                    Ville d'Ivry-sur-Seine
                                </div>
                            </li>
"#;

    #[test]
    fn la_fiche_nomme_l_acheteur_que_l_api_cache() {
        assert_eq!(
            parse_detail(FICHE_ENTITE).acheteur.as_deref(),
            Some("Ville d'Ivry-sur-Seine")
        );
    }

    #[test]
    fn le_principal_prime_les_secondaires_et_la_procedure_vide_reste_absente() {
        assert_eq!(
            parse_detail(FICHE_MAXIMILIEN),
            DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: Some("33168000".into()),
                procedure: None,
                acheteur: None,
            }
        );
    }

    #[test]
    fn une_fiche_muette_rend_un_detail_vide() {
        let sans_cpv = FICHE_MAXIMILIEN.split("Code CPV").next().unwrap();
        assert_eq!(parse_detail(sans_cpv), DetailAnnonce::default());
    }

    #[test]
    fn le_retrait_dce_suit_l_hote_de_la_fiche_et_tombe_sans_acronyme() {
        assert_eq!(
            retrait_dce(
                "https://marches.maximilien.fr/entreprise/consultation/941833?orgAcronyme=o6a",
                "941833"
            )
            .as_deref(),
            Some(
                "https://marches.maximilien.fr/index.php?page=Entreprise.EntrepriseDemandeTelechargementDce&id=941833&orgAcronyme=o6a"
            )
        );
        assert_eq!(
            retrait_dce(
                "https://www.marches-publics.gouv.fr/entreprise/consultation/3040064",
                "3040064"
            ),
            None
        );
    }
}
