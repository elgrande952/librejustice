//! URL de la décision chez son éditeur officiel, dérivée de `source_uid`.
//!
//! Le fonds administratif hors Légifrance passe par le portail opendata du
//! Conseil d'État, à `/recherche/shareFile/{Code_Juridiction}/{fichier sans
//! .xml}` : forme lue dans son propre moteur, pas documentée ailleurs.

/// Ce qu'une provenance apporte pour construire son URL officielle.
pub struct SourceRef<'a> {
    pub source: &'a str,
    pub source_uid: &'a str,
    /// `Dossier.Code_Juridiction` du flux opendata (« TA35 »), seul cas où
    /// l'uid ne porte pas la juridiction.
    pub court_code: Option<&'a str>,
    /// URL ou chemin que la source publie elle-même : `META_JURI_CONSTIT.URL_CC`
    /// au Conseil constitutionnel, `fiche_url` de la fiche éditoriale à la CNDA.
    pub published_url: Option<&'a str>,
    /// Date de lecture ISO et numéro de dossier du fonds ArianeWeb `dce`, dont
    /// l'uid ne porte que l'identifiant interne du greffe.
    pub ariane_ref: Option<(&'a str, &'a str)>,
}

/// URL chez l'éditeur officiel, ou `None` quand la provenance n'en expose pas.
pub fn official_url(src: &SourceRef<'_>) -> Option<String> {
    let uid = src
        .source_uid
        .strip_prefix(src.source)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(src.source_uid);

    match src.source {
        "dila-jade" => Some(format!("https://www.legifrance.gouv.fr/ceta/id/{uid}")),
        "dila-cnil" => Some(format!("https://www.legifrance.gouv.fr/cnil/id/{uid}")),
        "dila-constit" => src.published_url.map(str::to_string),
        "judilibre" => Some(format!("https://www.courdecassation.fr/decision/{uid}")),
        "cjue" => Some(format!(
            "https://eur-lex.europa.eu/legal-content/FR/TXT/?uri=CELEX:{uid}"
        )),
        "cedh" => Some(format!("https://hudoc.echr.coe.int/fre?i={uid}")),
        "cnda" => src
            .published_url
            .map(|path| format!("https://www.cnda.fr{path}")),
        // La forme `dce/<id>` répond 500 chez eux, seul `numéro@date` résout :
        // l'uid le porte côté `ariane-web`, le payload côté `ariane-dce`.
        "ariane-web" => uid
            .split_once('@')
            .map(|(dossier, date)| ariane_web(date, dossier)),
        "ariane-dce" => src
            .ariane_ref
            .map(|(date, dossier)| ariane_web(date, dossier)),
        "opendata" => {
            let file = uid.rsplit('/').next()?.strip_suffix(".xml")?;
            let court = src.court_code?;
            Some(format!(
                "https://opendata.justice-administrative.fr/recherche/shareFile/{court}/{file}"
            ))
        }
        _ => None,
    }
}

fn ariane_web(date: &str, dossier: &str) -> String {
    format!("https://www.conseil-etat.fr/fr/arianeweb/CE/decision/{date}/{dossier}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r<'a>(source: &'a str, uid: &'a str) -> SourceRef<'a> {
        SourceRef {
            source,
            source_uid: uid,
            court_code: None,
            published_url: None,
            ariane_ref: None,
        }
    }

    #[test]
    fn chaque_provenance_rend_lurl_de_son_editeur() {
        for (source, uid, attendu) in [
            (
                "dila-jade",
                "dila-jade/CETATEXT000050414461",
                "https://www.legifrance.gouv.fr/ceta/id/CETATEXT000050414461",
            ),
            (
                "dila-cnil",
                "dila-cnil/CNILTEXT000051911372",
                "https://www.legifrance.gouv.fr/cnil/id/CNILTEXT000051911372",
            ),
            (
                "judilibre",
                "judilibre/6194ba445a317cc1d116fbb9",
                "https://www.courdecassation.fr/decision/6194ba445a317cc1d116fbb9",
            ),
            (
                "cjue",
                "cjue/62025TJ0762",
                "https://eur-lex.europa.eu/legal-content/FR/TXT/?uri=CELEX:62025TJ0762",
            ),
            (
                "cedh",
                "cedh/001-247439",
                "https://hudoc.echr.coe.int/fre?i=001-247439",
            ),
            (
                "ariane-web",
                "ariane-web/67581@1967-02-17",
                "https://www.conseil-etat.fr/fr/arianeweb/CE/decision/1967-02-17/67581",
            ),
        ] {
            assert_eq!(official_url(&r(source, uid)).as_deref(), Some(attendu));
        }
    }

    /// Les deux profondeurs de chemin du flux opendata donnent la même URL : le
    /// code juridiction vient du payload, jamais du chemin de l'archive.
    #[test]
    fn opendata_construit_depuis_le_code_juridiction_et_le_fichier() {
        for uid in [
            "TA_202206.zip/TA06/DTA_1900103_20220630.xml",
            "CAA_202203.zip/DTA_1900103_20220630.xml",
        ] {
            let src = SourceRef {
                source: "opendata",
                source_uid: uid,
                court_code: Some("TA06"),
                published_url: None,
                ariane_ref: None,
            };
            assert_eq!(
                official_url(&src).as_deref(),
                Some(
                    "https://opendata.justice-administrative.fr/recherche/shareFile/TA06/DTA_1900103_20220630"
                )
            );
        }
    }

    #[test]
    fn le_conseil_constitutionnel_sert_son_url_telle_quelle() {
        let src = SourceRef {
            source: "dila-constit",
            source_uid: "dila-constit/CONSTEXT000054617308",
            court_code: None,
            published_url: Some(
                "https://www.conseil-constitutionnel.fr/decision/2026/2026904DC.htm",
            ),
            ariane_ref: None,
        };
        assert_eq!(
            official_url(&src).as_deref(),
            Some("https://www.conseil-constitutionnel.fr/decision/2026/2026904DC.htm")
        );
    }

    /// La fiche éditoriale est la page CNDA de la décision : le PDF sous
    /// `/Media` n'en est pas une, et l'uid ne porte que le numéro de dossier.
    #[test]
    fn la_cnda_passe_par_le_chemin_de_sa_fiche() {
        let src = SourceRef {
            source: "cnda",
            source_uid: "cnda/25013796",
            court_code: None,
            published_url: Some(
                "/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/la-cour-reconnait-le-groupe-social-des-personnes-homosexuelles-au-guatemala",
            ),
            ariane_ref: None,
        };
        assert_eq!(
            official_url(&src).as_deref(),
            Some(
                "https://www.cnda.fr/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/la-cour-reconnait-le-groupe-social-des-personnes-homosexuelles-au-guatemala"
            )
        );
    }

    /// `ariane-dce` rend la même URL qu'`ariane-web`, la référence venant du
    /// payload et non de l'uid.
    #[test]
    fn le_fonds_dce_rend_lurl_arianeweb_depuis_sa_reference() {
        let src = SourceRef {
            source: "ariane-dce",
            source_uid: "ariane-dce/89327",
            court_code: None,
            published_url: None,
            ariane_ref: Some(("2007-04-27", "269872")),
        };
        assert_eq!(
            official_url(&src).as_deref(),
            Some("https://www.conseil-etat.fr/fr/arianeweb/CE/decision/2007-04-27/269872")
        );
    }

    #[test]
    fn sans_permalien_public_rien_nest_invente() {
        assert!(official_url(&r("adde", "adde/caa-paris-30-janvier-2026-24pa04236")).is_none());
        assert!(official_url(&r("cnda", "cnda/455708")).is_none());
        assert!(official_url(&r("ariane-web", "ariane-web/dce/115849")).is_none());
        assert!(official_url(&r("ariane-dce", "ariane-dce/89327")).is_none());
        assert!(official_url(&r("dila-constit", "dila-constit/CONSTEXT000054617308")).is_none());
        let sans_code = SourceRef {
            source: "opendata",
            source_uid: "CAA_202203.zip/DCA_19DA02206_20220331.xml",
            court_code: None,
            published_url: None,
            ariane_ref: None,
        };
        assert!(official_url(&sans_code).is_none());
    }
}
