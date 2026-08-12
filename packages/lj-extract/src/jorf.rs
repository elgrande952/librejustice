//! Parser PUR du fond JORF (bulk DILA) — `payload XML → JorfArticle /
//! JorfTexte`. Aucun I/O : les octets sont fournis par `lj-ingest`.
//!
//! Le fond JORF partage la **structure XML de LEGI** : mêmes tags
//! `META_COMMUN/ID`, `META_ARTICLE/NUM|ETAT|DATE_DEBUT|DATE_FIN`,
//! `CONTEXTE/TEXTE@cid`, `BLOC_TEXTUEL/CONTENU`, mêmes sentinelles de date. Ce
//! module réutilise les helpers de [`crate::legi`] (`find_anywhere`,
//! `date_or_none`, `clean_contenu`, `collect_titre_tm`) et n'ajoute que les
//! divergences JORF :
//! - **`NUM` souvent absent** (annonces, actes `TYPE=AUTONOME`) → `num:
//!   Option<String>` (LEGI le rend obligatoire ; JORF non) ; idem `DATE_DEBUT`
//!   (les annonces n'ont qu'une sentinelle) ;
//! - **métadonnées texte plus riches** : `TITREFULL` (libellé descriptif vs
//!   `TITRE` court), `NATURE` (DECRET/ARRETE/LOI/ANNONCES…), mots-clés `MC`
//!   (détection des traités) et le graphe de liens `LIENS/LIEN` (rattachement
//!   avenant→accord initial).

use crate::legi::{clean_contenu, collect_titre_tm, date_or_none, find_anywhere};
use lj_core::error::CoreError;
use lj_core::parsing::{build_tree, node_text};

/// Une version d'article du fond JORF (`JORFARTI*.xml`, racine `<ARTICLE>`).
/// Calque [`crate::legi::LegiArticle`] mais `num`/`num_key`/`date_debut` sont
/// optionnels (cf. en-tête module). `jorftext` = cid `CONTEXTE/TEXTE@cid` (le
/// texte JORF parent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfArticle {
    pub jorfarti: String,
    pub jorftext: String,
    pub num: Option<String>,
    pub num_key: Option<String>,
    pub titre_text: Option<String>,
    pub etat: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
    pub texte: Option<String>,
    pub nota: Option<String>,
}

/// Un lien `META_TEXTE_VERSION/LIENS/LIEN` : graphe inter-textes JORF. Pour les
/// avenants d'un accord, `typelien ∈ {MODIFIE, MODIFICATION}` relie l'avenant à
/// l'accord initial. `date_signa` = sentinelle absorbée en `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfLien {
    pub cid: String,
    pub typelien: String,
    pub sens: String,
    pub date_signa: Option<String>,
    pub num_texte: Option<String>,
    pub libelle: Option<String>,
    /// `naturetexte` de la cible (DECRET, LOI, CODE…).
    pub nature: Option<String>,
    /// `num` : numéro d'article ciblé (liens CITATION vers un article de code).
    pub num: Option<String>,
    /// `id` : uid DILA de l'élément ciblé (texte ou article, ex. `LEGIARTI…`).
    pub target_id: Option<String>,
}

/// Un texte/version du fond JORF (`JORFTEXT*.xml`, racine `<TEXTE_VERSION>`). Le
/// corps vit dans les `JORFARTI` liés (pas ici) ; ce nœud porte les métadonnées.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfTexte {
    pub jorftext: String,
    pub titre: String,
    pub titre_full: Option<String>,
    pub nature: String,
    /// Numéro propre de l'acte (`META_TEXTE_CHRONICLE/NUM`, ex. `65-557`), absent pour
    /// les actes non numérotés (arrêté/décret « du X »). Brique d'`instrument_key`.
    pub num: Option<String>,
    /// NOR (`META_TEXTE_CHRONICLE/NOR`) — identifiant cross-diffuseur (même NOR
    /// LEGI↔JORF) ; workhorse du collapse d'identité.
    pub nor: Option<String>,
    /// ELI (`META_COMMUN/ID_ELI`) — identifiant autoritaire quand présent.
    pub eli: Option<String>,
    pub date_texte: Option<String>,
    pub date_publi: Option<String>,
    pub liens: Vec<JorfLien>,
}

/// Parse un `JORFARTI*.xml` (racine `<ARTICLE>`) en [`JorfArticle`]. Erreur
/// franche ([`CoreError::Xml`]) si `ID` ou `cid` manquent ; `NUM`/`DATE_DEBUT`
/// absents sont tolérés (`None`) — divergence JORF vs LEGI.
pub fn parse_jorf_article(raw: &[u8]) -> Result<JorfArticle, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("ARTICLE JORF: XML illisible".into()))?;

    let jorfarti = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("ARTICLE JORF: META_COMMUN/ID manquant".into()))?;
    let texte_node = find_anywhere(&root, "CONTEXTE/TEXTE");
    let jorftext = texte_node
        .and_then(|t| t.attr("cid"))
        .ok_or_else(|| {
            CoreError::Xml(format!(
                "ARTICLE JORF {jorfarti}: CONTEXTE/TEXTE@cid manquant"
            ))
        })?
        .to_string();

    let num = node_text(find_anywhere(&root, "META_ARTICLE/NUM"));
    let num_key = num.as_deref().map(lj_core::article_key::identity_key);
    let etat = node_text(find_anywhere(&root, "META_ARTICLE/ETAT")).unwrap_or_default();
    // Le fonds JORF ne date pas ses articles : `META_ARTICLE/DATE_DEBUT` et
    // `DATE_FIN` portent la sentinelle. Une entrée de journal est datée par sa
    // parution et n'a pas de fin de validité.
    let date_debut = date_or_none(node_text(find_anywhere(&root, "META_ARTICLE/DATE_DEBUT")))
        .or_else(|| {
            date_or_none(
                texte_node
                    .and_then(|t| t.attr("date_publi"))
                    .map(str::to_string),
            )
        });
    let date_fin = date_or_none(node_text(find_anywhere(&root, "META_ARTICLE/DATE_FIN")));
    let titre_text = find_anywhere(&root, "CONTEXTE").and_then(collect_titre_tm);

    Ok(JorfArticle {
        jorfarti,
        jorftext,
        num,
        num_key,
        titre_text,
        etat,
        date_debut,
        date_fin,
        texte: clean_contenu(find_anywhere(&root, "BLOC_TEXTUEL/CONTENU")),
        nota: clean_contenu(find_anywhere(&root, "NOTA/CONTENU")),
    })
}

/// Parse un `JORFTEXT*.xml` (racine `<TEXTE_VERSION>`) en [`JorfTexte`]. Erreur
/// franche si `META_COMMUN/ID` manque, ou si `META_TEXTE_VERSION/TITRE` **et**
/// `TITREFULL` manquent tous deux — le stock porte des `<TITRE/>` vides avec
/// un `TITREFULL` rempli, le titre long fait foi.
pub fn parse_jorf_texte(raw: &[u8]) -> Result<JorfTexte, CoreError> {
    let root = build_tree(raw)
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION JORF: XML illisible".into()))?;

    let jorftext = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION JORF: META_COMMUN/ID manquant".into()))?;
    let titre = node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITRE"))
        .or_else(|| node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITREFULL")))
        .ok_or_else(|| {
            CoreError::Xml(format!(
                "TEXTE_VERSION JORF {jorftext}: META_TEXTE_VERSION/TITRE et TITREFULL manquants"
            ))
        })?;

    Ok(JorfTexte {
        jorftext,
        titre,
        titre_full: node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITREFULL")),
        nature: node_text(find_anywhere(&root, "META_COMMUN/NATURE")).unwrap_or_default(),
        num: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/NUM")),
        nor: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/NOR")),
        eli: node_text(find_anywhere(&root, "META_COMMUN/ID_ELI")),
        date_texte: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/DATE_TEXTE")),
        date_publi: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/DATE_PUBLI")),
        liens: collect_liens(&root),
    })
}

/// Liens `META_TEXTE_VERSION/LIENS/LIEN` (plusieurs) avec leurs attributs.
fn collect_liens(root: &lj_core::parsing::XmlNode) -> Vec<JorfLien> {
    find_anywhere(root, "LIENS")
        .map(|n| {
            n.children
                .iter()
                .filter(|c| c.tag == "LIEN")
                .map(|c| JorfLien {
                    cid: c.attr("cidtexte").unwrap_or_default().to_string(),
                    typelien: c.attr("typelien").unwrap_or_default().to_string(),
                    sens: c.attr("sens").unwrap_or_default().to_string(),
                    date_signa: date_or_none(c.attr("datesignatexte").map(str::to_string)),
                    num_texte: c
                        .attr("numtexte")
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    libelle: c.text(),
                    nature: c
                        .attr("naturetexte")
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    num: c.attr("num").filter(|s| !s.is_empty()).map(str::to_string),
                    target_id: c.attr("id").filter(|s| !s.is_empty()).map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// L'ordre de lecture d'un `texte/struct/JORFTEXT*.xml` (racine `<TEXTELR>`) :
/// le cid chronique et ses `STRUCT/LIEN_ART@id` dans l'ordre du fichier — qui
/// est l'ordre du document au JO (les ids `JORFARTI`, eux, ne suivent PAS cet
/// ordre). Tous les liens sont rendus, numérotés ou non : ce parseur sert
/// l'assemblage du corps, pas le référentiel citable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfStructOrder {
    pub jorftext: String,
    pub article_ids: Vec<String>,
}

/// Parse un `texte/struct/JORFTEXT*.xml` en [`JorfStructOrder`]. Erreur franche
/// si le CID chronique manque ; `STRUCT` vide ou absente → liste vide (texte
/// pré-numérisation, contenu en fac-similé seulement).
pub fn parse_jorf_struct(raw: &[u8]) -> Result<JorfStructOrder, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("TEXTELR JORF: XML illisible".into()))?;
    let jorftext = node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/CID"))
        .ok_or_else(|| CoreError::Xml("TEXTELR JORF: META_TEXTE_CHRONICLE/CID manquant".into()))?;
    let article_ids = find_anywhere(&root, "STRUCT")
        .map(|s| {
            s.children
                .iter()
                .filter(|c| c.tag == "LIEN_ART")
                .filter_map(|c| c.attr("id").filter(|v| !v.is_empty()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(JorfStructOrder {
        jorftext,
        article_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // TEXTE_VERSION réel : décret 69-243 publiant l'accord franco-algérien 1968.
    // MC « ACCORD INTERNATIONAL » + LIENS vers les avenants (typelien
    // MODIFIE/MODIFICATION).
    const ACCORD_ALGERIEN: &str = r#"<TEXTE_VERSION>
<META>
<META_COMMUN><ID>JORFTEXT000000694290</ID><ORIGINE>JORF</ORIGINE><NATURE>DECRET</NATURE></META_COMMUN>
<META_SPEC>
<META_TEXTE_CHRONICLE><CID>JORFTEXT000000694290</CID><NUM>69-243</NUM><DATE_PUBLI>1969-03-22</DATE_PUBLI><DATE_TEXTE>1969-03-18</DATE_TEXTE></META_TEXTE_CHRONICLE>
<META_TEXTE_VERSION>
<TITRE>Décret n° 69-243 du 18 mars 1969</TITRE>
<TITREFULL>Décret n° 69-243 du 18 mars 1969 portant publication de l'accord entre le Gouvernement de la République française et le Gouvernement de la République Démocratique et populaire algérienne, relatif à la circulation, à l'emploi et au séjour en France des ressortissants algériens et de leurs familles, signé à Alger le 27 décembre 1968</TITREFULL>
<DATE_DEBUT/><DATE_FIN/>
<MCS_TXT><MC>ACCORD INTERNATIONAL</MC><MC>FRANCE</MC><MC>ALGERIE</MC><MC>ACCORDS DE 1968</MC></MCS_TXT>
<LIENS>
<LIEN cidtexte="JORFTEXT000000333424" datesignatexte="1986-03-07" id="JORFTEXT000000333424" naturetexte="DECRET" num="" numtexte="86-320" sens="cible" typelien="MODIFIE">Décret n°86-320 du 7 mars 1986</LIEN>
<LIEN cidtexte="JORFTEXT000000599731" datesignatexte="2002-12-20" id="JORFTEXT000000599731" naturetexte="DECRET" num="" numtexte="2002-1500" sens="source" typelien="MODIFICATION">Décret n° 2002-1500 du 20 décembre 2002, v. init.</LIEN>
<LIEN cidtexte="LEGITEXT000006069577" datesignatexte="2999-01-01" id="LEGIARTI000006311152" naturetexte="CODE" num="948" numtexte="" sens="cible" typelien="CITATION">CGI - art. 948</LIEN>
</LIENS>
</META_TEXTE_VERSION>
</META_SPEC>
</META>
<NOTICE><CONTENU/></NOTICE>
</TEXTE_VERSION>"#;

    #[test]
    fn parse_texte_accord_maps_metadata_and_links() {
        let t = parse_jorf_texte(ACCORD_ALGERIEN.as_bytes()).expect("texte");
        assert_eq!(t.jorftext, "JORFTEXT000000694290");
        assert_eq!(t.nature, "DECRET");
        assert_eq!(t.date_publi.as_deref(), Some("1969-03-22"));
        assert_eq!(t.date_texte.as_deref(), Some("1969-03-18"));
        assert!(t
            .titre_full
            .as_deref()
            .unwrap()
            .contains("27 décembre 1968"));
        assert_eq!(t.liens.len(), 3);
        assert_eq!(t.liens[0].typelien, "MODIFIE");
        assert_eq!(t.liens[0].cid, "JORFTEXT000000333424");
        assert_eq!(t.liens[0].num_texte.as_deref(), Some("86-320"));
        assert_eq!(t.liens[0].date_signa.as_deref(), Some("1986-03-07"));
        assert_eq!(t.liens[1].typelien, "MODIFICATION");
        assert_eq!(t.liens[2].typelien, "CITATION");
        assert_eq!(t.liens[2].date_signa, None);
    }

    // TEXTE_VERSION réel : délibération CSA 2018-11. Le `META_COMMUN` du fond JORF
    // ne porte que ID/ANCIEN_ID/ORIGINE/URL/NATURE ; NOR vit sous
    // `META_TEXTE_CHRONICLE`, comme NUM et les dates.
    const DELIBERATION_CSA: &str = r#"<TEXTE_VERSION>
<META>
<META_COMMUN><ID>JORFTEXT000036889522</ID><ANCIEN_ID/><ORIGINE>JORF</ORIGINE><URL>texte/version/JORF/TEXT/00/00/36/88/95/JORFTEXT000036889522.xml</URL><NATURE>DELIBERATION</NATURE></META_COMMUN>
<META_SPEC>
<META_TEXTE_CHRONICLE><CID>JORFTEXT000036889522</CID><NUM>2018-11</NUM><NOR>CSAC1812426X</NOR><DATE_PUBLI>2018-05-10</DATE_PUBLI><DATE_TEXTE>2018-04-18</DATE_TEXTE></META_TEXTE_CHRONICLE>
<META_TEXTE_VERSION>
<TITRE>Délibération n°2018-11 du 18 avril 2018</TITRE>
<TITREFULL>Délibération n° 2018-11 du 18 avril 2018 du Conseil supérieur de l'audiovisuel relative à l'honnêteté et à l'indépendance de l'information et des programmes qui y concourent</TITREFULL>
</META_TEXTE_VERSION>
</META_SPEC>
</META>
</TEXTE_VERSION>"#;

    #[test]
    fn parse_texte_lit_le_nor_sous_la_chronique() {
        let t = parse_jorf_texte(DELIBERATION_CSA.as_bytes()).expect("texte");
        assert_eq!(t.nor.as_deref(), Some("CSAC1812426X"));
        assert_eq!(t.num.as_deref(), Some("2018-11"));
    }

    // TEXTE_VERSION réel : accord de coopération T2S (JORFTEXT000033243907),
    // `<TITRE/>` vide mais TITREFULL rempli.
    const ACCORD_SANS_TITRE: &str = r#"<TEXTE_VERSION>
<META>
<META_COMMUN><ID>JORFTEXT000033243907</ID><ORIGINE>JORF</ORIGINE><NATURE/></META_COMMUN>
<META_SPEC>
<META_TEXTE_CHRONICLE><CID>JORFTEXT000033243907</CID><NUM/><NOR>AMFX1627336X</NOR><DATE_PUBLI>2016-10-16</DATE_PUBLI><DATE_TEXTE>2999-01-01</DATE_TEXTE></META_TEXTE_CHRONICLE>
<META_TEXTE_VERSION>
<TITRE/>
<TITREFULL>Accord de coopération relatif à T2S entre surveillants et autorités compétentes pour la supervision des dépositaires centraux de titres</TITREFULL>
</META_TEXTE_VERSION>
</META_SPEC>
</META>
</TEXTE_VERSION>"#;

    #[test]
    fn parse_texte_titre_vide_replie_sur_titrefull() {
        let t = parse_jorf_texte(ACCORD_SANS_TITRE.as_bytes()).expect("texte");
        assert!(t.titre.starts_with("Accord de coopération relatif à T2S"));
    }

    // JORFARTI réel : annonce « accès protégé » à NUM vide (TYPE=AUTONOME),
    // DATE_DEBUT/FIN sentinelles, corps présent.
    const ARTICLE_ANNONCE: &str = r#"<ARTICLE>
<META>
<META_COMMUN><ID>JORFARTI000051887074</ID><ORIGINE>JORF</ORIGINE><NATURE>Article</NATURE></META_COMMUN>
<META_SPEC><META_ARTICLE><NUM/><DATE_DEBUT>2999-01-01</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN><TYPE>AUTONOME</TYPE></META_ARTICLE></META_SPEC>
</META>
<CONTEXTE><TEXTE cid="JORFTEXT000051887073" nature="ANNONCES"><TITRE_TXT>Demandes de changement de nom</TITRE_TXT></TEXTE></CONTEXTE>
<BLOC_TEXTUEL><CONTENU><p>Les actes individuels relatifs à l'état des personnes sont accessibles sur Légifrance.</p></CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;

    #[test]
    fn parse_article_tolerates_missing_num_and_sentinel_dates() {
        let a = parse_jorf_article(ARTICLE_ANNONCE.as_bytes()).expect("article");
        assert_eq!(a.jorfarti, "JORFARTI000051887074");
        assert_eq!(a.jorftext, "JORFTEXT000051887073");
        assert_eq!(a.num, None);
        assert_eq!(a.num_key, None);
        assert_eq!(a.date_debut, None);
        assert_eq!(a.date_fin, None);
        assert_eq!(
            a.texte.as_deref(),
            Some("Les actes individuels relatifs à l'état des personnes sont accessibles sur Légifrance.")
        );
    }

    #[test]
    fn parse_article_with_num_normalizes_key() {
        let xml = r#"<ARTICLE>
<META><META_COMMUN><ID>JORFARTI000000694291</ID></META_COMMUN>
<META_SPEC><META_ARTICLE><NUM>6</NUM><ETAT>VIGUEUR</ETAT><DATE_DEBUT>2002-08-09</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN></META_ARTICLE></META_SPEC></META>
<CONTEXTE><TEXTE cid="JORFTEXT000000694290"/></CONTEXTE>
<BLOC_TEXTUEL><CONTENU>Les ressortissants algériens bénéficient de plein droit.</CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;
        let a = parse_jorf_article(xml.as_bytes()).expect("article");
        assert_eq!(a.num.as_deref(), Some("6"));
        assert_eq!(a.num_key.as_deref(), Some("6"));
        assert_eq!(a.etat, "VIGUEUR");
        assert_eq!(a.date_debut.as_deref(), Some("2002-08-09"));
        assert_eq!(a.date_fin, None);
    }

    #[test]
    fn parse_article_dates_from_publication() {
        // JORFARTI réel (délibération CSA 2018-11, art. 1er) : DATE_DEBUT et DATE_FIN
        // sentinelles, ETAT absent. La parution portée par CONTEXTE/TEXTE@date_publi
        // date l'entrée.
        let xml = r#"<ARTICLE>
<META><META_COMMUN><ID>JORFARTI000036889523</ID></META_COMMUN>
<META_SPEC><META_ARTICLE><NUM>1</NUM><DATE_DEBUT>2999-01-01</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN></META_ARTICLE></META_SPEC></META>
<CONTEXTE><TEXTE cid="JORFTEXT000036889522" date_publi="2018-04-20" date_signature="2018-04-18" nature="DELIBERATION" nor="CSAC1812426X"/></CONTEXTE>
<BLOC_TEXTUEL><CONTENU>Les éditeurs de services veillent à l'honnêteté de l'information.</CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;
        let a = parse_jorf_article(xml.as_bytes()).expect("article");
        assert_eq!(a.date_debut.as_deref(), Some("2018-04-20"));
        assert_eq!(a.date_fin, None);
        assert_eq!(a.etat, "");
    }

    #[test]
    fn parse_struct_keeps_document_order_including_unnumbered() {
        // Extrait réel (JORFTEXT000000209787) : les LIEN_ART sans num sont rendus dans
        // l'ordre du fichier.
        let xml = r#"<TEXTELR>
<META><META_SPEC><META_TEXTE_CHRONICLE><CID>JORFTEXT000000209787</CID></META_TEXTE_CHRONICLE></META_SPEC></META>
<STRUCT>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000001045162" num="" origine="JORF"/>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000002207442" num="" origine="JORF"/>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000001817636" num="Annexe" origine="JORF"/>
</STRUCT>
</TEXTELR>"#;
        let s = parse_jorf_struct(xml.as_bytes()).expect("struct");
        assert_eq!(s.jorftext, "JORFTEXT000000209787");
        assert_eq!(
            s.article_ids,
            vec![
                "JORFARTI000001045162",
                "JORFARTI000002207442",
                "JORFARTI000001817636"
            ]
        );
    }

    #[test]
    fn parse_struct_empty_is_prenumerisation() {
        // STRUCT vide (traité pré-numérisation, ex. décret 76-963) → liste vide.
        let xml = r#"<TEXTELR>
<META><META_SPEC><META_TEXTE_CHRONICLE><CID>JORFTEXT000000307098</CID></META_TEXTE_CHRONICLE></META_SPEC></META>
<STRUCT/>
</TEXTELR>"#;
        let s = parse_jorf_struct(xml.as_bytes()).expect("struct");
        assert!(s.article_ids.is_empty());
    }

    #[test]
    fn article_missing_id_is_hard_error() {
        let xml = br#"<ARTICLE><META_SPEC><META_ARTICLE><NUM>1</NUM></META_ARTICLE></META_SPEC>
<CONTEXTE><TEXTE cid="JORFTEXT000000000001"/></CONTEXTE></ARTICLE>"#;
        assert!(matches!(parse_jorf_article(xml), Err(CoreError::Xml(_))));
    }

    #[test]
    fn article_missing_cid_is_hard_error() {
        let xml = br#"<ARTICLE><META_COMMUN><ID>JORFARTI000000000001</ID></META_COMMUN>
<CONTEXTE><TEXTE/></CONTEXTE></ARTICLE>"#;
        assert!(matches!(parse_jorf_article(xml), Err(CoreError::Xml(_))));
    }
}
