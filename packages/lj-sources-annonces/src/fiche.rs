//! The platforms share neither template nor vocabulary, yet all write a notice
//! the same way: a label in bold, then its value up to the next label. Each
//! engine brings only its own vocabulary for the description.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use lj_annonces::{ContenuAnnonce, LotAnnonce, MontantEstime};

use super::txt;

/// The break is the separator of every list a notice publishes, so it survives
/// the strip instead of collapsing with it.
pub(crate) fn lignes(html: &str) -> Vec<String> {
    static SAUT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)<br\s*/?>|</(?:div|p|li|tr|dd|dt|td)>")
            .expect("regex saut de ligne valide")
    });
    SAUT.replace_all(html, "\n")
        .split('\n')
        .map(txt)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Folded, freed of the numbering the press layouts prepend to a label
/// (`6. Critères d'attribution`) and of its trailing colon.
pub(crate) fn cle(label: &str) -> String {
    static NUMERO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\d+\s*[.)-]\s*|^[a-z]\)\s*").expect("regex numérotation valide")
    });
    let plie = lj_fold::fold(label);
    NUMERO
        .replace(&plie, "")
        .trim_end_matches([':', ' ', '*'])
        .trim()
        .to_string()
}

/// A labelled field of a notice: the value beside the label, and the block that
/// runs to the next label. A layout puts the value in one or in the other.
pub(crate) struct Champ {
    pub cle: String,
    pub label: String,
    pub valeur: String,
    pub bloc: String,
}

impl Champ {
    /// The value wherever the layout put it: beside the label, or alone in the
    /// cell or paragraph that follows.
    pub fn texte(&self) -> String {
        if !self.valeur.is_empty() {
            return self.valeur.clone();
        }
        lignes(&self.bloc).first().cloned().unwrap_or_default()
    }
}

/// A block never carries a whole page: past this, the walk has missed the label
/// that closes it, and the value would be the rest of the document.
const BLOC_MAX: usize = 8_000;

/// The cut of the block above, walked back onto a character: a byte budget lands
/// mid-codepoint on the accented text these pages are made of.
fn borne(html: &str, mut fin: usize) -> usize {
    while fin > 0 && !html.is_char_boundary(fin) {
        fin -= 1;
    }
    fin
}

/// Every platform marks its labels with one of these tags. Document order is
/// kept: it is what binds a CPV to the lot it follows. A heading is a label
/// too, if only to close the field above it, whether its tag says so or a
/// class does.
fn champs_en_gras(html: &str) -> Vec<Champ> {
    static OUVRANTE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)<(strong|b|dt|th|span|td|div|p|li|h[1-6])\b([^>]*)>")
            .expect("regex étiquette")
    });
    static EN_GRAS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)class\s*=\s*.[^\x22']*(?:gras|bold|label|libelle|intitule|card-header|panel-heading|fr-h\d)",
        )
        .expect("regex classe en gras")
    });
    let bornes: Vec<_> = OUVRANTE
        .captures_iter(html)
        .filter(|c| {
            matches!(
                c[1].to_lowercase().as_str(),
                "strong" | "b" | "dt" | "th" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            ) || EN_GRAS.is_match(&c[2])
        })
        .collect();
    let mut out = Vec::with_capacity(bornes.len());
    for (i, c) in bornes.iter().enumerate() {
        let tout = c.get(0).expect("match global");
        let fermante = format!("</{}", c[1].to_lowercase());
        let apres = &html[tout.end()..];
        let Some(fin_label) = apres.to_lowercase().find(&fermante) else {
            continue;
        };
        let label = txt(&apres[..fin_label])
            .trim()
            .trim_end_matches([':', ' '])
            .to_string();
        let cle = cle(&label);
        if cle.is_empty() {
            continue;
        }
        let Some(reste) = apres[fin_label..].split_once('>').map(|(_, r)| r) else {
            continue;
        };
        let debut = html.len() - reste.len();
        let suivante = bornes
            .get(i + 1)
            .and_then(|n| n.get(0))
            .map(|m| m.start())
            .unwrap_or(html.len())
            .max(debut);
        let court = [
            "<br", "</div", "</p", "</li", "</td", "</tr", "</dd", "</dt", "</th",
        ]
        .iter()
        .filter_map(|m| reste.find(m))
        .min()
        .unwrap_or(reste.len());
        out.push(Champ {
            cle,
            label,
            valeur: txt(&reste[..court])
                .trim_start_matches(':')
                .trim()
                .to_string(),
            bloc: html[debut..borne(html, suivante.min(debut + BLOC_MAX))].to_string(),
        });
    }
    out
}

/// A platform that styles its labels in CSS marks them nowhere in the markup:
/// what names them is a short line closed by a colon, the value being what
/// follows up to the next such line. A full stop disqualifies a label, which is
/// how a sentence is told from one, so a notice that numbers its labels has that
/// numbering read off first, be it the `II.1.4)` of the European form, the `IV/`
/// a buyer writes its own notice with, or a plain `1.`.
fn champs_en_lignes(html: &str) -> Vec<Champ> {
    static ETIQUETTE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:[IVXivx]+(?:\.[\d.]*)?[.)/]\s*|\d+\s*[.)/-]\s*)?([A-Za-zÀ-ÿ][^:;.!?]{1,58}?)\s*:\s*(.*)$",
        )
            .expect("regex étiquette en ligne")
    });
    let lignes = lignes(html);
    let mut out: Vec<Champ> = Vec::new();
    for ligne in &lignes {
        match ETIQUETTE.captures(ligne) {
            Some(m) => {
                let label = m[1].trim().to_string();
                let cle = cle(&label);
                if cle.is_empty() || label.split_whitespace().count() > 6 {
                    continue;
                }
                let valeur = m[2].trim().to_string();
                out.push(Champ {
                    cle,
                    label,
                    bloc: format!("{valeur}<br>"),
                    valeur,
                });
            }
            None => {
                if let Some(dernier) = out.last_mut() {
                    if dernier.bloc.len() < BLOC_MAX {
                        dernier.bloc.push_str(ligne);
                        dernier.bloc.push_str("<br>");
                    }
                }
            }
        }
    }
    out
}

/// The two walks together: a platform marks its labels in the markup, in CSS,
/// or in both, and a key found twice is read from the marked one.
///
/// A CSS declaration and a JavaScript object entry are written `clé : valeur`
/// like a field, so a page that inlines either enters the walk with a hundred
/// of them.
pub(crate) fn champs(html: &str) -> Vec<Champ> {
    static CODE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<script\b[^>]*>.*?</script\s*>|<style\b[^>]*>.*?</style\s*>")
            .expect("regex script ou style")
    });
    let html = &CODE.replace_all(html, " ");
    let mut out = champs_en_gras(html);
    let vus: HashSet<String> = out.iter().map(|c| c.cle.clone()).collect();
    out.extend(
        champs_en_lignes(html)
            .into_iter()
            .filter(|c| !vus.contains(&c.cle)),
    );
    out
}

/// The first label of the list that the notice **fills**, the list being ordered
/// by how well each names the thing rather than by how often it appears. A form
/// heads its section with the very label its field carries below, so a label is
/// followed until one occurrence has a value.
pub(crate) fn premier(champs: &[Champ], cles: &[&str]) -> Option<String> {
    cles.iter().find_map(|k| {
        champs.iter().filter(|c| c.cle == *k).find_map(|c| {
            // The block, never the value beside the label: a value wrapped
            // in the source is one line there and two in the block, and
            // stitching the two writes its tail twice.
            let l = lignes(&c.bloc);
            let d = match l.is_empty() {
                true => c.valeur.clone(),
                false => l.join(" "),
            };
            Some(d.trim().trim_start_matches(':').trim().to_string()).filter(|d| !d.is_empty())
        })
    })
}

pub(crate) fn cpv_de(valeur: &str) -> Option<String> {
    static CPV: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(\d{8}(?:-\d)?)\b").expect("regex cpv valide"));
    CPV.captures(valeur).map(|c| c[1].to_string())
}

/// The four ways a notice designates a lot, most explicit first. One page
/// carries two of them at once, so the first that yields anything wins alone.
fn formes_lot() -> &'static [Regex; 4] {
    static FORMES: LazyLock<[Regex; 4]> = LazyLock::new(|| {
        [
            r"^lot n[°o]\s*\d+$",
            r"^lot \d+$",
            r"^lot unique$",
            r"^(?:description|intitule|libelle) du lot$",
        ]
        .map(|f| Regex::new(f).expect("regex forme de lot valide"))
    });
    &FORMES
}

pub(crate) fn est_lot(cle: &str) -> bool {
    formes_lot().iter().any(|f| f.is_match(cle))
}

/// A lot carries its CPV either beside it, in the `Code CPV` that follows, or
/// alone in its own block when the notice numbers its lots without naming them.
fn lots_de_forme(champs: &[Champ], forme: &Regex) -> Vec<LotAnnonce> {
    // A layout that repeats the lot number beside its heading separates the two
    // with a dash, and closes the line on the lot CPV, which `cpv` already holds.
    static ORNEMENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\s*-?\s*CPV\s*:?\s*\d{8}(?:-\d)?\s*$").expect("regex ornement de lot")
    });
    let mut out: Vec<LotAnnonce> = Vec::new();
    for c in champs {
        if forme.is_match(&c.cle) {
            let texte = c.texte();
            out.push(LotAnnonce {
                nom: Some(if texte.is_empty() {
                    c.label.clone()
                } else {
                    ORNEMENT
                        .replace(texte.trim_start_matches(['-', ':', ' ']), "")
                        .trim_end_matches(['.', ':', ' '])
                        .to_string()
                }),
                description: None,
                cpv: cpv_de(&c.bloc),
            });
        } else if c.cle.starts_with("code cpv") {
            if let Some(lot) = out.last_mut() {
                if lot.cpv.is_none() {
                    lot.cpv = cpv_de(&c.texte());
                }
            }
        }
    }
    let mut vus = HashSet::new();
    out.retain(|l| vus.insert(l.nom.clone()));
    out
}

/// The press layouts hang the whole allotment under a single label and list it
/// as prose, one `Lot 02 : couverture tuiles` a line. It names them, so it wins.
fn lots_en_prose(champs: &[Champ]) -> Vec<LotAnnonce> {
    static LIGNE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(lot\s*n?[°o]?\s*\d+\s*[:.-]\s*\S.*)$")
            .expect("regex ligne de lot valide")
    });
    champs
        .iter()
        .filter(|c| c.cle.contains("lots"))
        .flat_map(|c| lignes(&c.bloc))
        .filter_map(|l| {
            LIGNE
                .captures(&l)
                .map(|m| m[1].trim_end_matches('.').to_string())
        })
        .map(|nom| LotAnnonce {
            nom: Some(nom),
            description: None,
            cpv: None,
        })
        .collect()
}

pub(crate) fn lots(champs: &[Champ]) -> Vec<LotAnnonce> {
    let prose = lots_en_prose(champs);
    if !prose.is_empty() {
        return prose;
    }
    formes_lot()
        .iter()
        .map(|f| lots_de_forme(champs, f))
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

/// Two shapes: typed with its weight beside, or a preamble then one bullet a
/// line. A notice that allots repeats the whole block identically for each lot.
pub(crate) fn criteres(champs: &[Champ]) -> Vec<String> {
    static TYPE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)crit[eè]re[^:]*:\s*(.+?)\s*,\s*pond[eé]ration\s*:\s*([0-9]+(?:[.,][0-9]+)?)",
        )
        .expect("regex critère typé valide")
    });
    static PUCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(?:-|\d{1,2}\s*[.)])?\s*([^:]{2,90}?)\s*:\s*(?:sur\s+)?([0-9]+(?:[.,][0-9]+)?\s*(?:%|points?))\s*[.,;]?$",
        )
        .expect("regex critère à puce valide")
    });
    /// The same scale written as prose, its items telling themselves apart by
    /// their numbering alone.
    static EN_PROSE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\d{1,2}\s*[.)]\s*([^:;]{2,80}?)\s*:\s*([0-9]+(?:[.,][0-9]+)?\s*(?:%|points?))",
        )
        .expect("regex barème en prose valide")
    });
    let mut out = Vec::new();
    for c in champs.iter().filter(|c| {
        c.cle.contains("criteres d'attrib")
            || c.cle.contains("criteres de selection")
            || c.cle.contains("criteres de jugement")
    }) {
        for m in TYPE.captures_iter(&c.valeur) {
            out.push(format!(
                "{} ({})",
                m[1].trim_end_matches('.').trim(),
                m[2].trim()
            ));
        }
        // A table lays the weight in its own cell, so it lands on its own line
        // and names the criterion that precedes it.
        static SEUL: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^(\d+(?:[.,]\d+)?)\s*%$").expect("regex poids seul valide")
        });
        let lignes = lignes(&c.bloc);
        for (i, ligne) in lignes.iter().enumerate() {
            if let Some(m) = PUCE.captures(ligne) {
                out.push(format!("{} ({})", m[1].trim(), m[2].trim()));
            } else if let Some(m) = SEUL.captures(ligne) {
                if let Some(nom) = lignes[..i]
                    .iter()
                    .rev()
                    .find(|l| !SEUL.is_match(l))
                    .filter(|l| l.len() <= 80 && !l.ends_with(':'))
                {
                    out.push(format!(
                        "{} ({} %)",
                        nom.trim_end_matches([':', ' ']),
                        &m[1]
                    ));
                }
            }
        }
        for m in EN_PROSE.captures_iter(&lignes.join(" ")) {
            out.push(format!("{} ({})", m[1].trim(), m[2].trim()));
        }
    }
    let mut vus = HashSet::new();
    out.retain(|c| vus.insert(c.clone()));
    out
}

/// The figure as the notice writes it: a space, a dot or a comma groups the
/// thousands, and a separator is the decimal one only when it closes on one or
/// two digits. The European form writes `330,000 Euro` where the national one
/// writes `330 000 euros`, and reading the comma as a decimal point divides the
/// amount by a thousand.
fn nombre(brut: &str) -> Option<f64> {
    static DECIMALE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[.,]\d{1,2}$").expect("regex décimale valide"));
    let serre = brut.replace(['\u{a0}', ' '], "");
    let plat = match DECIMALE.find(&serre) {
        Some(m) => format!(
            "{}.{}",
            serre[..m.start()].replace(['.', ','], ""),
            &serre[m.start() + 1..]
        ),
        None => serre.replace(['.', ','], ""),
    };
    plat.parse::<f64>().ok().filter(|v| *v > 0.0)
}

/// The amount, from the first label that heads one and carries a figure: a form
/// that titles its amount (`Valeur totale estimée :`) leaves that line empty and
/// writes the figure under it, in a field of its own.
pub(crate) fn montant_estime(champs: &[Champ]) -> Option<MontantEstime> {
    static NOMBRE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d[\d \u{a0}.,]*\d|\d").expect("regex nombre valide"));
    static DEVISE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\beur\b|euro|€").expect("regex devise valide"));
    champs
        .iter()
        .filter(|c| {
            c.cle.contains("montant")
                || c.cle.contains("valeur estim")
                || c.cle.contains("valeur tot")
                || c.cle.contains("valeur hors")
        })
        .find_map(|c| {
            let texte = c.texte();
            Some(MontantEstime {
                valeur: nombre(NOMBRE.find(&texte)?.as_str())?,
                devise: DEVISE
                    .is_match(&lj_fold::fold(&texte))
                    .then(|| "EUR".to_string()),
            })
        })
}

/// The three fields whose vocabulary is common to every platform. The
/// description is not one: each names it otherwise, so each supplies its list.
pub(crate) fn contenu(champs: &[Champ], description: Option<String>) -> ContenuAnnonce {
    ContenuAnnonce {
        description: description.map(|d| d.chars().take(4_000).collect()),
        lots: lots(champs),
        criteres: criteres(champs),
        montant_estime: montant_estime(champs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A label beside its value, the shape of an aggregator card.
    const EN_LIGNE: &str = r#"<strong>Objet du marché : </strong>travaux AEP.<br>
	<strong>Montant maximum annuel : </strong>150 000 euros HT sur 7 ans.<br>
	<strong>Critères d'attribution : </strong>offre la plus avantageuse :<br>
	- prix des prestations : 30 %,<br>
	- valeur technique : 65 %.<br>"#;

    /// A label above its value, the shape of a press portal, and the same four
    /// fields read by the same walk.
    const EN_PARAGRAPHE: &str = r#"<p><strong>Objet du marché</strong></p><p>Reprise des tableaux de la salle de judo</p>
	<p><strong>Critères de sélection</strong></p><p>- prix des prestations : 60%</p><p>- valeur technique de l'offre : 40%</p>"#;

    #[test]
    fn a_label_beside_its_value_reads_like_a_label_above_it() {
        let ligne = champs(EN_LIGNE);
        assert_eq!(
            premier(&ligne, &["objet du marche"]).as_deref(),
            Some("travaux AEP.")
        );
        assert_eq!(
            criteres(&ligne),
            ["prix des prestations (30 %)", "valeur technique (65 %)"]
        );
        assert_eq!(
            montant_estime(&ligne),
            Some(MontantEstime {
                valeur: 150_000.0,
                devise: Some("EUR".to_string()),
            })
        );

        let para = champs(EN_PARAGRAPHE);
        assert_eq!(
            premier(&para, &["objet du marche"]).as_deref(),
            Some("Reprise des tableaux de la salle de judo")
        );
        assert_eq!(
            criteres(&para),
            [
                "prix des prestations (60%)",
                "valeur technique de l'offre (40%)"
            ]
        );
    }

    #[test]
    fn a_bare_label_falls_back_to_the_cell_that_follows() {
        let c = champs(r#"<b>Code CPV</b></td><td>45262210-6</td>"#);
        assert_eq!(c[0].texte(), "45262210-6");
        assert_eq!(cpv_de(&c[0].texte()).as_deref(), Some("45262210-6"));

        let table = champs(
            r#"<tr><td><b>Lot n° 1</b></td><td>Gros oeuvre</td></tr>
	<tr><td><b>Code CPV</b></td><td>45262210-6</td></tr>"#,
        );
        assert_eq!(
            lots(&table),
            [LotAnnonce {
                nom: Some("Gros oeuvre".to_string()),
                description: None,
                cpv: Some("45262210-6".to_string()),
            }]
        );
    }

    /// A list item closes the value of the label it opened with, which no `<p>`
    /// nor `<td>` marks here.
    #[test]
    fn a_value_ends_with_the_item_that_carries_it() {
        let c = champs(
            r#"<ul><li><span class="Texte_Gras">Type de procédure</span><span>Appel d'offres ouvert</span></li>
	<li><span class="Texte_Gras">Objet</span><span>Fourniture de véhicules</span></li></ul>"#,
        );
        assert_eq!(
            premier(&c, &["type de procedure"]).as_deref(),
            Some("Appel d'offres ouvert")
        );
        assert_eq!(
            premier(&c, &["objet"]).as_deref(),
            Some("Fourniture de véhicules")
        );
    }

    /// The scale written as prose, where the numbering is the only thing that
    /// tells one criterion from the next.
    #[test]
    fn a_numbered_scale_is_read_run_on_as_it_is_read_a_line_each() {
        let prose = champs(
            r#"<p><strong>Critères d'attribution :</strong> Offre économiquement la plus avantageuse appréciée en fonction des critères suivants avec leur pondération : 1.La valeur technique : 50% 2.Le prix des prestations : 40%</p>"#,
        );
        assert_eq!(
            criteres(&prose),
            ["La valeur technique (50%)", "Le prix des prestations (40%)"]
        );

        let lignes = champs(
            r#"<strong>Critères d'attribution :</strong><br>1. Valeur technique : 60 points<br>2. Prix : 40 points<br>"#,
        );
        assert_eq!(
            criteres(&lignes),
            ["Valeur technique (60 points)", "Prix (40 points)"]
        );
    }

    /// A field no later label closes is cut at the byte budget, which on accented
    /// text lands mid-character.
    #[test]
    fn le_budget_d_octets_du_bloc_ne_coupe_pas_un_caractere_en_deux() {
        let long = format!("<strong>Objet :</strong>a{}", "é".repeat(BLOC_MAX));
        assert_eq!(champs(&long).len(), 1);
    }
}
