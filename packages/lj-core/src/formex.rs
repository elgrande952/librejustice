//! Parser Formex 4 (actes EUR-Lex servis par CELLAR).
//!
//! Extrait les articles du dispositif d'un `<ACT>` ou `<CONS.ACT>` : numéro
//! (`<TI.ART>`), sous-titre (`<STI.ART>`), chemin des divisions englobantes
//! (`<DIVISION><TITLE>`, segments joints par ` > ` comme `title_path` LEGI) et
//! corps à blocs séparés par sauts de ligne. Une passe streaming quick-xml :
//! séparateurs de blocs et guillemets `<QUOT.START CODE=…/>` (éléments vides à
//! code hexadécimal Unicode) dépendent de l'entrelacement texte/enfants.
//!
//! Annexes come out in the same list, after the enacting terms, numbered from
//! their `<TITLE><TI>` ("ANNEXE I bis" -> "Annexe I bis"): the `<ANNEX>` volume
//! a base act publishes as a separate document and the `<CONS.ANNEX>` a
//! consolidated act carries inline. The same container holds the declarations
//! attached to the act, which carry no annex number and do not come out.
//!
//! Hors périmètre, ignoré : préambule, considérants, signatures, notes de bas
//! de page (`<NOTE>`). Un `<ARTICLE>` imbriqué dans un autre (acte modificatif
//! citant le texte qu'il insère, article d'un accord annexé) reste du corps de
//! l'article ou de l'annexe englobante, jamais un article distinct.

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
const SKIP: &[&str] = &["NOTE", "TOC", "BIB.INSTANCE"];

/// Annex containers: the standalone volume and the inline consolidated one.
const ANNEX: &[&str] = &["ANNEX", "CONS.ANNEX"];

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
    /// Depth of the container in the stack. An amending act replaces the title
    /// of a cited article with a bare `<TI.ART>` inside a `<QUOT.S>`: only a
    /// direct child of the container names it.
    depth: usize,
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

/// A token an annex number is made of: "I", "XIII", "2", "A", "bis". The roman
/// form stops at "IVX", because a caption word made of "IVXLCDM" is ordinary
/// French ("CIVIL") whereas one made of "IVX" is not.
fn annex_numeral(token: &str) -> bool {
    const SUFFIXES: &[&str] = &["bis", "ter", "quater", "quinquies", "sexies"];
    !token.is_empty()
        && (token.chars().all(|c| "IVX".contains(c))
            || token.chars().all(|c| c.is_ascii_digit())
            || (token.len() == 1 && token.starts_with(|c: char| c.is_ascii_uppercase()))
            || SUFFIXES.iter().any(|s| token.eq_ignore_ascii_case(s)))
}

/// Number of an annex read from its `<TI>`, and the caption that sometimes
/// follows it in the same run. `None` outside that shape, which leaves out the
/// declarations sharing the container. The number stops at the first token that
/// is not a numeral: it becomes a key, and a key carries no sentence.
fn annex_num(ti: &str) -> Option<(String, String)> {
    let flat = collapse_ws(&ti.replace('\u{a0}', " "));
    let mut tokens = flat.split(' ');
    if !tokens.next()?.eq_ignore_ascii_case("annexe") {
        return None;
    }
    let rest: Vec<&str> = tokens.collect();
    let taken = rest.iter().take_while(|t| annex_numeral(t)).count();
    let mut num = "Annexe".to_string();
    for token in &rest[..taken] {
        num.push(' ');
        num.push_str(token);
    }
    Some((num, rest[taken..].join(" ")))
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

/// Is a `<TI.ART>`/`<STI.ART>` pushed at `depth` the title of the open article,
/// or quoted text? An amending act replaces the title of a cited article with a
/// bare `<TI.ART>` inside a `<QUOT.S>`, which is not a child of any `<ARTICLE>`.
fn names_container(acc: &ArticleAcc, depth: usize) -> bool {
    acc.nesting == 0 && acc.depth + 1 == depth
}

/// Same question for the `<TITLE>` of the annex container opened at `annex`. An
/// appendix is itself an annex container, nested in the one it completes: same
/// shape, and its title must not feed the number of its parent.
fn under_annex_title(stack: &[String], annex: Option<usize>) -> bool {
    let n = stack.len();
    n >= 3
        && annex == Some(n - 2)
        && stack[n - 2] == "TITLE"
        && ANNEX.contains(&stack[n - 3].as_str())
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
    // Stack depth of the open annex container, `None` outside one.
    let mut annex: Option<usize> = None;
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
                if ANNEX.contains(&tag.as_str()) && article.is_none() {
                    annex = Some(stack.len());
                    article = Some(ArticleAcc {
                        depth: stack.len(),
                        ..Default::default()
                    });
                    sink = Sink::Body;
                    buf.clear();
                    continue;
                }
                match (tag.as_str(), &mut article) {
                    ("TI", Some(_)) if under_annex_title(&stack, annex) => sink = Sink::Num,
                    ("STI", Some(_)) if under_annex_title(&stack, annex) => sink = Sink::Subtitle,
                    ("ARTICLE", Some(acc)) => acc.nesting += 1,
                    ("ARTICLE", None) => {
                        article = Some(ArticleAcc {
                            identifier: attr(&e, "IDENTIFIER"),
                            title_path: {
                                let labels: Vec<String> =
                                    divisions.iter().filter_map(Division::label).collect();
                                (!labels.is_empty()).then(|| labels.join(" > "))
                            },
                            depth: stack.len(),
                            ..Default::default()
                        });
                        sink = Sink::Body;
                    }
                    ("TI.ART", Some(acc)) if names_container(acc, stack.len()) => sink = Sink::Num,
                    ("STI.ART", Some(acc)) if names_container(acc, stack.len()) => {
                        sink = Sink::Subtitle
                    }
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
                if annex == Some(stack.len() + 1) && ANNEX.contains(&tag.as_str()) {
                    let acc = article.take().expect("annexe ouverte, accumulateur posé");
                    if let Some((num, caption)) = annex_num(&acc.num) {
                        let subtitle = match collapse_ws(&acc.subtitle) {
                            s if s.is_empty() => caption,
                            s => s,
                        };
                        out.push(FormexArticle {
                            num,
                            subtitle: (!subtitle.is_empty()).then_some(subtitle),
                            title_path: None,
                            texte: finish_body(&acc.body),
                        });
                    }
                    annex = None;
                    sink = Sink::Void;
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
                        Some(acc) if names_container(acc, stack.len() + 1) => sink = Sink::Body,
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
                        } else if annex.is_some() {
                            sink = Sink::Body;
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
<ARTICLE IDENTIFIER="004"><TI.ART>Article 3</TI.ART>
<ALINEA>Le titre est remplac&#233; par le titre suivant:
<QUOT.S LEVEL="1"><TI.ART>Dispositions g&#233;n&#233;rales</TI.ART></QUOT.S></ALINEA>
<ALINEA>L'annexe suivante est ajout&#233;e:
<ANNEX><TITLE><TI><P>ANNEXE III</P></TI></TITLE><CONTENTS><ALINEA>Corps annex&#233; cit&#233;.</ALINEA></CONTENTS></ANNEX></ALINEA></ARTICLE>
</DIVISION></DIVISION>
</ENACTING.TERMS>
<CONS.ANNEX><TITLE><TI><P>ANNEXE I</P></TI><STI><P>&#201;QUIPEMENTS NON R&#201;GLEMENT&#201;S</P></STI></TITLE>
<CONTENTS><NP><NO.P>1.</NO.P><TXT>Les kits de composants.</TXT></NP>
<ARTICLE IDENTIFIER="900"><TI.ART>Article premier</TI.ART><ALINEA>Corps de l'accord annex&#233;.</ALINEA></ARTICLE></CONTENTS></CONS.ANNEX>
<CONS.ANNEX><TITLE><TI><P>ANNEXE II bis</P></TI></TITLE><CONTENTS><ALINEA>Tableau de correspondance.</ALINEA>
<CONS.ANNEX><TITLE><TI><P>Appendice 1 &#224; l'annexe II bis</P></TI><STI><P>M&#201;THODE D'ESSAI</P></STI></TITLE><CONTENTS><ALINEA>Corps de l'appendice.</ALINEA></CONTENTS></CONS.ANNEX></CONTENTS></CONS.ANNEX>
<CONS.ANNEX><TITLE><TI><P>D&#201;CLARATION DU PARLEMENT EUROP&#201;EN</P></TI></TITLE><CONTENTS><ALINEA>Sans num&#233;ro d'annexe.</ALINEA></CONTENTS></CONS.ANNEX>
</CONS.ACT>"#;

    #[test]
    fn parse_formex_extrait_le_dispositif() {
        let arts = parse_formex_articles(FIXTURE);
        assert_eq!(
            arts.iter().map(|a| a.num.as_str()).collect::<Vec<_>>(),
            ["1er", "2", "2 bis", "3", "Annexe I", "Annexe II bis"]
        );
        // What an amending article quotes stays in its body: leaving it, an
        // annex and a replacement title both carried its number away.
        assert_eq!(arts[3].num, "3");
        assert!(arts[3].texte.contains("Dispositions générales"));
        assert!(arts[3].texte.contains("ANNEXE III"));
        assert!(arts[3].texte.contains("Corps annexé cité."));

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

        let an1 = &arts[4];
        assert_eq!(an1.subtitle.as_deref(), Some("ÉQUIPEMENTS NON RÉGLEMENTÉS"));
        assert_eq!(an1.title_path, None);
        assert_eq!(
            an1.texte,
            "1. Les kits de composants.\nArticle premier\nCorps de l'accord annexé."
        );
        // An appendix is an annex container nested in the annex it completes:
        // it stays in its body, and its title is not the parent's number.
        let an2 = &arts[5];
        assert_eq!(an2.subtitle, None);
        assert_eq!(
            an2.texte,
            "Tableau de correspondance.\nAppendice 1 à l'annexe II bis\n\
             MÉTHODE D'ESSAI\nCorps de l'appendice."
        );
    }

    #[test]
    fn annex_num_ecarte_les_declarations_jointes() {
        let num = annex_num;
        assert_eq!(num("ANNEXE"), Some(("Annexe".into(), String::new())));
        assert_eq!(
            num("ANNEXE\u{a0}I bis"),
            Some(("Annexe I bis".into(), String::new()))
        );
        assert_eq!(num("Annexe  II"), Some(("Annexe II".into(), String::new())));
        assert_eq!(num("DÉCLARATION DU PARLEMENT EUROPÉEN"), None);
        assert_eq!(num(""), None);
        // The `<TI>` of an annex often runs the number and the caption together,
        // and the caption is a whole page: only the number becomes a key.
        assert_eq!(
            num("ANNEXE XIII APPENDICE 1 AU CERTIFICAT"),
            Some(("Annexe XIII".into(), "APPENDICE 1 AU CERTIFICAT".into()))
        );
        assert_eq!(
            num("ANNEXE II PRESCRIPTIONS POUR LA RÉCEPTION CE"),
            Some((
                "Annexe II".into(),
                "PRESCRIPTIONS POUR LA RÉCEPTION CE".into()
            ))
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
