use std::sync::LazyLock;

use jiff::Timestamp;
use regex::Regex;

use lj_core::annonces::{
    departement_from_cp, echeance_ts, normalise_departement, parse_date_fr, parse_heure,
    AnnonceBrute, NatureMarche,
};
use lj_core::referential_labels::departement_label;

use super::{apres_balise, client, dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::Result;

const BASE: &str = "https://www.klekoon.com";
const RECHERCHE: &str =
    "https://www.klekoon.com/rechercher-une-annonce-ou-un-dce-dematerialise-sur-klekoon";
const PER_PAGE: &str = "100";

const POLITESSE: std::time::Duration = std::time::Duration::from_secs(1);

/// Réécrit une URL klekoon.com vers le pont d'egress, seul chemin de sortie du
/// connecteur : le pont est un reverse-proxy à upstream figé, donc seul le
/// chemin (+query) compte. Une URL hors `BASE` (déjà relative, ou étrangère)
/// passe inchangée. Les URLs stockées et servies, elles, restent klekoon.com.
fn via_egress(url: &str, egress: &str) -> String {
    match url.strip_prefix(BASE) {
        Some(rest) => format!("{}{}", egress.trim_end_matches('/'), rest),
        None => url.to_string(),
    }
}

/// Pages consécutives sans aucune consultation ouverte avant d'arrêter.
///
/// Le tri est par **parution**, pas par clôture : les avis clos sont mêlés aux
/// ouverts tout du long, et s'arrêter à la première clôture passée coupe
/// l'énumération bien trop tôt. Le seul repère solide est la page entière
/// muette.
const PAGES_MUETTES: u32 = 2;

/// Plafond dur : l'archive fait 640 pages à 100 par page et on ne moissonne
/// que l'ouvert.
const MAX_PAGES: u64 = 10;

/// Une page de résultats. Le formulaire poste ses dix champs ; `page` et
/// `per_page` sont les seuls qui varient.
async fn page(client: &reqwest::Client, p: u64, egress: &str) -> Result<String> {
    let p = p.to_string();
    send_text(
        client
            .post(via_egress(RECHERCHE, egress))
            .header("Referer", RECHERCHE)
            .form(&[
                ("type_recherche", "1"),
                ("page", &p),
                ("per_page", PER_PAGE),
                ("motcle", ""),
                ("source", "1"),
                ("list_region", ""),
                ("selection_list", ""),
                ("retour", "0"),
                ("id_retour", ""),
                ("critere_emetteur", ""),
            ]),
        &format!("klekoon page {p}"),
    )
    .await
}

/// Total déclaré : le `<select name="critere_emetteur">` du formulaire, servi
/// avec chaque page de résultats, annote ses acheteurs d'un « (N marchés) » qui
/// compte leurs consultations ouvertes. La somme est un compte **indépendant**
/// du listing.
fn total_declare(html: &str) -> u64 {
    static SELECT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)name="critere_emetteur".*?</select>"#).expect("regex select valide")
    });
    static MARCHES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\((\d+)\s*march").expect("regex marchés valide"));
    let Some(bloc) = SELECT.find(html) else {
        return 0;
    };
    MARCHES
        .captures_iter(bloc.as_str())
        .filter_map(|c| c[1].parse::<u64>().ok())
        .sum()
}

/// Le délai de dépôt court encore. Sans clôture lisible, une ligne n'entre
/// jamais dans la fenêtre ouverte du fond (`echeance > NOW()`).
fn ouverte(r: &AnnonceBrute, maintenant: Timestamp) -> bool {
    r.echeance.is_some_and(|e| e > maintenant)
}

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    const LIGNE: &str = r#"<div class="row no-gutters mb-3" id=""#;
    static SLUG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"/appels-offres/avis/([^?"/]+)\?consultation_ID="#).expect("regex slug valide")
    });
    static TITRE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<b><u>(.*?)</u></b>").expect("regex titre valide"));
    static EMETTEUR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Emetteur\s*:\s*(.*?)\s*Source\s*:").expect("regex émetteur valide")
    });
    // « Source : Klekoon - Procédure adaptée » : le moteur préfixe la
    // procédure de son propre nom (la seule source de `type_recherche=1`).
    static SOURCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Source\s*:\s*Klekoon\s*-\s*(.*?)\s*Parution\s*:").expect("regex source valide")
    });
    static PARUTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Parution\s*:\s*(\d{2}/\d{2}/\d{4})").expect("regex parution valide")
    });
    static CLOTURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Clôture\s*:\s*(\d{2}/\d{2}/\d{4})").expect("regex clôture valide")
    });
    // Le libellé d'émetteur se termine par le département entre parenthèses,
    // parfois suivi du fanion « Accès Restreint » — qui n'est pas du nom
    // d'acheteur.
    static DEPT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\s*\((\d{2,3}|2[AB])\)(?:\s*Accès Restreint)?\s*$").expect("regex dept valide")
    });

    let mut out = Vec::new();
    for bloc in html.split(LIGNE).skip(1) {
        let id = bloc.split('"').next().unwrap_or_default();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let corps = apres_balise(bloc);
        let Some(slug) = SLUG.captures(corps).map(|c| c[1].to_string()) else {
            continue;
        };
        // Le dernier bloc de la page emporte tout le pied de page dans sa
        // découpe : les champs se lisent en tête, on borne le texte plat.
        let flat = txt(&corps.chars().take(20_000).collect::<String>());
        let avis = format!("{BASE}/appels-offres/avis/{slug}?consultation_ID={id}&page=1&dce=1");
        let emetteur = EMETTEUR.captures(&flat).map(|c| c[1].to_string());
        out.push(AnnonceBrute {
            moteur: "klekoon",
            source_uid: format!("klekoon:{id}"),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: emetteur
                .as_deref()
                .map(|e| DEPT.replace(e, "").trim().to_string())
                .filter(|a| !a.is_empty()),
            siren: None,
            objet: TITRE
                .captures(corps)
                .map(|c| txt(&c[1]))
                .unwrap_or_default()
                .chars()
                .take(400)
                .collect(),
            nature: None,
            cpv: None,
            // Le suffixe du libellé note la Corse « (20) », code d'avant 1976
            // qui ne tranche pas entre 2A et 2B : hors référentiel, donc pas
            // de département. La fiche, elle, porte le code postal.
            departement: emetteur
                .as_deref()
                .and_then(|e| DEPT.captures(e))
                .and_then(|c| normalise_departement(&c[1]))
                .filter(|d| departement_label(d).is_some()),
            date_publication: PARUTION.captures(&flat).and_then(|c| parse_date_fr(&c[1])),
            echeance: CLOTURE
                .captures(&flat)
                .and_then(|c| parse_date_fr(&c[1]))
                .map(|d| echeance_ts(d, None)),
            url_avis: Some(avis.clone()),
            url_rc: None,
            // Les pièces du marché et le dépôt de pli sont deux vues de la
            // fiche, commutées sur le fragment (`case "#depot_plis"` du
            // routeur de la page) — le dépôt route vers
            // `/dematernet/entreprise_privee_02_enveloppe.asp`.
            url_dce: Some(format!("{avis}#section_pieces")),
            url_depot: Some(format!("{avis}#depot_plis")),
            procedure: SOURCE
                .captures(&flat)
                .map(|c| c[1].trim().to_string())
                .filter(|p| !p.is_empty()),
        });
    }
    out
}

/// Ce que la fiche apprend en plus de la ligne de liste.
#[derive(Debug, Default, PartialEq, Eq)]
struct Fiche {
    reference: Option<String>,
    /// Objet complet : la liste tronque à ~90 caractères.
    objet: Option<String>,
    nature: Option<NatureMarche>,
    /// Division CPV seule : le moteur publie « Classe 45 : Travaux de
    /// construction », jamais un code à huit chiffres.
    cpv: Option<String>,
    departement: Option<String>,
    date_publication: Option<jiff::civil::Date>,
    /// Date **et heure** limite de remise des offres, que la liste n'a pas.
    echeance: Option<Timestamp>,
    procedure: Option<String>,
}

fn champs_fiche(html: &str) -> Fiche {
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Référence de la consultation\s*:\s*(.*?)\s*Mise en ligne")
            .expect("regex référence valide")
    });
    static MISE_EN_LIGNE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Mise en ligne\s*:\s*(\d{2}/\d{2}/\d{4})").expect("regex mise en ligne valide")
    });
    static PASSATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Mode de passation\s*:\s*(.*?)\s*Catégorie de marché")
            .expect("regex passation valide")
    });
    static CATEGORIE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Catégorie de marché\s*:\s*(.*?)\s*Classe d'activité")
            .expect("regex catégorie valide")
    });
    static CLASSE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Classe d'activité\s*:\s*Classe\s*(\d{1,2})\s*:").expect("regex classe valide")
    });
    static LIMITE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"Date limite des offres \(heure de Paris\)\s*:\s*(\d{2}/\d{2}/\d{4})(?:\s+([\d:]+))?",
        )
        .expect("regex date limite valide")
    });
    // Le bloc « Acheteur public » porte l'adresse postale : son code postal
    // tranche la Corse (20213 → 2B) là où le champ « Département : Corse (20) »
    // de la fiche reste ambigu.
    static ADRESSE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Acheteur public\s+(.*?)\s+Informations générales")
            .expect("regex adresse valide")
    });
    static CP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b\d{5}\b").expect("regex code postal valide"));
    static OBJET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Objet de la consultation\s*</span>\s*<p>(.*?)</p>")
            .expect("regex objet valide")
    });

    let flat = txt(html);
    let adresse = ADRESSE
        .captures(&flat)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    Fiche {
        reference: REF
            .captures(&flat)
            .map(|c| c[1].chars().take(120).collect::<String>())
            .filter(|r| !r.is_empty()),
        objet: OBJET
            .captures(html)
            .map(|c| txt(&c[1]).chars().take(400).collect::<String>())
            .filter(|o| !o.is_empty()),
        nature: CATEGORIE
            .captures(&flat)
            .and_then(|c| NatureMarche::parse(&c[1])),
        cpv: CLASSE
            .captures(&flat)
            .map(|c| format!("{:0>2}", &c[1]))
            .filter(|d| lj_core::annonces::cpv_division_uid(d).is_some()),
        departement: CP
            .find_iter(adresse.split("Téléphone").next().unwrap_or_default())
            .last()
            .and_then(|m| departement_from_cp(m.as_str())),
        date_publication: MISE_EN_LIGNE
            .captures(&flat)
            .and_then(|c| parse_date_fr(&c[1])),
        echeance: LIMITE.captures(&flat).and_then(|c| {
            let d = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                d,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        }),
        procedure: PASSATION
            .captures(&flat)
            .map(|c| c[1].trim().to_string())
            .filter(|p| !p.is_empty()),
    }
}

/// Complète une ligne par sa fiche. Une fiche en échec garde la ligne de
/// liste.
async fn completer_par_fiche(
    client: &reqwest::Client,
    mut row: AnnonceBrute,
    egress: &str,
) -> AnnonceBrute {
    let url = row.url_avis.clone().unwrap_or_default();
    let html = match send_text(client.get(via_egress(&url, egress)), "klekoon fiche").await {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(url = %url, error = %e, "klekoon : fiche en échec");
            return row;
        }
    };
    let f = champs_fiche(&html);
    row.reference = f.reference;
    row.nature = f.nature;
    row.cpv = f.cpv;
    if let Some(objet) = f.objet {
        row.objet = objet;
    }
    if f.departement.is_some() {
        row.departement = f.departement;
    }
    if f.date_publication.is_some() {
        row.date_publication = f.date_publication;
    }
    if f.echeance.is_some() {
        row.echeance = f.echeance;
    }
    if f.procedure.is_some() {
        row.procedure = f.procedure;
    }
    row
}

/// Ré-énumère le stock ouvert, puis complète chaque ligne par sa fiche.
/// Tout se fait en série, à `POLITESSE` d'intervalle : ~5 pages + une fiche par
/// consultation ouverte, en parallèle des autres moteurs.
pub async fn enumerate(egress: &str) -> Result<MoteurRun> {
    let client = client();
    let maintenant = Timestamp::now();
    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut declare = 0;
    let mut muettes = 0;
    for p in 1..=MAX_PAGES {
        if p > 1 {
            tokio::time::sleep(POLITESSE).await;
        }
        let html = page(&client, p, egress).await?;
        if p == 1 {
            declare = total_declare(&html);
        }
        let lignes = parse_page(&html);
        if lignes.is_empty() {
            break;
        }
        let avant = rows.len();
        rows.extend(lignes.into_iter().filter(|r| ouverte(r, maintenant)));
        if rows.len() > avant {
            muettes = 0;
        } else {
            muettes += 1;
            if muettes >= PAGES_MUETTES {
                break;
            }
        }
    }

    let mut completes = Vec::with_capacity(rows.len());
    for row in rows {
        tokio::time::sleep(POLITESSE).await;
        completes.push(completer_par_fiche(&client, row, egress).await);
    }
    let rows = completes;

    Ok(MoteurRun {
        annonce: (declare > 0).then_some(declare),
        echecs: 0,
        hotes: Vec::new(),
        rows: dedup_source_uid(rows),
    })
}

/// Page de détail d'un avis. Le CPV n'existe qu'au grain de la division
/// (« Classe 45 »), et « Mode de passation » est vide sur les fiches — la
/// procédure d'une consultation Klekoon se lit en liste, sous « Source :
/// Klekoon - … ».
pub async fn detail(egress: &str, url_avis: &str) -> Result<DetailAnnonce> {
    tokio::time::sleep(POLITESSE).await;
    let html = send_text(client().get(via_egress(url_avis, egress)), "klekoon détail").await?;
    Ok(parse_detail(&html))
}

fn parse_detail(html: &str) -> DetailAnnonce {
    let f = champs_fiche(html);
    DetailAnnonce {
        cpv: f.cpv,
        procedure: f.procedure,
        acheteur: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une ligne réelle de la page de listing (`consultation_ID=93436`), réduite
    /// à la vue « grands écrans » — la vue mobile répète les mêmes champs plus bas
    /// dans le bloc.
    const LIGNE: &str = r#"<div class="row no-gutters mb-3" id="93436">
            <div class="col kle-border">
                <div class="row bg-light-grey no-gutters p-2 kle-border-bottom">
                    <div class="col pl-2">
                        <a href="/appels-offres/avis/travaux-fourniture-pose-portes-fenetres-volets-roulants-marquises?consultation_ID=93436&page=1&dce=1#detail_marche" rel="canonical" class="pointer text-black unselectable"><b><u>Travaux de fourniture et pose de portes, fenêtres, volets roulants et marquises au Groupe scolaire François REVERET </u></b></a>
                    </div>
                </div>
                <div class="row no-gutters bg-light-grey d-none d-lg-flex">
                    <div class="col-6 kle-border-right">
                        <div class="row no-gutters align-items-center h-50 kle-border-bottom">
                            <div class="col pl-3 pt-2 pb-2">
                                <span class="text-black"><b>Emetteur :</b> COMMUNE D'AVERMES (03)
                                     </span>
                            </div>
                        </div>
                        <div class="row no-gutters align-items-center h-50">
                            <div class="col pl-3 pt-2 pb-2">
                                <span class="text-black"><b>Source :</b> Klekoon - Procédure adaptée </span>
                            </div>
                        </div>
                    </div>
                    <div class="col-2 kle-border-right">
                        <div class="row no-gutters align-items-center h-50 kle-border-bottom">
                            <div class="col p-2">
                                <span><b class="mr-2">Parution :</b>07/08/2026</span>
                            </div>
                        </div>
                        <div class="row no-gutters align-items-center h-50">
                            <div class="col p-2">
                                <span class=" text-success "><b class="text-black mr-2">Clôture :</b>28/08/2026</span>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>"#;

    /// Le fanion « Accès Restreint » se colle au libellé d'émetteur, derrière le
    /// département.
    const LIGNE_RESTREINTE: &str = r#"<div class="row no-gutters mb-3" id="93424">
            <div class="col kle-border">
                <div class="row bg-light-grey no-gutters p-2 kle-border-bottom">
                    <div class="col pl-2">
                        <a href="/appels-offres/avis/etude-suivi-mesures-compensatoires-travaux-plantation-zone-ecorhena?consultation_ID=93424&page=1&dce=1#detail_marche" rel="canonical" class="pointer text-black unselectable"><b><u>Etude pour le suivi des mesures compensatoires des travaux de plantation de la zone EcoRhéna </u></b></a>
                    </div>
                </div>
                <div class="row no-gutters bg-light-grey d-none d-lg-flex">
                    <div class="col-6 kle-border-right">
                        <div class="row no-gutters align-items-center h-50 kle-border-bottom">
                            <div class="col pl-3 pt-2 pb-2">
                                <span class="text-black"><b>Emetteur :</b> SM GESTION PORT RHENAN COLMAR (68)
                                    <span class="text-warning">&nbsp;Accès Restreint</span>
                                     </span>
                            </div>
                        </div>
                        <div class="row no-gutters align-items-center h-50">
                            <div class="col pl-3 pt-2 pb-2">
                                <span class="text-black"><b>Source :</b> Klekoon - Marché négocié sans publicité ni mise en concurrence préalable </span>
                            </div>
                        </div>
                    </div>
                    <div class="col-2 kle-border-right">
                        <div class="row no-gutters align-items-center h-50 kle-border-bottom">
                            <div class="col p-2">
                                <span><b class="mr-2">Parution :</b>05/08/2026</span>
                            </div>
                        </div>
                        <div class="row no-gutters align-items-center h-50">
                            <div class="col p-2">
                                <span class=" text-success "><b class="text-black mr-2">Clôture :</b>11/09/2026</span>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>"#;

    fn le(annee: i16, mois: i8, jour: i8, h: i8, m: i8) -> Timestamp {
        echeance_ts(
            jiff::civil::date(annee, mois, jour),
            Some(jiff::civil::time(h, m, 0, 0)),
        )
    }

    #[test]
    fn ligne_de_liste() {
        let r = &parse_page(LIGNE)[0];
        assert_eq!(r.source_uid, "klekoon:93436");
        assert_eq!(
            r.objet,
            "Travaux de fourniture et pose de portes, fenêtres, volets roulants et marquises au Groupe scolaire François REVERET"
        );
        assert_eq!(r.acheteur.as_deref(), Some("COMMUNE D'AVERMES"));
        assert_eq!(r.departement.as_deref(), Some("03"));
        assert_eq!(r.procedure.as_deref(), Some("Procédure adaptée"));
        assert_eq!(
            r.date_publication,
            Some(jiff::civil::date(2026, 8, 7)),
            "la parution est la date de mise en ligne, pas la clôture"
        );
        // Sans heure, la consultation reste ouverte jusqu'au bout du jour civil.
        assert_eq!(
            r.echeance,
            Some(echeance_ts(jiff::civil::date(2026, 8, 28), None))
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some("https://www.klekoon.com/appels-offres/avis/travaux-fourniture-pose-portes-fenetres-volets-roulants-marquises?consultation_ID=93436&page=1&dce=1")
        );
        assert_eq!(
            r.url_depot.as_deref(),
            Some("https://www.klekoon.com/appels-offres/avis/travaux-fourniture-pose-portes-fenetres-volets-roulants-marquises?consultation_ID=93436&page=1&dce=1#depot_plis")
        );
    }

    #[test]
    fn le_fanion_acces_restreint_quitte_le_libelle() {
        let r = &parse_page(LIGNE_RESTREINTE)[0];
        assert_eq!(r.acheteur.as_deref(), Some("SM GESTION PORT RHENAN COLMAR"));
        assert_eq!(r.departement.as_deref(), Some("68"));
        assert_eq!(
            r.procedure.as_deref(),
            Some("Marché négocié sans publicité ni mise en concurrence préalable")
        );
    }

    #[test]
    fn la_corse_du_libelle_ne_donne_pas_de_departement() {
        let html = LIGNE.replace("COMMUNE D'AVERMES (03)", "PENTA DI CASINCA (20)");
        let r = &parse_page(&html)[0];
        assert_eq!(r.acheteur.as_deref(), Some("PENTA DI CASINCA"));
        assert_eq!(r.departement, None);
    }

    #[test]
    fn seule_une_page_sans_ouverte_arrete_l_enumeration() {
        let maintenant = le(2026, 8, 8, 12, 0);
        let melangee = format!(
            "{}{}",
            LIGNE
                .replace("id=\"93436\"", "id=\"93088\"")
                .replace("28/08/2026", "09/07/2026"),
            LIGNE_RESTREINTE
                .replace("id=\"93424\"", "id=\"93080\"")
                .replace("11/09/2026", "21/08/2026")
        );
        let lignes = parse_page(&melangee);
        assert_eq!(lignes.len(), 2);
        let ouvertes: Vec<_> = lignes.iter().filter(|r| ouverte(r, maintenant)).collect();
        assert_eq!(ouvertes.len(), 1, "la page porte encore une ouverte");
        assert_eq!(ouvertes[0].source_uid, "klekoon:93080");

        let close = LIGNE.replace("28/08/2026", "11/03/2026");
        assert!(!parse_page(&close).iter().any(|r| ouverte(r, maintenant)));
    }

    #[test]
    fn un_bloc_sans_cloture_ne_compte_pas_comme_ouvert() {
        let html = LIGNE.replace(
            r#"<b class="text-black mr-2">Clôture :</b>28/08/2026"#,
            r#"<b class="text-black mr-2">Clôture :</b>"#,
        );
        let r = &parse_page(&html)[0];
        assert_eq!(r.echeance, None);
        assert_eq!(r.source_uid, "klekoon:93436");
        assert!(!ouverte(r, le(2026, 8, 8, 12, 0)));
    }

    /// Fiche réelle (`consultation_ID=93409`), réduite aux deux blocs que lit
    /// `champs_fiche`.
    const FICHE: &str = r#"<div class="col-8 col-lg-10">
        <span>Acheteur public</span>
        <p>
            MAIRIE DE BANDRELE
            <br>
            PLACE DE LA MAIRIE<br />97660 BANDRELE FR
            <br>
            T&eacute;l&eacute;phone : 0102030405
        </p>
    </div>
    <div class="col-sm-12">
        <span>Informations g&eacute;n&eacute;rales </span>
        <p>
            R&eacute;f&eacute;rence de la consultation : K4192977
            <br>
            Mise en ligne : 30/07/2026 08:14:11
            <br>
            Mode de passation :
            <br>
            Cat&eacute;gorie de march&eacute; : Travaux
            <br />
            Classe d'activit&eacute; : Classe 31 : Machines, appareils, équipements et consommables électriques; éclairage<br />
            <br />
            D&eacute;partement : Mayotte  (976)
            <br />
            Date limite des candidatures (heure de Paris) :
            <br />
            Date limite des offres (heure de Paris) : <font color=#00B71C>28/08/2026 10:00</font>
        </p>
    </div>
    <div class="col-sm-12">
        <span>Objet de la consultation </span>
        <p>Travaux de rénovation de l’éclairage du terrain de football de Nyambadao</p>
    </div>"#;

    #[test]
    fn la_fiche_donne_reference_heure_et_division_cpv() {
        let f = champs_fiche(FICHE);
        assert_eq!(f.reference.as_deref(), Some("K4192977"));
        assert_eq!(
            f.objet.as_deref(),
            Some("Travaux de rénovation de l’éclairage du terrain de football de Nyambadao")
        );
        assert_eq!(f.nature, Some(NatureMarche::Travaux));
        assert_eq!(f.cpv.as_deref(), Some("31"));
        assert_eq!(f.departement.as_deref(), Some("976"));
        assert_eq!(f.date_publication, Some(jiff::civil::date(2026, 7, 30)));
        assert_eq!(f.echeance, Some(le(2026, 8, 28, 10, 0)));
        assert_eq!(f.procedure, None);
    }

    #[test]
    fn le_code_postal_tranche_la_corse_et_la_reference_peut_manquer() {
        let f = champs_fiche(
            &FICHE
                .replace("97660 BANDRELE FR", "20213 PENTA DI CASINCA FR")
                .replace("Mayotte  (976)", "Corse  (20)")
                .replace("consultation : K4192977", "consultation : "),
        );
        assert_eq!(f.departement.as_deref(), Some("2B"));
        assert_eq!(f.reference, None);
        let f3 = champs_fiche(&FICHE.replace("Classe 31 :", "Classe 3 :"));
        assert_eq!(f3.cpv.as_deref(), Some("03"));
    }

    #[test]
    fn la_page_d_erreur_rend_un_detail_vide() {
        let html = "<h1>&nbsp;</h1><p>La page que vous recherchez n'est pas actuellement \
                    disponible.</p><a href=\"/inscription-klekoon\">Inscrivez-vous</a>";
        assert_eq!(champs_fiche(html), Fiche::default());
        assert_eq!(parse_detail(html), DetailAnnonce::default());
    }

    #[test]
    fn total_declare_somme_le_selecteur_d_acheteurs() {
        let html = r#"<select class="form-control w-100 pointer" name="critere_emetteur" id="critere-emetteur" >
            <option  selected="selected"  value="0">Sélectionner un acheteur public</option>
            <optgroup label="01 - Ain">
                <option value="82890" >Mairie de Ferney-Voltaire (0 marché) (39 données)</option>
            <optgroup label="03 - Allier">
                <option value="611519" >COMMUNE D'AVERMES (1 marché) (8 données)</option>
            <optgroup label="57 - Moselle">
                <option value="94582" >MAIRIE SAINT AVOLD (15 marchés) (272 données)</option>
            </select>
            <p>Klekoon publie (999 marchés) hors du sélecteur</p>"#;
        assert_eq!(total_declare(html), 16);
        assert_eq!(total_declare("<html>pas de sélecteur</html>"), 0);
    }
}
