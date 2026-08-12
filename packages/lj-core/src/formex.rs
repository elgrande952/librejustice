//! Parser Formex 4 (actes EUR-Lex servis par CELLAR).
//!
//! Extrait les articles du dispositif d'un `<ACT>` ou `<CONS.ACT>` : numéro
//! (`<TI.ART>`), sous-titre (`<STI.ART>`), chemin des divisions englobantes
//! (`<DIVISION><TITLE>`, segments joints par ` > ` comme `title_path` LEGI) et
//! corps à blocs séparés par sauts de ligne. Une passe streaming quick-xml :
//! séparateurs de blocs et guillemets `<QUOT.START CODE=…/>` (éléments vides à
//! code hexadécimal Unicode) dépendent de l'entrelacement texte/enfants.
//!
//! Hors périmètre, ignoré : préambule, considérants, signatures, notes de bas
//! de page (`<NOTE>`), annexes (`<ANNEX>`/`<CONS.ANNEX>`). Un `<ARTICLE>`
//! imbriqué dans un autre (acte modificatif citant le texte qu'il insère) reste
//! du corps de l'article englobant, jamais un article distinct.

use quick_xml::events::Event;
use quick_xml::Reader;

/// Un article du dispositif.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormexArticle {
    /// Numéro d'affichage : « 1er », « 2 », « 44 bis », « unique ».
    pub num: String,
    /// Sous-titre `<STI.ART>` (« Définitions »), si présent.
    pub subtitle: Option<String>,
    /// Divisions englobantes, segments ` > ` (`TI : STI`), `None` hors division.
    pub title_path: Option<String>,
    /// Corps : un bloc (alinéa, point, ligne de tableau) par ligne.
    pub texte: String,
}

/// Blocs dont la fermeture termine une ligne du corps.
const BLOCK_END: &[&str] = &[
    "P", "ALINEA", "NP", "ITEM", "PARAG", "ROW", "LIST", "TBL", "TXT",
];

/// Sous-arbres entièrement ignorés.
const SKIP: &[&str] = &["NOTE", "TOC", "ANNEX", "CONS.ANNEX", "BIB.INSTANCE"];

/// Cible du texte courant pendant la passe.
#[derive(PartialEq)]
enum Sink {
    /// Hors de tout capteur (préambule, biblio…).
    Void,
    /// `<TI.ART>` de l'article de tête.
    Num,
    /// `<STI.ART>` de l'article de tête.
    Subtitle,
    /// Corps de l'article de tête (y compris articles cités imbriqués).
    Body,
    /// `<TI>` du `<TITLE>` d'une division.
    DivTi,
    /// `<STI>` du `<TITLE>` d'une division.
    DivSti,
}

/// Étiquette d'une division en cours (`TI : STI` une fois complète).
#[derive(Default)]
struct Division {
    ti: String,
    sti: String,
}

impl Division {
    fn label(&self) -> Option<String> {
        let ti = collapse_ws(&self.ti);
        let sti = collapse_ws(&self.sti);
        match (ti.is_empty(), sti.is_empty()) {
            (true, true) => None,
            (false, true) => Some(ti),
            (true, false) => Some(sti),
            (false, false) => Some(format!("{ti} : {sti}")),
        }
    }
}

/// Article en cours d'accumulation.
#[derive(Default)]
struct ArticleAcc {
    identifier: Option<String>,
    num: String,
    subtitle: String,
    body: String,
    title_path: Option<String>,
    /// Profondeur d'`<ARTICLE>` imbriqués (0 = article de tête).
    nesting: usize,
}

/// Blancs internes repliés en espaces simples, bornes trimées.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Numéro d'affichage depuis `<TI.ART>` (« Article premier » → « 1er »,
/// « Article\u{a0}44 bis » → « 44 bis ») ; repli sur `IDENTIFIER` sans zéros
/// de tête (« 001 » → « 1 »).
fn clean_num(ti_art: &str, identifier: Option<&str>) -> String {
    let flat = collapse_ws(&ti_art.replace('\u{a0}', " "));
    let stripped = flat
        .strip_prefix("Article")
        .or_else(|| flat.strip_prefix("ARTICLE"))
        .unwrap_or(&flat)
        .trim();
    let num = if stripped.eq_ignore_ascii_case("premier") {
        "1er".to_string()
    } else {
        stripped.to_string()
    };
    if !num.is_empty() {
        return num;
    }
    let ident = identifier.unwrap_or_default();
    let trimmed = ident.trim_start_matches('0');
    if trimmed.is_empty() { ident } else { trimmed }.to_string()
}

/// Décode le texte d'un event quick-xml (decode + unescape des entités).
fn event_text(e: &quick_xml::events::BytesText) -> String {
    match e.decode() {
        Ok(d) => quick_xml::escape::unescape(&d)
            .map(|u| u.into_owned())
            .unwrap_or_else(|_| d.into_owned()),
        Err(_) => String::from_utf8_lossy(e.as_ref()).into_owned(),
    }
}

/// La balise qui vient d'être empilée est-elle enfant direct du `<TITLE>`
/// d'une `<DIVISION>` ? (`stack` finit par `…, DIVISION, TITLE, tag`.)
fn under_division_title(stack: &[String]) -> bool {
    let n = stack.len();
    n >= 3 && stack[n - 2] == "TITLE" && stack[n - 3] == "DIVISION"
}

/// Nom local d'une balise (`ns:Tag` → `Tag`).
fn local(qname: &[u8]) -> String {
    let s = String::from_utf8_lossy(qname);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// Valeur d'attribut d'un élément ouvrant/vide.
fn attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes().filter_map(Result::ok).find_map(|a| {
        (local(a.key.as_ref()) == name).then(|| String::from_utf8_lossy(&a.value).into_owned())
    })
}

/// Caractère d'un marqueur `<QUOT.START/END CODE="00AB"/>` (hexadécimal Unicode).
fn quot_char(code: &str) -> Option<char> {
    u32::from_str_radix(code, 16).ok().and_then(char::from_u32)
}

/// Corps accumulé → lignes repliées ([`collapse_ws`]) non vides jointes par
/// `\n`.
fn finish_body(raw: &str) -> String {
    raw.lines()
        .map(collapse_ws)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extrait les articles du dispositif d'un acte Formex (`ACT`/`CONS.ACT`).
/// Tolérant au XML dégradé : la passe s'arrête à l'EOF ou à la première erreur
/// de lexing et rend ce qu'elle a.
pub fn parse_formex_articles(xml: &str) -> Vec<FormexArticle> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = false;

    let mut out: Vec<FormexArticle> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut skip_below: Option<usize> = None;
    let mut divisions: Vec<Division> = Vec::new();
    let mut article: Option<ArticleAcc> = None;
    let mut sink = Sink::Void;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let tag = local(e.name().as_ref());
                stack.push(tag.clone());
                if skip_below.is_some() {
                    buf.clear();
                    continue;
                }
                if SKIP.contains(&tag.as_str()) {
                    skip_below = Some(stack.len());
                    buf.clear();
                    continue;
                }
                match (tag.as_str(), &mut article) {
                    ("ARTICLE", Some(acc)) => acc.nesting += 1,
                    ("ARTICLE", None) => {
                        article = Some(ArticleAcc {
                            identifier: attr(&e, "IDENTIFIER"),
                            title_path: {
                                let labels: Vec<String> =
                                    divisions.iter().filter_map(Division::label).collect();
                                (!labels.is_empty()).then(|| labels.join(" > "))
                            },
                            ..Default::default()
                        });
                        sink = Sink::Body;
                    }
                    ("TI.ART", Some(acc)) if acc.nesting == 0 => sink = Sink::Num,
                    ("STI.ART", Some(acc)) if acc.nesting == 0 => sink = Sink::Subtitle,
                    ("DIVISION", None) => divisions.push(Division::default()),
                    ("TI", None) if under_division_title(&stack) => sink = Sink::DivTi,
                    ("STI", None) if under_division_title(&stack) => sink = Sink::DivSti,
                    ("CELL", Some(acc)) if sink == Sink::Body => {
                        // Séparateur de colonnes, sauf en tête de ligne (la
                        // ligne courante peut ne porter que des blancs).
                        let line = acc.body.rsplit('\n').next().unwrap_or("");
                        if !line.trim().is_empty() {
                            acc.body.push_str(" | ");
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                if skip_below.is_some() {
                    buf.clear();
                    continue;
                }
                let tag = local(e.name().as_ref());
                if (tag == "QUOT.START" || tag == "QUOT.END") && sink == Sink::Body {
                    if let Some(c) = attr(&e, "CODE").as_deref().and_then(quot_char) {
                        if let Some(acc) = &mut article {
                            acc.body.push(c);
                        }
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if skip_below.is_some() {
                    buf.clear();
                    continue;
                }
                // La structure en lignes vient des fins de blocs, jamais de la mise en
                // forme du XML source.
                let txt = event_text(&e).replace(['\n', '\r', '\t'], " ");
                if txt.is_empty() {
                    buf.clear();
                    continue;
                }
                match (&sink, &mut article, divisions.last_mut()) {
                    (Sink::Num, Some(acc), _) => acc.num.push_str(&txt),
                    (Sink::Subtitle, Some(acc), _) => acc.subtitle.push_str(&txt),
                    (Sink::Body, Some(acc), _) => acc.body.push_str(&txt),
                    (Sink::DivTi, _, Some(div)) => div.ti.push_str(&txt),
                    (Sink::DivSti, _, Some(div)) => div.sti.push_str(&txt),
                    _ => {}
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if skip_below.is_some() {
                    buf.clear();
                    continue;
                }
                // Entités hors des `Text` voisins (quick-xml 0.40).
                let txt = match e.decode() {
                    Ok(content) => quick_xml::escape::unescape(&format!("&{content};"))
                        .map(|u| u.into_owned())
                        .unwrap_or_else(|_| format!("&{content};")),
                    Err(_) => String::from_utf8_lossy(e.as_ref()).into_owned(),
                };
                match (&sink, &mut article, divisions.last_mut()) {
                    (Sink::Num, Some(acc), _) => acc.num.push_str(&txt),
                    (Sink::Subtitle, Some(acc), _) => acc.subtitle.push_str(&txt),
                    (Sink::Body, Some(acc), _) => acc.body.push_str(&txt),
                    (Sink::DivTi, _, Some(div)) => div.ti.push_str(&txt),
                    (Sink::DivSti, _, Some(div)) => div.sti.push_str(&txt),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => {
                let tag = stack.pop().unwrap_or_default();
                if let Some(depth) = skip_below {
                    if stack.len() < depth {
                        skip_below = None;
                    }
                    buf.clear();
                    continue;
                }
                match tag.as_str() {
                    "ARTICLE" => match &mut article {
                        Some(acc) if acc.nesting > 0 => acc.nesting -= 1,
                        Some(_) => {
                            let acc = article.take().expect("article de tête présent");
                            let subtitle = collapse_ws(&acc.subtitle);
                            out.push(FormexArticle {
                                num: clean_num(&acc.num, acc.identifier.as_deref()),
                                subtitle: (!subtitle.is_empty()).then_some(subtitle),
                                title_path: acc.title_path,
                                texte: finish_body(&acc.body),
                            });
                            sink = Sink::Void;
                        }
                        None => {}
                    },
                    "TI.ART" | "STI.ART" => match &mut article {
                        Some(acc) if acc.nesting == 0 => sink = Sink::Body,
                        // Titre d'un article cité imbriqué : il vit dans le
                        // corps, sa fin termine une ligne.
                        Some(acc) if sink == Sink::Body => acc.body.push('\n'),
                        _ => {}
                    },
                    "DIVISION" => {
                        divisions.pop();
                    }
                    "TI" | "STI" => {
                        if matches!(sink, Sink::DivTi | Sink::DivSti) {
                            sink = Sink::Void;
                        }
                    }
                    "NO.P" | "NO.PARAG" => {
                        if let (Sink::Body, Some(acc)) = (&sink, &mut article) {
                            acc.body.push(' ');
                        }
                    }
                    t if BLOCK_END.contains(&t) => {
                        if let (Sink::Body, Some(acc)) = (&sink, &mut article) {
                            acc.body.push('\n');
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture condensée d'un `CONS.ACT` réel : divisions imbriquées,
    /// paragraphes numérotés, liste à points, tableau, guillemets `QUOT.*`, note de
    /// bas de page, article cité imbriqué, annexe.
    const FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<CONS.ACT>
<ENACTING.TERMS>
<DIVISION><TITLE><TI><P>TITRE PREMIER</P></TI><STI><P>OBJET</P></STI></TITLE>
<ARTICLE IDENTIFIER="001"><TI.ART>Article premier</TI.ART>
<PARAG IDENTIFIER="001.001"><NO.PARAG>1.</NO.PARAG>
<ALINEA>La pr&#233;sente directive &#233;tablit le syst&#232;me commun de TVA.</ALINEA></PARAG>
<PARAG IDENTIFIER="001.002"><NO.PARAG>2.</NO.PARAG>
<ALINEA><P>Premier alin&#233;a.</P><P>Second alin&#233;a<NOTE NOTE.ID="E1" TYPE="FOOTNOTE"><P>JO C 220</P></NOTE>.</P></ALINEA></PARAG></ARTICLE>
<DIVISION><TITLE><TI><P>CHAPITRE 1</P></TI></TITLE>
<ARTICLE IDENTIFIER="002"><TI.ART>Article&#160;2</TI.ART><STI.ART>D&#233;finitions</STI.ART>
<ALINEA>Aux fins du pr&#233;sent acte, on entend:</ALINEA>
<LIST TYPE="alpha"><ITEM><NP><NO.P>a)</NO.P><TXT>premier point;</TXT></NP></ITEM>
<ITEM><NP><NO.P>b)</NO.P><TXT>second point.</TXT></NP></ITEM></LIST>
<ALINEA><TBL COLS="2"><GR.SEQ><ROW TYPE="HEADER"><CELL COL="1">Code NC</CELL><CELL COL="2">Biens</CELL></ROW>
<ROW><CELL COL="1">2716 00 00</CELL><CELL COL="2">&#201;lectricit&#233;</CELL></ROW></GR.SEQ></TBL></ALINEA></ARTICLE>
<ARTICLE IDENTIFIER="003"><TI.ART>Article 2 bis</TI.ART>
<ALINEA>L'article suivant est ins&#233;r&#233;:
<QUOT.START CODE="00AB" ID="QS1" REF.END="QE1"/><ARTICLE IDENTIFIER="003.001"><TI.ART>Article 5 ter</TI.ART>
<ALINEA>Corps cit&#233;.</ALINEA></ARTICLE><QUOT.END CODE="00BB" ID="QE1" REF.START="QS1"/></ALINEA></ARTICLE>
</DIVISION></DIVISION>
</ENACTING.TERMS>
<CONS.ANNEX><ARTICLE IDENTIFIER="900"><TI.ART>Article annexe</TI.ART><ALINEA>hors dispositif</ALINEA></ARTICLE></CONS.ANNEX>
</CONS.ACT>"#;

    #[test]
    fn parse_formex_extrait_le_dispositif() {
        let arts = parse_formex_articles(FIXTURE);
        assert_eq!(
            arts.iter().map(|a| a.num.as_str()).collect::<Vec<_>>(),
            ["1er", "2", "2 bis"]
        );

        let a1 = &arts[0];
        assert_eq!(a1.title_path.as_deref(), Some("TITRE PREMIER : OBJET"));
        assert_eq!(
            a1.texte,
            "1. La présente directive établit le système commun de TVA.\n\
             2. Premier alinéa.\nSecond alinéa."
        );

        let a2 = &arts[1];
        assert_eq!(a2.subtitle.as_deref(), Some("Définitions"));
        assert_eq!(
            a2.title_path.as_deref(),
            Some("TITRE PREMIER : OBJET > CHAPITRE 1")
        );
        assert_eq!(
            a2.texte,
            "Aux fins du présent acte, on entend:\na) premier point;\n\
             b) second point.\nCode NC | Biens\n2716 00 00 | Électricité"
        );

        let a3 = &arts[2];
        assert_eq!(
            a3.texte,
            "L'article suivant est inséré: «Article 5 ter\nCorps cité.\n»"
        );
    }

    #[test]
    fn clean_num_couvre_les_formes_reelles() {
        assert_eq!(clean_num("Article premier", Some("001")), "1er");
        assert_eq!(clean_num("Article\u{a0}44 bis", None), "44 bis");
        assert_eq!(clean_num("Article unique", None), "unique");
        assert_eq!(clean_num("", Some("059")), "59");
    }
}
