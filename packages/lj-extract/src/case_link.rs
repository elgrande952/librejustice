//! Le nom d'usage cité dans un corps : « la jurisprudence Danthony » vise une
//! décision qu'aucun numéro ne nomme, donc qu'aucun lexer ne voit.

use std::collections::HashMap;

use lj_core::address::CaseAddress;
use lj_core::case_names::case_name_key as key;

/// Les noms qu'un corps peut citer, et la clé pendante que chacun désigne. Une
/// clé de nom portée par plusieurs décisions n'y entre pas : « GISTI » nomme
/// quatre arrêts et le corps ne dit pas lequel.
#[derive(Debug, Default, Clone)]
pub struct CaseNameSnapshot {
    par_cle: HashMap<String, CaseAddress>,
    /// Premier mot replié vers la longueur de la plus longue clé qui commence par
    /// lui : une clé de 222 mots ne dicte plus la borne des 20 197 autres.
    par_tete: HashMap<String, usize>,
}

/// What the deciding court brings to the sorting of candidate targets.
#[derive(Debug, Default, Clone, Copy)]
pub struct Citing<'a> {
    /// `CJUE` or `CEDH`. A European court naming a French target is a homonym
    /// 2 555 times out of 2 687, so the target is refused by default
    /// (31/08/2026).
    pub european: bool,
    /// Reading date, ISO. A decision does not cite what comes after it.
    pub date: Option<&'a str>,
}

impl CaseNameSnapshot {
    /// `rows` = `(clé repliée du nom, adresse de la décision nommée)`.
    pub fn new(rows: impl IntoIterator<Item = (String, CaseAddress)>) -> Self {
        let mut par_cle = HashMap::new();
        let mut par_tete: HashMap<String, usize> = HashMap::new();
        for (cle, address) in rows {
            if cle.is_empty() {
                continue;
            }
            let mut mots = cle.split(' ');
            let Some(tete) = mots.next() else { continue };
            let n = mots.count() + 1;
            let borne = par_tete.entry(tete.to_string()).or_default();
            *borne = (*borne).max(n);
            par_cle.insert(cle, address);
        }
        CaseNameSnapshot { par_cle, par_tete }
    }

    pub fn is_empty(&self) -> bool {
        self.par_cle.is_empty()
    }
}

/// The two courts of the address that are not French.
const COURS_EUROPEENNES: [&str; 2] = ["cjue", "cedh"];

/// Codepoints of the identity block a European decision opens with (`AFFAIRE
/// LA ROSA ET 3 AUTRES c. ITALIE (No 5)`, `En l'affaire Granata c. France`).
/// It names the present case, and the catalogue answers with a sibling of the
/// same family: 299 of the 21 489 European spans, every reread one false
/// (31/08/2026).
const EUROPEAN_HEADER: usize = 400;

/// What the name key alone cannot see, measured on the 24 216 production spans
/// (31/08/2026): 2 555 attach the European judgment a European judge names
/// (`arrêt Kadi`) to a homonymous French decision, 929 point at a decision
/// handed down after the citing one.
fn impossible(cle: &str, cible: &CaseAddress, citing: &Citing<'_>, banniere: Option<&str>) -> bool {
    let cour = cible.court.as_deref().unwrap_or_default();
    if citing.european
        && !COURS_EUROPEENNES.contains(&cour)
        && !nomme_le_droit_francais(cle, banniere)
    {
        return true;
    }
    match (cible.date.as_deref(), citing.date) {
        (Some(cible), Some(citante)) => cible > citante,
        _ => false,
    }
}

/// A European decision whose respondent state is France reviews French law and
/// names it: 132 of the 2 687 French targets a European court points at, and the
/// only ones that hold up on reread (`décision Czabaj` 57, Darmont 8,
/// Gardedieu 5, Nicolo 3). Unless the name is the applicant's own, which the
/// banner also carries: `SANCHEZ c. FRANCE` writing `Sanchez` names itself.
fn nomme_le_droit_francais(cle: &str, banniere: Option<&str>) -> bool {
    let Some(b) = banniere else {
        return false;
    };
    b.contains(" c france") && !cle.split(' ').any(|mot| b.split(' ').any(|w| w == mot))
}

/// Sur 34 déclenchements relus, la locution fait la citation et le nom nu tombe
/// sur les parties ou l'en-tête. Accents et pluriel sont pliés par [`egal_plie`].
const LOCUTIONS: [&str; 4] = ["jurisprudence", "arret", "decision", "affaire"];

/// Mots qui s'intercalent entre la locution et le nom sans rien changer.
const LIAISONS: [&str; 2] = ["dite", "dit"];

/// Punctuation between the locution and the name: the greffe banner
/// (`AFFAIRE : Marie-Claude X…`), the apposed party (`l'agent d'affaires,
/// Beaufils`), the sentence already closed (`de cette décision. Gaston B…`).
/// 455 spans of the 24 216 in production, every reread one false (31/08/2026).
fn severed(separator: &str) -> bool {
    separator.contains(['.', ';', ':', '!', '?', ',', '\n'])
}

/// `chiffre d'affaires croissant`, `chargé d'affaires Marié`: glued to `d'`,
/// `affaire` is the common noun and announces nothing.
fn common_noun(mots: &[Mot<'_>], rang: usize) -> bool {
    egal_plie(mots[rang].mot, "affaire", true)
        && mots[rang].avant.starts_with(['\'', '’'])
        && rang > 0
        && egal_plie(mots[rang - 1].mot, "d", false)
}

/// Spans `(début, fin, clé pendante)` des noms d'usage cités, en **codepoints**,
/// triés et sans chevauchement. `soi` = les clés pendantes de la décision qui
/// porte le texte : elle se nomme dans son propre corps sans se citer.
pub fn cited_case_name_spans(
    text: &str,
    snap: &CaseNameSnapshot,
    soi: &[CaseAddress],
    citing: Citing<'_>,
) -> Vec<(usize, usize, CaseAddress)> {
    if snap.is_empty() {
        return Vec::new();
    }
    let mots = mots(text);
    // The identity banner, folded once: it carries the respondent state and the
    // applicant's name, which nothing else brings to the extraction.
    let banniere = citing
        .european
        .then(|| key(&text.chars().take(EUROPEAN_HEADER).collect::<String>()));
    let mut spans: Vec<(usize, usize, CaseAddress)> = Vec::new();
    let mut cle = String::new();
    let mut coupes: Vec<usize> = Vec::new();
    for (rang, mot) in mots.iter().enumerate() {
        if !LOCUTIONS.iter().any(|l| egal_plie(mot.mot, l, true)) {
            continue;
        }
        if common_noun(&mots, rang) {
            continue;
        }
        let mut tete = rang + 1;
        if mots
            .get(tete)
            .is_some_and(|m| LIAISONS.iter().any(|l| egal_plie(m.mot, l, false)))
        {
            tete += 1;
        }
        if tete >= mots.len() || mots[rang + 1..=tete].iter().any(|m| severed(m.avant)) {
            continue;
        }
        let mot_tete = key(mots[tete].mot);
        // Un mot qui ne commence aucune clé ferme le cas ici, ce qui est le sort
        // des 11 locutions d'une décision moyenne : rien n'est replié au-delà.
        let Some(&borne) = snap.par_tete.get(&mot_tete) else {
            continue;
        };
        let lim = borne.min(mots.len() - tete);
        // Les clés d'un même départ sont préfixes les unes des autres : on étend
        // une fois jusqu'à `lim`, puis on relit les coupes de la plus longue à la
        // plus courte.
        cle.clear();
        coupes.clear();
        cle.push_str(&mot_tete);
        coupes.push(cle.len());
        for i in 1..lim {
            cle.push(' ');
            cle.push_str(&key(mots[tete + i].mot));
            coupes.push(cle.len());
        }
        for taille in (1..=lim).rev() {
            let Some(nommee) = snap.par_cle.get(&cle[..coupes[taille - 1]]) else {
                continue;
            };
            let (debut, fin) = (mots[tete].debut, mots[tete + taille - 1].fin);
            let refuse = (citing.european && debut < EUROPEAN_HEADER)
                || soi.contains(nommee)
                || impossible(
                    &cle[..coupes[taille - 1]],
                    nommee,
                    &citing,
                    banniere.as_deref(),
                )
                || nom_tronque(&mots, tete + taille)
                || spans.last().is_some_and(|(_, f, _)| *f > debut);
            if !refuse {
                spans.push((debut, fin, nommee.clone()));
            }
            break;
        }
    }
    spans
}

/// Le nom reconnu n'est que la tête d'un nom plus long : « arrêt Soulier et
/// Doke » vise la décision de la CJUE, pas l'arrêt Soulier du Conseil d'État.
///
/// Three more continuations, worth 1 075 of the 24 216 production spans and
/// false to the last one reread (31/08/2026): a capitalised word right after
/// (`arrêt Kadi II`, `López Brea et Hidalgo Palacios` read as `López`), a mark
/// glued to the name (`Weber's Wine World`, `Marie-Claire Tatete`), a slash or
/// an ampersand (`France Télécom/Commission`, `Benjamin & Wilson`). The mark
/// must be glued: `jurisprudence Ternon '` and `arrêt Ringeisen - sur` close a
/// sentence, they do not lengthen a name.
fn nom_tronque(mots: &[Mot<'_>], apres: usize) -> bool {
    let Some(suivant) = mots.get(apres) else {
        return false;
    };
    if egal_plie(suivant.mot, "et", false) {
        return mots
            .get(apres + 1)
            .is_some_and(|m| m.mot.chars().next().is_some_and(char::is_uppercase));
    }
    let sep = suivant.avant;
    sep.starts_with(['-', '\'', '’'])
        || sep.trim_matches([' ', '\t']).starts_with(['&', '/'])
        || (!sep.is_empty()
            && sep.chars().all(|c| c == ' ' || c == '\t')
            && suivant.mot.chars().next().is_some_and(char::is_uppercase))
}

struct Mot<'a> {
    mot: &'a str,
    debut: usize,
    fin: usize,
    /// Text separating this word from the previous one: it carries the
    /// punctuation that splitting into words would otherwise throw away.
    avant: &'a str,
}

/// Découpe en mots avec leurs offsets en codepoints, comme les spans stockés.
fn mots(text: &str) -> Vec<Mot<'_>> {
    let mut mots = Vec::new();
    let mut ouvert: Option<(usize, usize)> = None;
    let mut precedent = 0usize;
    for (rang, (octet, c)) in text.char_indices().enumerate() {
        if c.is_alphanumeric() {
            if ouvert.is_none() {
                ouvert = Some((rang, octet));
            }
        } else if let Some((r, o)) = ouvert.take() {
            mots.push(Mot {
                mot: &text[o..octet],
                debut: r,
                fin: rang,
                avant: &text[precedent..o],
            });
            precedent = octet;
        }
    }
    if let Some((r, o)) = ouvert {
        mots.push(Mot {
            mot: &text[o..],
            debut: r,
            fin: r + text[o..].chars().count(),
            avant: &text[precedent..o],
        });
    }
    mots
}

/// Égalité accents et casse repliés, sans allocation. `pluriel` accepte le `s`
/// final du mot lu : « arrêts Danthony » cite comme « arrêt Danthony ».
fn egal_plie(mot: &str, cible: &str, pluriel: bool) -> bool {
    let mut lu = mot.chars().flat_map(lj_fold::fold_char);
    let mut attendu = cible.chars();
    loop {
        match (lu.next(), attendu.next()) {
            (Some(a), Some(b)) if a == b => continue,
            (None, None) => return true,
            (Some('s'), None) if pluriel => return lu.next().is_none(),
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> CaseNameSnapshot {
        CaseNameSnapshot::new([
            (
                "danthony".to_string(),
                CaseAddress::new("ce", "335033", "2011-12-23"),
            ),
            (
                "soulier".to_string(),
                CaseAddress::new("ce", "223041", "2002-10-30"),
            ),
            (
                "societe tropic travaux signalisation".to_string(),
                CaseAddress::new("ce", "291545", "2007-07-16"),
            ),
            (
                "tropic".to_string(),
                CaseAddress::new("ce", "291545", "2007-07-16"),
            ),
            (
                "granata c france".to_string(),
                CaseAddress::new("cedh", "39626/98", "2003-03-27"),
            ),
            (
                "czabaj".to_string(),
                CaseAddress::new("ce", "389011", "2016-07-13"),
            ),
            (
                "sanchez".to_string(),
                CaseAddress::new("ce", "401800", "2017-05-10"),
            ),
        ])
    }

    /// A French 2024 citing decision, later than every target in the snapshot.
    fn citante() -> Citing<'static> {
        Citing {
            european: false,
            date: Some("2024-01-01"),
        }
    }

    /// `court|docket` of each span, an address in its readable test form.
    fn rendu(a: &CaseAddress) -> String {
        let c = |o: &Option<String>| o.clone().unwrap_or_default();
        format!("{}|{}", c(&a.court), c(&a.docket))
    }

    fn cles(text: &str, soi: &[CaseAddress]) -> Vec<String> {
        cited_case_name_spans(text, &snap(), soi, citante())
            .into_iter()
            .map(|(_, _, r)| rendu(&r))
            .collect()
    }

    #[test]
    fn la_locution_fait_la_citation() {
        for texte in [
            "au sens de la jurisprudence Danthony et ont été de nature",
            "le Conseil d'Etat applique la jurisprudence dite Danthony à tout vice",
            "il résulte de l'arrêt Danthony que le vice",
            "la décision Danthony du 23 décembre 2011",
            "les arrêts Danthony et suivants",
            "il devra faire application de la « jurisprudence Danthony » dans la mesure où",
        ] {
            assert_eq!(cles(texte, &[]), vec!["ce|335033"], "texte {texte:?}");
        }
    }

    #[test]
    fn le_nom_nu_ne_cite_pas() {
        assert!(cles("M. Danthony demande l'annulation du décret", &[]).is_empty());
    }

    /// Deux des trois extraits relus portant « Soulier » visent l'arrêt de la
    /// CJUE, dont le nom complet est « Soulier et Doke ».
    #[test]
    fn le_nom_tronque_ne_cite_pas() {
        assert!(cles(
            "la solution de l'arrêt Soulier et Doke n'est pas transposable",
            &[]
        )
        .is_empty());
        assert_eq!(
            cles(
                "compte tenu de l'arrêt Soulier qui ne permet le retrait",
                &[]
            ),
            vec!["ce|223041"]
        );
    }

    #[test]
    fn la_decision_ne_se_cite_pas_elle_meme() {
        assert!(cles(
            "la jurisprudence Danthony",
            &[CaseAddress::new("ce", "335033", "2011-12-23")]
        )
        .is_empty());
    }

    /// La bannière du greffe, la partie apposée et la phrase déjà close.
    #[test]
    fn la_ponctuation_entre_la_locution_et_le_nom_coupe() {
        for texte in [
            "N° Minute : 24/01897  AFFAIRE :  Danthony et associés  C/  [V]",
            "l'agent d'affaires, Danthony, était chargé de rédiger les statuts",
            "il n'y a pas lieu de revenir sur cette décision. Danthony répondait le 31 juillet",
            "R.G. 11/04606  Affaire\nDanthony X... épouse Y...",
        ] {
            assert!(cles(texte, &[]).is_empty(), "texte {texte:?}");
        }
    }

    #[test]
    fn le_chiffre_d_affaires_n_est_pas_une_locution() {
        assert!(cles("un chiffre d'affaires Danthony de 688 000 euros", &[]).is_empty());
        assert!(cles("le chargé d’affaires Danthony a signé", &[]).is_empty());
        assert_eq!(
            cles("la solution retenue dans l'affaire Danthony vaut ici", &[]),
            vec!["ce|335033"]
        );
    }

    /// Three continuations the `et <Capital>` guard did not cover.
    #[test]
    fn le_nom_prolonge_ne_cite_pas() {
        for texte in [
            "il ressort de l'arrêt Danthony II, point 124, que le vice",
            "l'arrêt Danthony-Burckhardt du 22 octobre 1975",
            "l'arrêt Danthony's Wine World, C 147/01",
            "voir en ce sens l'arrêt Danthony/Commission, précité",
            "l'arrêt Danthony & Wilson c. Royaume-Uni",
        ] {
            assert!(cles(texte, &[]).is_empty(), "texte {texte:?}");
        }
    }

    /// A detached mark closes the sentence, it does not extend the name.
    #[test]
    fn la_marque_detachee_laisse_citer() {
        for texte in [
            "la jurisprudence Danthony ' 2°) Notamment les dispositions",
            "l'arrêt Danthony - sur l'organisation administrative",
        ] {
            assert_eq!(cles(texte, &[]), vec!["ce|335033"], "texte {texte:?}");
        }
    }

    /// `arrêt Kadi` under a European judge's pen names a European judgment,
    /// never the Conseil d'État decision carrying the same party name.
    #[test]
    fn la_juridiction_europeenne_ne_nomme_pas_une_cible_francaise() {
        let europeenne = Citing {
            european: true,
            date: Some("2024-01-01"),
        };
        assert!(cited_case_name_spans(
            "il ressort de l'arrêt Danthony que le vice",
            &snap(),
            &[],
            europeenne
        )
        .is_empty());
    }

    /// The ECHR ruling against France names French case law, and an applicant
    /// naming themselves is not citing anyone.
    #[test]
    fn l_etat_defendeur_rouvre_la_cible_francaise() {
        let bandeau = "CINQUIÈME SECTION AFFAIRE Sanchez c. FRANCE (Requête no 45581/15) ARRÊT ";
        let europeenne = Citing {
            european: true,
            date: Some("2024-01-01"),
        };
        let corps = format!(
            "{bandeau}{}Il résulte de la jurisprudence Czabaj que le délai court.",
            " ".repeat(400)
        );
        assert_eq!(
            cited_case_name_spans(&corps, &snap(), &[], europeenne)
                .into_iter()
                .map(|(_, _, r)| rendu(&r))
                .collect::<Vec<_>>(),
            vec!["ce|389011"]
        );
        // Same banner, but the name aimed at is the applicant's own.
        let soi = format!(
            "{bandeau}{}La requête de l'arrêt Sanchez est recevable.",
            " ".repeat(400)
        );
        assert!(cited_case_name_spans(&soi, &snap(), &[], europeenne).is_empty());
        // With no France in the banner, the French target stays refused.
        let turquie = format!(
            "QUATRIÈME SECTION AFFAIRE Yilmaz c. TURQUIE{}Il résulte de la jurisprudence Czabaj que.",
            " ".repeat(400)
        );
        assert!(cited_case_name_spans(&turquie, &snap(), &[], europeenne).is_empty());
    }

    /// A European decision's identity banner names the case at hand, and the
    /// catalogue answers with a sibling of the same family.
    #[test]
    fn le_bandeau_europeen_ne_cite_pas() {
        let europeenne = Citing {
            european: true,
            date: Some("2024-01-01"),
        };
        let entete = "PREMIÈRE SECTION AFFAIRE Granata c. France (no 3) (Requête no 39634/98)";
        assert!(cited_case_name_spans(entete, &snap(), &[], europeenne).is_empty());
        let corps = format!(
            "{}Il ressort de l'arrêt Granata c. France que",
            " ".repeat(420)
        );
        assert_eq!(
            cited_case_name_spans(&corps, &snap(), &[], europeenne)
                .into_iter()
                .map(|(_, _, r)| rendu(&r))
                .collect::<Vec<_>>(),
            vec!["cedh|39626/98"]
        );
    }

    #[test]
    fn une_decision_ne_cite_pas_ce_qui_la_suit() {
        let avant = Citing {
            european: false,
            date: Some("2005-06-01"),
        };
        assert!(cited_case_name_spans(
            "au sens de la jurisprudence Danthony et ont été de nature",
            &snap(),
            &[],
            avant
        )
        .is_empty());
    }

    #[test]
    fn le_nom_le_plus_long_gagne() {
        let spans = cited_case_name_spans(
            "la jurisprudence Société Tropic Travaux Signalisation ouvre le recours",
            &snap(),
            &[],
            citante(),
        );
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].0, spans[0].1), (17, 53));
    }

    #[test]
    fn les_spans_comptent_en_codepoints() {
        let texte = "où l'intérêt s'apprécie, la jurisprudence Danthony";
        let spans = cited_case_name_spans(texte, &snap(), &[], citante());
        let lu: String = texte.chars().skip(spans[0].0).take(8).collect();
        assert_eq!(lu, "Danthony");
    }
}
