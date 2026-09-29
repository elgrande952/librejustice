use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_annonces::zones::zones_departements;
use lj_annonces::{
    departement_from_cp, echeance_ts, parse_heure, AnnonceBrute, ContenuAnnonce, LotAnnonce,
    NatureMarche,
};
use lj_core::dates::parse_date_fr;

use super::{dedup_source_uid, fiche, send_text, txt, DetailAnnonce, MoteurRun};
use lj_sources::error::{Result, SourceError};

const BASE: &str = "https://www.e-marchespublics.com";
const PER_PAGE: u64 = 10;
const SESSIONS: u64 = 12;
/// Borne dure de pages explorées ; au-delà du stock actif les cartes sont
/// vides et la tranche s'arrête d'elle-même.
const MAX_PAGES: u64 = 400;

struct Recherche {
    client: wreq::Client,
    token: String,
}

impl Recherche {
    async fn ouvrir() -> Result<Self> {
        let client = super::client();
        let page = send_text(
            client.get(format!("{BASE}/appel-offre")),
            "dematis appel-offre",
        )
        .await?;
        static TOKEN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#"name="_token"[^>]*value="([^"]*)""#).unwrap());
        let token = TOKEN
            .captures(&page)
            .ok_or_else(|| SourceError::Invalid("dematis: jeton _token absent".into()))?[1]
            .to_string();
        Ok(Self { client, token })
    }

    async fn page(&self, n: u64) -> Result<(u64, Vec<AnnonceBrute>)> {
        let body = send_text(
            self.client
                .post(format!("{BASE}/notices/search/update"))
                .header("X-Requested-With", "XMLHttpRequest")
                .header("Referer", format!("{BASE}/appel-offre"))
                .form(&[("_token", self.token.as_str()), ("page", &n.to_string())]),
            &format!("dematis page {n}"),
        )
        .await?;
        let j: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| SourceError::Invalid(format!("dematis: JSON: {e}")))?;
        let vue = j.get("view").and_then(|v| v.as_str()).unwrap_or_default();
        Ok((
            j.get("nbResult").and_then(|v| v.as_u64()).unwrap_or(0),
            parse_view(vue),
        ))
    }

    /// The same endpoint, criteria carried this time. It reads `what` only when
    /// all five fields of the form arrive: one missing and it serves the whole
    /// active stock without a word.
    async fn mots_cles(&self, mots: &str) -> Result<(u64, Vec<AnnonceBrute>)> {
        let body = send_text(
            self.client
                .post(format!("{BASE}/notices/search/update"))
                .header("X-Requested-With", "XMLHttpRequest")
                .header("Referer", format!("{BASE}/appel-offre"))
                .form(&[
                    ("_token", self.token.as_str()),
                    ("what", mots),
                    ("where", ""),
                    ("category", ""),
                    ("notice_type", ""),
                    ("tri", ""),
                    ("page", "1"),
                ]),
            "dematis recherche",
        )
        .await?;
        let j: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| SourceError::Invalid(format!("dematis: JSON: {e}")))?;
        let vue = j.get("view").and_then(|v| v.as_str()).unwrap_or_default();
        Ok((
            j.get("nbResult").and_then(|v| v.as_u64()).unwrap_or(0),
            parse_view(vue),
        ))
    }
}

/// The consultations the platform serves for a market reference. The index cuts
/// the reference on its separators and answers with an OR, and the listing
/// carries no reference to depart the page: only a single answer designates.
pub async fn chercher(reference: &str) -> Result<Vec<AnnonceBrute>> {
    let (nb, rows) = Recherche::ouvrir().await?.mots_cles(reference).await?;
    Ok(if nb == 1 { rows } else { Vec::new() })
}

fn parse_view(vue: &str) -> Vec<AnnonceBrute> {
    static LIEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"href=\x22?(/appel-offre/([a-z0-9\-]+)/([a-z0-9\-]+)/([a-z0-9\-]+)/(\d+)/(\d+))",
        )
        .unwrap()
    });
    static TITRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)class="box-header-title".*?<div[^>]*>(.*?)</div>"#).unwrap()
    });
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Date limite de l'offre\s*:?\s*(\d{2}/\d{2}/\d{4})(?:\s*à\s*([\dhH:]+))?")
            .unwrap()
    });
    static PUB: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:Publi\w+|Mise en ligne)[^\d]{0,20}(\d{2}/\d{2}/\d{4})").unwrap()
    });
    // L'acheteur suit l'objet et précède « CP Commune » puis la nature.
    static ACHETEUR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?s)^\s*(.{2,140}?)\s*\b(\d{5})\b\s*(.{0,60}?)\s*(?:Travaux|Services|Fournitures|Autres|Date limite|$)",
        )
        .unwrap()
    });

    let mut out = Vec::new();
    for bloc in vue.split(r#"<div class="box">"#).skip(1) {
        let Some(lien) = LIEN.captures(bloc) else {
            continue;
        };
        let (url, aid) = (lien[1].to_string(), lien[5].to_string());
        let flat = txt(bloc);
        let objet = TITRE
            .captures(bloc)
            .map(|c| txt(&c[1]).chars().take(400).collect::<String>())
            .unwrap_or_default();
        let reste = flat.strip_prefix(objet.as_str()).unwrap_or(&flat);
        let ach = ACHETEUR.captures(reste);
        let cp = ach.as_ref().map(|c| c[2].to_string());
        let echeance = LIM.captures(&flat).and_then(|c| {
            let d = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                d,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        });
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "dematis",
            source_uid: format!("dematis:{aid}"),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: ach.as_ref().map(|c| {
                c[1].trim()
                    .trim_end_matches("Accéder au site acheteur")
                    .trim()
                    .to_string()
            }),
            siren: None,
            objet,
            nature: NatureMarche::parse(&flat),
            cpv: None,
            zones: zones_departements(cp.as_deref().and_then(departement_from_cp)),
            date_publication: PUB.captures(&flat).and_then(|c| parse_date_fr(&c[1])),
            echeance,
            url_avis: Some(format!("{BASE}{url}")),
            // The tabs of a notice are AVIS, DOSSIER, QUESTIONS, DEPOT: there
            // is no second tab, so no separate rules document, and the
            // anonymous retrieval lives in DOSSIER, the same piece under
            // `private` asking for an account.
            url_rc: None,
            url_dce: Some(format!("{BASE}{url}#tabs-3")),
            url_depot: Some(format!("{BASE}{url}#tabs-5")),
            procedure: None,
        });
    }
    out
}

/// Une tranche de pages, dans sa propre session (curseur serveur propre).
async fn tranche(lo: u64, hi: u64) -> Result<Vec<AnnonceBrute>> {
    let recherche = Recherche::ouvrir().await?;
    let mut rows = Vec::new();
    for p in lo..=hi {
        let (_, got) = recherche.page(p).await?;
        if got.is_empty() {
            break;
        }
        rows.extend(got);
    }
    Ok(rows)
}

/// Ré-énumère le stock actif. `annonce: None` : `nbResult` compte les clos.
pub async fn enumerate() -> Result<MoteurRun> {
    let tete = Recherche::ouvrir().await?;
    let (nb_result, premiere) = tete.page(1).await?;
    let npages = nb_result.div_ceil(PER_PAGE).clamp(1, MAX_PAGES);
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
        annonce: None,
        echecs: 0,
        hotes: Vec::new(),
        rows: premiere,
    };
    for res in resultats {
        match res {
            Ok(rows) => run.rows.extend(rows),
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(error = %e, "dematis: slice failed, stock kept");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

/// Three layouts, three words for the same paragraph: `Description` in the flat
/// eForms sections, `Description succincte du marché` in the FNS simplified
/// form, `Objet du marché` in the buyer's free body. `Objet` is none of them:
/// the eForms layout writes the nature of the market under it.
const DESCRIPTIONS: [&str; 3] = [
    "description",
    "description succincte du marche",
    "objet du marche",
];

/// The flat eForms layout opens each lot with `5.1 Lot: LOT-0001 Titre: …`,
/// then repeats under it the labels the market level already used. Document
/// order binds them: what follows a lot header belongs to that lot, and the
/// market's own description and CPV precede the first one.
fn lots_eforms(champs: &[fiche::Champ]) -> Vec<LotAnnonce> {
    static ENTETE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(LOT-\d+)(?:\s*Titre\s*:\s*(.+))?$").expect("regex entête de lot valide")
    });
    let mut out: Vec<LotAnnonce> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    for c in champs {
        match c.cle.as_str() {
            "lot" => {
                let Some(m) = ENTETE.captures(&c.valeur) else {
                    continue;
                };
                if !vus.insert(m[1].to_string()) {
                    continue;
                }
                out.push(LotAnnonce {
                    nom: m.get(2).map(|t| t.as_str().trim().to_string()),
                    description: None,
                    cpv: None,
                });
            }
            "description" => {
                if let Some(lot) = out.last_mut() {
                    lot.description.get_or_insert_with(|| c.texte());
                }
            }
            "nomenclature principale" => {
                if let Some(lot) = out.last_mut() {
                    if lot.cpv.is_none() {
                        lot.cpv = fiche::cpv_de(&c.valeur);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// L'en-tête de fiche porte « `Nature - Procédure` » derrière l'icône
/// `coll-ico far fa-file-alt`, seul repère commun aux deux formes d'avis servies
/// (formulaire eForms rendu en sections numérotées, ou corps libre de
/// l'acheteur). Le CPV ne vit que dans le corps et sous trois libellés selon la
/// forme ; le premier du document est celui du marché, les suivants ceux des
/// lots.
fn parse_detail(page: &str) -> DetailAnnonce {
    static PROCEDURE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"coll-ico far fa-file-alt"></i>\s*([^<]*)</p>"#).unwrap());
    static CPV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(?:Nomenclature principale\s*\(\s*cpv\s*\)|Classification CPV\s*:\s*Principale|CPV\s*-\s*Objet principal)\s*:\s*(\d{8}(?:-\d)?)",
        )
        .unwrap()
    });

    let champs = fiche::champs(page);
    let mut contenu = fiche::contenu(&champs, fiche::premier(&champs, &DESCRIPTIONS));
    if contenu.lots.is_empty() {
        contenu.lots = lots_eforms(&champs);
    }
    DetailAnnonce {
        contenu,
        zones: Vec::new(),
        cpv: CPV.captures(&txt(page)).map(|c| c[1].to_string()),
        procedure: PROCEDURE.captures(page).and_then(|c| {
            let entete = txt(&c[1]);
            entete
                .split_once(" - ")
                .map(|(_nature, p)| p.trim().to_string())
        }),
        acheteur: None,
    }
}

/// Page de détail d'un avis : le contenu du marché, plus le CPV et la
/// procédure, absents de la ligne de listing.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let page = send_text(super::client().get(url_avis), "dematis détail").await?;
    Ok(parse_detail(&page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eforms_prend_la_nomenclature_principale() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Fourniture - Appel d&#039;Offres Ouvert
             </p>
             <div title="block02011003">
                <div><span class="label">Nomenclature principale</span><span class="text"> </span><span class="text">(</span><span class="value">cpv</span><span class="text">): </span><span class="value">38652100</span><span class="text"> </span><span class="dynamic-label">Appareils de projection</span></div>
             </div>
             <div title="block04010404">
                <div><span class="label">Nomenclature complémentaire</span><span class="text"> </span><span class="text">(</span><span class="value">cpv</span><span class="text">): </span><span class="value">32321200</span><span class="text"> </span><span class="dynamic-label">Equipement audiovisuel</span></div>
             </div>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: Some("38652100".into()),
                procedure: Some("Appel d'Offres Ouvert".into()),
                acheteur: None,
            }
        );
    }

    #[test]
    fn corps_libre_prend_le_cpv_du_marche_pas_celui_du_lot() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Travaux - Procédure Adaptée
             </p>
             <div class="html notice-html-content"><p><strong>Classification CPV :</strong></p><p><strong>Principale :</strong> 45454100 - Travaux de réfection</p><p><strong>Lot N° 1</strong> - Terrassement- Maçonnerie - CPV 45112500</p></div>"#;
        let d = parse_detail(page);
        assert_eq!(d.cpv.as_deref(), Some("45454100"));
        assert_eq!(d.procedure.as_deref(), Some("Procédure Adaptée"));
        assert_eq!(
            d.contenu.lots,
            [LotAnnonce {
                nom: Some("Terrassement- Maçonnerie".to_string()),
                description: None,
                cpv: Some("45112500".to_string()),
            }]
        );
    }

    /// Real sections of notice 1182283, the market level then the first two of
    /// its six lots. The market carries its own `Description` and
    /// `Nomenclature principale`, ahead of the first lot header.
    const FICHE_EFORMS: &str = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Service - Appel d&#039;Offres Ouvert
             </p>
             <h3 id="section-2.1-mFfMP"><span class="numbering">2.1&nbsp;</span><span class="label">Procédure</span></h3>
             <div title="block020101"><div><span class="label">Titre</span><span class="text">: </span><span class="value">Prestation des services d'assurances</span></div></div>
             <div title="block020102"><div><span class="label">Description</span><span class="text">: </span><span class="value">prestation des services d'assurance de l'ehpad Saint Fran&ccedil;ois d'Assise</span></div></div>
             <div title="block020103"><div><span class="label">Identifiant de la procédure</span><span class="text">: </span><span class="value">14f98a60-02b8-4b25-ac11-a0cb3a676598</span></div></div>
             <h3 id="section-2.1.1"><span class="numbering">2.1.1&nbsp;</span><span class="label">Objet</span></h3>
             <div title="block02011001"><div><span class="label">Nature principale du marché</span><span class="text">: </span><span class="dynamic-label">Services</span></div></div>
             <div title="block02011003"><div><span class="label">Nomenclature principale</span><span class="text"> </span><span class="text">(</span><span class="value">cpv</span><span class="text">): </span><span class="value">66500000</span><span class="text"> </span><span class="dynamic-label">Services d'assurance</span></div></div>
             <h2 id="section-5">5.&nbsp;<span class="label">Lot</span></h2>
             <h3 id="section-5.1-YivOj"><span class="numbering">5.1&nbsp;</span><span class="label">Lot</span><span class="text">: </span><span class="value">LOT-0001</span></h3>
             <div title="block040101"><div><span class="label">Titre</span><span class="text">: </span><span class="value">Assurance Responsabilit&eacute; Civile</span></div></div>
             <div title="block040102"><div><span class="label">Description</span><span class="text">: </span><span class="value">assurance de la responsabilit&eacute; civile</span></div></div>
             <div title="block04010403"><div><span class="label">Nomenclature principale</span><span class="text"> </span><span class="text">(</span><span class="value">cpv</span><span class="text">): </span><span class="value">66516000</span></div></div>
             <h3 id="section-5.1-KpQr2"><span class="numbering">5.1&nbsp;</span><span class="label">Lot</span><span class="text">: </span><span class="value">LOT-0003</span></h3>
             <div title="block040101"><div><span class="label">Titre</span><span class="text">: </span><span class="value">Assurance automobile</span></div></div>
             <div title="block040102"><div><span class="label">Description</span><span class="text">: </span><span class="value">assurance de la flotte automobile de l'&eacute;tablissement</span></div></div>
             <div title="block04010403"><div><span class="label">Nomenclature principale</span><span class="text"> </span><span class="text">(</span><span class="value">cpv</span><span class="text">): </span><span class="value">66514110</span></div></div>"#;

    #[test]
    fn a_flat_eforms_lot_takes_the_title_description_and_cpv_that_follow_it() {
        let d = parse_detail(FICHE_EFORMS);
        assert_eq!(
            d.contenu.lots,
            [
                LotAnnonce {
                    nom: Some("Assurance Responsabilité Civile".to_string()),
                    description: Some("assurance de la responsabilité civile".to_string()),
                    cpv: Some("66516000".to_string()),
                },
                LotAnnonce {
                    nom: Some("Assurance automobile".to_string()),
                    description: Some(
                        "assurance de la flotte automobile de l'établissement".to_string()
                    ),
                    cpv: Some("66514110".to_string()),
                },
            ]
        );
        assert_eq!(d.cpv.as_deref(), Some("66500000"));
    }

    #[test]
    fn the_market_description_beats_the_nature_written_under_objet() {
        assert_eq!(
            parse_detail(FICHE_EFORMS).contenu.description.as_deref(),
            Some("prestation des services d'assurance de l'ehpad Saint François d'Assise")
        );
    }

    /// Real FNS simplified form of notice 1182542.
    #[test]
    fn the_fns_form_names_its_description_succincte() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Travaux - Procédure Adaptée
             </p>
             <div class="html notice-html-content">
             <p>Intitulé du marché :<br> Travaux d'Effacement des réseaux aériens</p>
             <p>Description succincte du marché :<br> Travaux d'Effacement des réseaux aériens électriques, télécommunication et éclairage public - Grande Rue/Route Départementale n°206</p>
             <p>Valeur estimée hors taxes du besoin :<br>  Euro</p>
             <p>Marché alloti :<br> Non</p>
             </div>"#;
        let d = parse_detail(page);
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "Travaux d'Effacement des réseaux aériens électriques, télécommunication et \
                 éclairage public - Grande Rue/Route Départementale n°206"
            )
        );
        assert!(d.contenu.lots.is_empty());
        assert_eq!(
            d.contenu.montant_estime, None,
            "an amount with no figure is no amount"
        );
    }

    #[test]
    fn formulaire_joue_cpv_objet_principal() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Travaux - Procédure Adaptée
             </p>
             <p><strong>Intitulé du marché :</strong> Restructuration intérieure de la mairie</p><p>CPV - Objet principal : 45421000.</p><p>Type de marché : Travaux</p>"#;
        assert_eq!(parse_detail(page).cpv.as_deref(), Some("45421000"));
    }

    #[test]
    fn avis_sans_cpv_rend_la_procedure_seule() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Autre - Procédure Adaptée
             </p>
             <div class="html notice-html-content"><p>2.Mode de passation du marché</p><p>Procédure adaptée.</p></div>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: None,
                procedure: Some("Procédure Adaptée".into()),
                acheteur: None,
            }
        );
    }
}
