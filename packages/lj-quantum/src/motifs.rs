//! The motifs, section by section: what the court awards, refuses or confirms
//! on each poste of the direct victim.
use std::sync::OnceLock;

use aho_corasick::{AhoCorasick, AhoCorasickKind, MatchKind};

use super::ancrage::jetons;
use super::dispositif::{Extraction, Mode, Nature};
use super::leftovers::{self, Amount};
use super::vocabulary::{self, Cue, CueTok};
use super::{Poste, Victime, EPS};
use lj_extract::scan::DocScan;

/// Where the court's own discussion opens inside the motifs. What precedes it
/// recites the claims and the judgment, and the same figure read there would be
/// a party's, not the court's.
const DISCUSSION: [&str; 12] = [
    "motifs de la decision",
    "expose des motifs",
    "motifs de l'arret",
    "motifs de la cour",
    "sur ce, la cour",
    "cela etant expose",
    "reponse de la cour",
    "considerant ce qui suit",
    "motivation",
    "discussion",
    "sur ce",
    "motifs",
];

/// Words a heading may carry before the label of the poste it opens.
const TITLE_WORDS: &[&str] = &[
    "sur",
    "le",
    "la",
    "les",
    "l",
    "de",
    "du",
    "des",
    "d",
    "au",
    "aux",
    "a",
    "en",
    "ce",
    "qui",
    "concerne",
    "concernant",
    "quant",
    "s",
    "agissant",
    "titre",
    "poste",
    "postes",
    "chef",
    "chefs",
    "indemnisation",
    "evaluation",
    "reparation",
    "liquidation",
    "demande",
    "demandes",
    "prejudice",
    "prejudices",
    "frais",
    "autres",
];

/// A heading carrying one of these opens the block of a proche: the record
/// holds no name, so what follows is nobody's until a direct poste reopens it.
const RICOCHET: [&str; 18] = [
    "consorts",
    "proches",
    "ayant droit",
    "ayants droit",
    "victime indirecte",
    "victimes indirectes",
    "par ricochet",
    "ses parents",
    "les parents",
    "son pere",
    "sa mere",
    "ses enfants",
    "son epouse",
    "son epoux",
    "sa compagne",
    "son compagnon",
    "sa veuve",
    "son veuf",
];

/// A rate the court multiplies rather than the poste it prices.
const UNIT_MARKS: [&str; 24] = [
    "point",
    "taux",
    "tarif",
    "forfait",
    "base de",
    "de l'heure",
    "unitaire",
    "le point",
    "du point",
    "par point",
    "par jour",
    "quotidien",
    "le jour",
    "la journee",
    "journali",
    "par semaine",
    "hebdomadaire",
    "par mois",
    "mensuel",
    "par an",
    "annuel",
    "de rente",
    "par heure",
    "horaire",
];

/// What closes the court's own operation is its result, never the unit price it
/// multiplied one line above.
const RESULT: [&str; 5] = [
    "soit une somme",
    "soit la somme",
    "soit un montant",
    "soit un total",
    "soit au total",
];

/// What a sentence names beside the figure to make it somebody else's: a
/// third-party payer's claim, an expert's estimate, the whole of the harm.
const NOT_A_POSTE: [&str; 25] = [
    "sommes precitees",
    "sommes susvisees",
    "sommes ci-dessus",
    "caisse",
    "cpam",
    "creance",
    "debours",
    "indemnites journalieres",
    "tiers payeur",
    "provision",
    "expert",
    "recours",
    "devis",
    "facture",
    "bulletin",
    "piece ",
    "avis d'imposition",
    "frais irrepetibles",
    "article 700",
    "apres deduction",
    "prejudice corporel",
    "prejudices corporels",
    "ses prejudices",
    "les prejudices subis",
    "l'integralite de son prejudice",
];

/// Families of postes the court totals before it opens the next one. Named
/// before the figure, they price a group; named after it, they are the title of
/// what follows.
/// Where the poste by poste discussion closes: what an opener of the account
/// introduces is a total, and no section reaches past it.
const RECAP: [&str; 5] = [
    "il resulte de tout ce qui precede",
    "il resulte de ce qui precede",
    "il suit de ce qui precede",
    "compte tenu de tout ce qui precede",
    "en definitive",
];
const GROUPS: [&str; 5] = [
    "prejudices patrimoniaux",
    "prejudices extra-patrimoniaux",
    "prejudices temporaires",
    "prejudices permanents",
    "ensemble de ce qui precede",
];

/// What hangs a label on the figure that precedes it: the complement names the
/// poste, and another poste's name takes the figure away from this section.
const HANGERS: [&str; 6] = [
    "au titre",
    "a titre",
    "en reparation",
    "en indemnisation",
    "du chef",
    "correspondant a",
];

/// Complements that hang the figure back on the section itself.
const ANAPHORA: [&str; 6] = [
    "ce poste",
    "ce chef",
    "ce prejudice",
    "ce dernier",
    "ce titre",
    "cette indemnisation",
];

/// The award the sentence writes is the share, not the poste: a reduction takes
/// its part before the figure is written.
const REDUCED: [&str; 10] = [
    "reduction",
    "reduit",
    "partage",
    "perte de chance",
    "droit a indemnisation",
    "apres application",
    "apres limitation",
    "revenant a",
    "en faveur de",
    "a la charge de",
];
/// What opens a sentence whose verb pays a share of the poste, the poste itself
/// priced in the sentence before.
const CUT_DOWN: [&str; 6] = [
    "apres application",
    "apres limitation",
    "apres reduction",
    "apres partage",
    "taux de perte de chance",
    "compte tenu du partage",
];

/// How far the governing verb may sit from the amount it prices.
const GOVERN: usize = 150;

/// Above this, the section prices its own intermediate steps and the choice
/// between them is a guess: the sure mode then writes nothing.
const AMOUNTS_PER_SECTION: usize = 3;

/// How far a section runs when no other heading closes it: beyond, the headings
/// were missed and the figures below belong to another poste.
const SECTION: usize = 1200;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    /// The court prices the poste itself, in its own voice.
    Award,
    /// The court adopts the figure of the judgment, which it then also names.
    Confirm,
    /// What the first judge priced, as the appeal recites it.
    First,
    /// What a party asks, which is not what the court decides.
    Claim,
    /// What the defendant offers, which is not what the court decides either.
    Offer,
    /// The figure is nobody's decision on the poste: an expert's, a rate, a
    /// third-party payer's claim, or an allocation named without a subject,
    /// « l'octroi de la somme de X », which prices a step as often as a poste.
    Block,
    /// The court reverses: the judgment's figure is no longer the award.
    Infirm,
    Reject,
    Reserve,
    /// Matched only to keep a longer surface from reading as a shorter one.
    Shadow,
}

impl Role {
    /// Does the mark say whose the next figure is? A section flag does not.
    fn governs(self) -> bool {
        matches!(
            self,
            Role::Award | Role::Confirm | Role::First | Role::Claim | Role::Offer | Role::Block
        )
    }
}

#[rustfmt::skip]
const MARKS: &[(&str, Role)] = &[
    ("il sera alloue", Role::Award),
    ("il sera alloues", Role::Award),
    ("il sera allouee", Role::Award),
    ("sera alloue", Role::Award),
    ("sera allouee", Role::Award),
    ("seront alloues", Role::Award),
    ("seront allouees", Role::Award),
    ("y a lieu d'allouer", Role::Award),
    ("y a lieu de lui allouer", Role::Award),
    ("y a lieu de leur allouer", Role::Award),
    ("convient d'allouer", Role::Award),
    ("convient de lui allouer", Role::Award),
    ("convient de leur allouer", Role::Award),
    ("la cour alloue", Role::Award),
    ("lui alloue la somme", Role::Award),
    ("il sera accorde", Role::Award),
    ("sera accorde", Role::Award),
    ("sera accordee", Role::Award),
    ("seront accordees", Role::Award),
    ("convient d'accorder", Role::Award),
    ("y a lieu d'accorder", Role::Award),
    ("sera fixe a", Role::Award),
    ("sera fixee a", Role::Award),
    ("seront fixes a", Role::Award),
    ("seront fixees a", Role::Award),
    ("doit etre repare a hauteur", Role::Award),
    ("doit etre reparee a hauteur", Role::Award),
    ("doit etre fixe a", Role::Award),
    ("doit etre fixee a", Role::Award),
    ("convient de fixer", Role::Award),
    ("y a lieu de fixer", Role::Award),
    ("la cour fixe", Role::Award),
    ("sera arrete a", Role::Award),
    ("sera arretee a", Role::Award),
    ("sera evalue a", Role::Award),
    ("sera evaluee a", Role::Award),
    ("sera estime a", Role::Award),
    ("sera estimee a", Role::Award),
    ("seront estimes a", Role::Award),
    ("seront estimees a", Role::Award),
    ("doit etre estime a", Role::Award),
    ("doit etre estimee a", Role::Award),
    ("doivent etre estimes a", Role::Award),
    ("doivent etre estimees a", Role::Award),
    ("sera evalue", Role::Award),
    ("sera evaluee", Role::Award),
    ("sera fixe", Role::Award),
    ("sera fixee", Role::Award),
    ("seront evalues a", Role::Award),
    ("convient d'evaluer", Role::Award),
    ("y a lieu d'evaluer", Role::Award),
    ("la cour evalue", Role::Award),
    ("en l'evaluant a", Role::Block),
    ("en l'evaluant a la somme", Role::Award),
    ("en evaluant ce prejudice", Role::Award),
    ("en evaluant ce poste", Role::Award),
    ("en evaluant ce chef", Role::Award),
    ("en fixant le montant", Role::Award),
    ("en le fixant a", Role::Award),
    ("en la fixant a", Role::Award),
    ("en les fixant a", Role::Award),
    ("appreciation suffisante", Role::Award),
    ("suffisamment repare", Role::Award),
    ("suffisamment evalue", Role::Award),
    ("suffisamment apprecie", Role::Award),
    ("l'evaluant a", Role::Block),
    ("y a lieu d'indemniser", Role::Award),
    ("y a lieu de l'indemniser", Role::Award),
    ("convient d'indemniser", Role::Award),
    ("sera indemnise", Role::Award),
    ("sera indemnisee", Role::Award),
    ("seront indemnises", Role::Award),
    ("sera repare", Role::Award),
    ("sera reparee", Role::Award),
    ("seront repares", Role::Award),
    ("justifie l'octroi", Role::Block),
    ("justifient l'octroi", Role::Block),
    ("l'octroi d'une somme", Role::Block),
    ("l'octroi de la somme", Role::Block),
    ("par l'allocation", Role::Block),
    ("l'allocation d'une somme", Role::Block),
    ("l'allocation de la somme", Role::Block),
    ("sera retenu a hauteur", Role::Award),
    ("sera retenue a hauteur", Role::Award),
    ("seront retenues a hauteur", Role::Award),
    ("sera fait droit", Role::Award),
    ("il sera fait une juste", Role::Award),
    ("il en sera fait une juste", Role::Award),
    ("en octroyant", Role::Award),
    ("en lui octroyant", Role::Award),
    ("en leur octroyant", Role::Award),
    ("en accordant", Role::Award),
    ("en lui accordant", Role::Award),
    ("en leur accordant", Role::Award),
    ("en allouant", Role::Award),
    ("en lui allouant", Role::Award),
    ("en leur allouant", Role::Award),
    ("mettre a la charge", Role::Award),
    ("mise a la charge", Role::Award),
    ("y a lieu de condamner", Role::Award),
    ("convient de condamner", Role::Award),
    ("doivent etre fixes a", Role::Award),
    ("doivent etre fixees a", Role::Award),
    ("sera liquide a", Role::Award),
    ("sera liquidee a", Role::Award),
    ("soit un total de", Role::Block),
    ("soit un montant total de", Role::Block),
    ("soit au total", Role::Block),

    ("sera confirme", Role::Confirm),
    ("sera confirmee", Role::Confirm),
    ("seront confirmes", Role::Confirm),
    ("est confirme", Role::Confirm),
    ("doit etre confirme", Role::Confirm),
    ("doit etre confirmee", Role::Confirm),
    ("convient de confirmer", Role::Confirm),
    ("y a lieu de confirmer", Role::Confirm),
    ("la cour confirme", Role::Confirm),
    ("c'est a raison que", Role::Confirm),
    ("c'est a juste titre que", Role::Confirm),
    ("c'est donc a juste titre que", Role::Confirm),
    ("juste appreciation", Role::Confirm),
    ("juste evaluation", Role::Confirm),
    ("exactement fixe", Role::Confirm),
    ("exactement evalue", Role::Confirm),
    ("a exactement", Role::Confirm),

    ("le tribunal a alloue", Role::First),
    ("les premiers juges ont alloue", Role::First),
    ("a alloue", Role::First),
    ("ont alloue", Role::First),
    ("avait alloue", Role::First),
    ("ayant alloue", Role::First),
    ("a fixe", Role::First),
    ("ont fixe", Role::First),
    ("avait fixe", Role::First),
    ("ayant fixe", Role::First),
    ("a evalue", Role::First),
    ("ont evalue", Role::First),
    ("ayant evalue", Role::First),
    ("a retenu", Role::First),
    ("ont retenu", Role::First),
    ("ayant retenu", Role::First),
    ("a indemnise", Role::First),
    ("ont indemnise", Role::First),
    ("a accorde", Role::First),
    ("ont accorde", Role::First),
    ("a octroye", Role::First),
    ("ont octroye", Role::First),
    ("a arbitre", Role::First),
    ("a chiffre", Role::First),
    ("a liquide", Role::First),
    ("a condamne", Role::First),
    ("par le tribunal", Role::First),
    ("par le premier juge", Role::First),
    ("par les premiers juges", Role::First),
    ("par le jugement", Role::First),
    ("ont condamne", Role::First),

    ("sollicite", Role::Claim),
    ("sollicitent", Role::Claim),
    ("sollicitant", Role::Claim),
    ("sollicitait", Role::Block),
    ("sollicitaient", Role::Block),
    ("demande", Role::Claim),
    ("demandent", Role::Claim),
    ("demandait", Role::Block),
    ("demandaient", Role::Block),
    ("reclame", Role::Claim),
    ("reclament", Role::Claim),
    ("reclamant", Role::Claim),
    ("reclamait", Role::Block),
    ("requiert", Role::Claim),
    ("entend voir", Role::Claim),
    ("conclut a l'allocation", Role::Claim),
    ("conclut a l'octroi", Role::Claim),
    ("concluent a l'octroi", Role::Claim),
    ("conclut a la fixation", Role::Claim),
    ("chiffre sa demande", Role::Claim),

    ("propose", Role::Offer),
    ("proposent", Role::Offer),
    ("proposant", Role::Offer),
    ("proposait", Role::Block),
    ("offre de verser", Role::Offer),
    ("offre la somme", Role::Offer),
    ("offre une somme", Role::Offer),
    ("a offert", Role::Offer),
    ("ont offert", Role::Offer),
    ("conclut au paiement", Role::Offer),
    ("de ramener", Role::Offer),
    ("soit ramene", Role::Offer),
    ("soit ramenee", Role::Offer),
    ("de limiter", Role::Offer),
    ("soit limite", Role::Offer),
    ("soit limitee", Role::Offer),
    ("de reduire", Role::Offer),
    ("soit reduit", Role::Offer),
    ("soit reduite", Role::Offer),
    ("n'est pas conteste", Role::Offer),
    ("n'est pas contestee", Role::Offer),
    ("ne conteste pas", Role::Offer),
    ("ne contestent pas", Role::Offer),

    ("l'expert a evalue", Role::Block),
    ("l'expert a chiffre", Role::Block),
    ("l'expert a retenu", Role::Block),
    ("l'expert a fixe", Role::Block),
    ("l'expert a estime", Role::Block),
    ("l'expert judiciaire a evalue", Role::Block),
    ("l'expert judiciaire a retenu", Role::Block),
    ("les experts ont evalue", Role::Block),
    ("les experts ont retenu", Role::Block),
    ("l'expert", Role::Block),
    ("les experts", Role::Block),
    ("rapport d'expertise", Role::Block),
    ("provision", Role::Block),
    ("provisions", Role::Block),
    ("provisionnelle", Role::Block),
    ("article 700", Role::Block),
    ("depens", Role::Block),
    ("creance", Role::Block),
    ("debours", Role::Block),
    ("indemnites journalieres", Role::Block),
    ("capital constitutif", Role::Block),
    ("sur la base de", Role::Block),
    ("a raison de", Role::Block),
    ("euro de rente", Role::Block),
    ("devis", Role::Block),
    ("facture", Role::Block),
    ("factures", Role::Block),
    ("bulletin", Role::Block),
    ("salaire", Role::Block),
    ("salaires", Role::Block),
    ("revenu annuel", Role::Block),
    ("revenus annuels", Role::Block),
    ("pension", Role::Block),
    ("smic", Role::Block),

    ("sera infirme", Role::Infirm),
    ("sera infirmee", Role::Infirm),
    ("convient d'infirmer", Role::Infirm),
    ("y a lieu d'infirmer", Role::Infirm),
    ("la cour infirme", Role::Infirm),
    ("sera reforme", Role::Infirm),
    ("sera reformee", Role::Infirm),
    ("convient de reformer", Role::Infirm),
    ("infirmant", Role::Infirm),
    ("reformant", Role::Infirm),

    ("sera deboute", Role::Reject),
    ("sera deboutee", Role::Reject),
    ("seront deboutes", Role::Reject),
    ("sera rejetee", Role::Reject),
    ("seront rejetees", Role::Reject),
    ("est rejetee", Role::Reject),
    ("n'y a pas lieu a indemnisation", Role::Reject),
    ("aucune somme ne sera allouee", Role::Reject),
    ("ne sera alloue aucune", Role::Reject),
    ("a deboute", Role::Reject),
    ("l'a deboute", Role::Reject),

    ("sursis a statuer", Role::Reserve),
    ("il sera sursis", Role::Reserve),
    ("sont reserves", Role::Reserve),
    ("seront reserves", Role::Reserve),
    ("expertise complementaire", Role::Reserve),
    ("avant dire droit", Role::Reserve),

    ("sollicite la confirmation", Role::Shadow),
    ("sollicitent la confirmation", Role::Shadow),
    ("demande la confirmation", Role::Shadow),
    ("demandent la confirmation", Role::Shadow),
    ("demande de confirmer", Role::Shadow),
    ("demandent de confirmer", Role::Shadow),
    ("sollicite de confirmer", Role::Shadow),
    ("propose de confirmer", Role::Shadow),
    ("proposent de confirmer", Role::Shadow),
    ("conclut a la confirmation", Role::Shadow),
    ("concluent a la confirmation", Role::Shadow),
    ("la demande", Role::Shadow),
    ("sa demande", Role::Shadow),
    ("ses demandes", Role::Shadow),
    ("leur demande", Role::Shadow),
    ("leurs demandes", Role::Shadow),
    ("cette demande", Role::Shadow),
    ("les demandes", Role::Shadow),
    ("des demandes", Role::Shadow),
    ("aux demandes", Role::Shadow),
    ("de la demande", Role::Shadow),
    ("une demande", Role::Shadow),
];

#[derive(Clone, Copy, Debug)]
struct Mark {
    s: usize,
    e: usize,
    role: Role,
}

fn automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .kind(Some(AhoCorasickKind::DFA))
            .build(MARKS.iter().map(|(s, _)| *s))
            .expect("quantum motifs automaton")
    })
}

/// Names of poste families the vocabulary refuses to code, « assistance par
/// tierce personne » or « préjudice esthétique » alone. In title position they
/// open nothing, but they close what runs above them.
fn families() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .kind(Some(AhoCorasickKind::DFA))
            .build(vocabulary::AMBIGUOUS.iter().map(|(s, _)| *s))
            .expect("quantum motifs families")
    })
}

/// Does a family name a poste where it stands? A single word does not: « son
/// état de santé » is not the dépenses de santé, « les frais » nothing at all.
fn compound(folded: &[char], at: usize) -> bool {
    vocabulary::AMBIGUOUS.iter().any(|(s, _)| {
        s.contains(' ') && text_of(folded, at, (at + s.chars().count()).min(folded.len())) == *s
    })
}

fn barriers(folded: &str, byte2char: &[usize]) -> Vec<(usize, &'static [&'static str])> {
    let mut out: Vec<(usize, &'static [&'static str])> = families()
        .find_iter(folded)
        .filter_map(|m| {
            let before = folded[..m.start()].chars().next_back();
            (!before.is_some_and(char::is_alphanumeric))
                .then(|| (byte2char[m.start()], vocabulary::AMBIGUOUS[m.pattern()].1))
        })
        .collect();
    for opener in RECAP {
        for (b, _) in folded.match_indices(opener) {
            out.push((byte2char[b], &[]));
        }
    }
    out.sort_by_key(|(b, _)| *b);
    out
}

/// A heading that opens another person's prejudices closes the section above
/// it: « En ce qui concerne le préjudice de M. A », under which the court
/// prices what it awards to him, not to the victim the section was about.
fn person_bars(doc: Doc, from: usize, ds: usize) -> Vec<usize> {
    const OF: [&str; 7] = [
        " de m. ",
        " de mme ",
        " de mlle ",
        " par m. ",
        " par mme ",
        " par mlle ",
        " subis par ",
    ];
    let (chars, breaks, folded) = (doc.chars, doc.breaks, doc.folded);
    let to = ds.min(folded.len());
    let text: String = folded[from.min(to)..to].iter().collect();
    let mut out = Vec::new();
    for (b, _) in text.match_indices("prejudice") {
        let at = from + text[..b].chars().count();
        if !heading(chars, breaks, folded, from, at) {
            continue;
        }
        let tail: String = folded[at..(at + 60).min(to)].iter().collect();
        if OF.iter().any(|w| tail.contains(w)) {
            out.push(at);
        }
    }
    out
}

fn marks(folded: &str, byte2char: &[usize]) -> Vec<Mark> {
    let mut out = Vec::new();
    for m in automaton().find_iter(folded) {
        let (bs, be) = (m.start(), m.end());
        let before = folded[..bs].chars().next_back();
        let after = folded[be..].chars().next();
        let glued = after.is_some_and(char::is_alphanumeric) && !folded[..be].ends_with('\'');
        if before.is_some_and(char::is_alphanumeric) || glued {
            continue;
        }
        let s = byte2char[bs];
        out.push(Mark {
            s,
            e: s + folded[bs..be].chars().count(),
            role: MARKS[m.pattern().as_usize()].1,
        });
    }
    out
}

fn text_of(folded: &[char], s: usize, e: usize) -> String {
    folded[s.min(e)..e.min(folded.len())].iter().collect()
}

/// The paragraph `at` sits in, bounded below by `from`. The scan maps every line
/// break to a space, so the break itself is read from `breaks`, and a run of two
/// spaces closes a paragraph in a text that arrived without them.
fn paragraph_start(chars: &[char], breaks: &[bool], at: usize, from: usize) -> usize {
    let mut i = at.min(chars.len());
    while i > from + 1 {
        if breaks[i - 1] || (chars[i - 1] == ' ' && chars[i - 2] == ' ') {
            return i;
        }
        i -= 1;
    }
    from
}

/// Where the paragraph `at` sits in closes, no later than `to`.
pub(super) fn paragraph_end(chars: &[char], breaks: &[bool], at: usize, to: usize) -> usize {
    let limit = to.min(chars.len());
    let mut i = at;
    while i + 1 < limit {
        if breaks[i] || (chars[i] == ' ' && chars[i + 1] == ' ') {
            return i;
        }
        i += 1;
    }
    limit
}

/// The sentence `at` belongs to, no earlier than `from`. A full stop closes one
/// only before a capital: « M. [K] » and « 1 867,50 » write their own.
fn sentence_start(chars: &[char], at: usize, from: usize) -> usize {
    let mut i = at.min(chars.len());
    while i > from + 2 {
        let (stop, sep, next) = (chars[i - 3], chars[i - 2], chars[i - 1]);
        if sep == ' ' && (stop == ';' || (matches!(stop, '.' | '!' | '?') && next.is_uppercase())) {
            return i - 1;
        }
        i -= 1;
    }
    from
}

/// A heading opens on numbering alone, or on the words that introduce a poste:
/// « 2.3.2) », « - », « Sur le », « S'agissant du ».
pub(super) fn heading(
    chars: &[char],
    breaks: &[bool],
    folded: &[char],
    from: usize,
    at: usize,
) -> bool {
    let ls = paragraph_start(chars, breaks, at, from);
    if at - ls > 60 {
        return false;
    }
    let prefix: Vec<char> = folded[ls..at].to_vec();
    let mut i = 0;
    while i < prefix.len() && !prefix[i].is_alphabetic() {
        i += 1;
    }
    let mut word = String::new();
    for &c in &prefix[i..] {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            if !word.is_empty() && !title_word(&word) {
                return false;
            }
            word.clear();
        }
    }
    word.is_empty() || title_word(&word)
}

/// « IV.2.4 Le déficit fonctionnel permanent : » : the administrative order
/// numbers every heading of its motifs, and the number is not a word.
fn title_word(word: &str) -> bool {
    TITLE_WORDS.contains(&word)
        || word.chars().all(|c| c.is_ascii_digit())
        || (word.len() <= 6 && word.chars().all(|c| "ivxlcdm".contains(c)))
}

/// The first heading of the motifs that opens the court's own discussion.
pub(super) fn discussion_start(
    chars: &[char],
    breaks: &[bool],
    folded: &[char],
    zs: usize,
    ds: usize,
) -> Option<usize> {
    let zone = text_of(folded, zs, ds);
    let mut best = None;
    for anchor in DISCUSSION {
        for (b, _) in zone.match_indices(anchor) {
            let at = zs + zone[..b].chars().count();
            let ls = paragraph_start(chars, breaks, at, zs);
            if folded[ls..at].iter().all(|c| !c.is_alphanumeric()) && best.is_none_or(|x| at < x) {
                best = Some(at);
            }
        }
    }
    best
}

/// « S'agissant du préjudice particulier tiré de l'altération du lien avec son
/// nouveau-né : » : a heading that opens a head of damage the vocabulary does
/// not know still closes the section above it, unless it names the same poste.
pub(super) fn foreign_opener(
    doc: Doc,
    heads: &[(usize, usize, &'static str)],
    code: &'static str,
    from: usize,
    to: usize,
) -> Option<usize> {
    let (chars, breaks, folded) = (doc.chars, doc.breaks, doc.folded);
    let mut p = from + 1;
    while p < to {
        let starts = breaks[p - 1] || (chars[p - 1] == ' ' && p >= 2 && chars[p - 2] == ' ');
        if !starts {
            p += 1;
            continue;
        }
        let pe = paragraph_end(chars, breaks, p, to);
        let text = text_of(folded, p, pe);
        let text = text.trim_start_matches(|c: char| !c.is_alphanumeric());
        let opens = OPENERS.iter().any(|w| text.starts_with(w));
        let damage = DAMAGE_WORDS.iter().any(|w| text.contains(w));
        let same = heads
            .iter()
            .any(|(hs, _, c)| *c == code && *hs >= p && *hs < pe);
        if opens && damage && !same && pe - p <= 200 {
            return Some(p);
        }
        p = pe.max(p + 1);
    }
    None
}

/// A sentence that opens on a poste to deny it any compensation is not the
/// title of its section.
pub(super) fn refusing(chars: &[char], breaks: &[bool], folded: &[char], from: usize) -> bool {
    let to = paragraph_end(chars, breaks, from, (from + 160).min(folded.len()));
    let sentence = text_of(folded, from, to);
    let sentence = sentence.split([':', ';', '.']).next().unwrap_or("");
    NEGATIONS
        .iter()
        .filter_map(|n| sentence.find(n).map(|i| &sentence[i + n.len()..]))
        .any(|after| COMPENSATION.iter().any(|w| after.contains(w)))
}

const NEGATIONS: [&str; 8] = [
    "ne peut ",
    "ne peuvent ",
    "ne saurait ",
    "ne sauraient ",
    "n'ouvre ",
    "n'ouvrent ",
    "ne donne ",
    "ne donnent ",
];

const COMPENSATION: [&str; 4] = ["indemnis", "repar", "faire l'objet", "droit a"];

/// The paragraph as the text lays it out: a blank line, or a line that opens
/// on the number of the next numbered paragraph. A wrapped line inside a
/// sentence closes nothing.
pub(super) fn hard_paragraph_end(doc: Doc, at: usize, to: usize) -> usize {
    let limit = to.min(doc.chars.len());
    let mut i = at;
    while i + 1 < limit {
        let blank = (doc.breaks[i] && doc.breaks[i + 1])
            || (doc.chars[i] == ' ' && doc.chars[i + 1] == ' ');
        if blank || (doc.breaks[i] && numbered_line(doc.chars, i + 1)) {
            return i;
        }
        i += 1;
    }
    limit
}

pub(super) fn hard_paragraph_start(doc: Doc, at: usize, from: usize) -> usize {
    let mut i = at.min(doc.chars.len().saturating_sub(1));
    while i > from + 1 {
        let blank = (doc.breaks[i - 1] && doc.breaks[i])
            || (doc.chars[i - 1] == ' ' && doc.chars[i] == ' ');
        if blank || (doc.breaks[i - 1] && numbered_line(doc.chars, i)) {
            return i;
        }
        i -= 1;
    }
    from
}

/// « 21. » or « 21) » opening a line.
fn numbered_line(chars: &[char], from: usize) -> bool {
    let digits = chars[from.min(chars.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    (1..=3).contains(&digits) && matches!(chars.get(from + digits), Some('.') | Some(')'))
}

const OPENERS: [&str; 12] = [
    "s'agissant",
    "sur le ",
    "sur la ",
    "sur les ",
    "sur l'",
    "en ce qui concerne",
    "quant a ",
    "quant au",
    "concernant le",
    "concernant la",
    "concernant les",
    "concernant l'",
];

const DAMAGE_WORDS: [&str; 9] = [
    "prejudice",
    "frais",
    "perte",
    "deficit",
    "incidence",
    "assistance",
    "depenses",
    "souffrances",
    "dommage",
];

/// The three views of the document a section walks: its chars, the line breaks
/// of the text it was built from, and its folded form.
#[derive(Clone, Copy)]
pub(super) struct Doc<'a> {
    pub(super) chars: &'a [char],
    pub(super) breaks: &'a [bool],
    pub(super) folded: &'a [char],
}

/// One poste of the motifs: the heading that names it, and the text down to the
/// next heading.
struct Section {
    code: &'static str,
    label: (usize, usize),
    s: usize,
    e: usize,
}

fn sections(
    doc: Doc,
    labels: &[CueTok],
    bars: &[(usize, &'static [&'static str])],
    from: usize,
    ds: usize,
) -> Vec<Section> {
    let (chars, breaks, folded) = (doc.chars, doc.breaks, doc.folded);
    let heads: Vec<(usize, usize, &'static str)> = labels
        .iter()
        .filter_map(|c| match c.cue {
            // « - des souffrances endurees cotees 3/7, » enumerates the expert's
            // findings: a title never carries the cotation of its poste.
            Cue::Poste(code)
                if heading(chars, breaks, folded, from, c.s)
                    && !text_of(folded, c.e, (c.e + 8).min(folded.len())).starts_with(" cote")
                    && !refusing(chars, breaks, folded, c.e) =>
            {
                Some((c.s, c.e, code))
            }
            _ => None,
        })
        .collect();
    let mut out: Vec<Section> = Vec::new();
    let mut ricochet = false;
    let mut i = 0;
    while i < heads.len() {
        let (s, e, code) = heads[i];
        let mut j = i + 1;
        while j < heads.len() && heads[j].2 == code {
            j += 1;
        }
        // The heading of the block, which is the paragraph above the label.
        let head = paragraph_start(chars, breaks, s, from);
        let intro = text_of(
            folded,
            paragraph_start(chars, breaks, head.saturating_sub(2).max(from), from),
            s,
        );
        ricochet = match vocabulary::direct_side(code) {
            Some(true) => false,
            Some(false) => true,
            None => ricochet,
        } || (intro.len() < 160 && RICOCHET.iter().any(|w| intro.contains(w)));
        if !ricochet {
            let body = heads[j - 1].1;
            // A family that could name this poste says nothing new: only the
            // title of another family closes the section.
            let bar = bars
                .iter()
                .find(|(b, codes)| {
                    *b > body && !codes.contains(&code) && heading(chars, breaks, folded, from, *b)
                })
                .map_or(ds, |(b, _)| *b);
            let end = heads
                .get(j)
                .map_or(ds, |h| h.0)
                .min(body + SECTION)
                .min(bar);
            let foreign = foreign_opener(doc, &heads, code, body, end).unwrap_or(end);
            out.push(Section {
                code,
                label: (s, e),
                s: body,
                e: end.min(foreign),
            });
        }
        i = j;
    }
    out
}

/// What one section of the motifs says of its poste.
#[derive(Default)]
struct Read {
    alloue: Option<f64>,
    premiere: Option<f64>,
    refused: bool,
    /// The poste and the share the court leaves the victim, for each reduction
    /// the section writes as an operation.
    reduit: Vec<(f64, f64)>,
}

/// « la somme de 30 375 x 75 % = 22 781,25 euros » : the court prices the poste,
/// then takes the share of liability off it. The dispositif carries the result
/// alone, and only this operation says what it is a share of. The left operand
/// is read from the raw digits: written without its currency, it is no amount.
fn reduction_pairs(doc: Doc, amounts: &[Amount], sec: &Section) -> Vec<(f64, f64)> {
    const TIMES: [char; 3] = [' ', 'x', ' '];
    amounts
        .iter()
        .filter(|a| a.start >= sec.s && a.end <= sec.e)
        .filter_map(|b| {
            let from = b.start.saturating_sub(45).max(sec.s);
            let lead = &doc.folded[from..b.start];
            let eq = lead.iter().rposition(|c| *c == '=')?;
            let pct = lead[..eq].iter().rposition(|c| *c == '%')?;
            let x = lead[..pct].windows(3).rposition(|w| w == TIMES)?;
            let rate = jetons(&lead[x..pct]).pop()?;
            // The left operand sits right against the sign, its currency word
            // apart: written without one it is no amount and no token of theirs.
            let brut = jetons(&lead[..x]).pop().filter(|j| {
                let gap = text_of(lead, j.fin, x);
                let gap = gap.trim();
                gap.is_empty()
                    || gap == "\u{20ac}"
                    || leftovers::currency_word(lead, j.fin).is_some_and(|w| gap == w)
            })?;
            let (brut, part) = (brut.valeur as f64 / 1000.0, b.value as f64 / 1000.0);
            (rate.valeur <= 100_000 && part < brut - EPS).then_some((brut, part))
        })
        .collect()
}

/// « la somme de 7 513 euros allouée par le premier juge sera confirmée » : the
/// figure opens its sentence and the verb that owns it follows.
fn following(marks: &[Mark], amounts: &[Amount], at: usize, to: usize) -> Option<Role> {
    let next = marks
        .iter()
        .filter(|m| m.s >= at && m.e <= to && m.role.governs())
        .min_by_key(|m| m.s)?;
    let split = amounts.iter().any(|a| a.start >= at && a.end <= next.s);
    (!split && next.role == Role::First).then_some(Role::First)
}

/// The mark that says whose the amount at `at` is: the last one of its sentence,
/// no other amount standing between the two. A party's claim swallows what
/// follows it in the sentence, except the court's own award verb: « sollicite
/// l'infirmation du jugement et l'allocation de la somme de X » is a claim.
fn governing<'m>(
    marks: &'m [Mark],
    amounts: &[Amount],
    rates: &[usize],
    from: usize,
    at: usize,
) -> Option<(Role, &'m Mark)> {
    let inside = || {
        marks
            .iter()
            .filter(|m| m.role.governs() && m.s >= from && m.e <= at)
    };
    let last = inside().max_by_key(|m| m.s)?;
    let between = amounts
        .iter()
        .any(|a| a.start >= last.e && a.end < at && !rates.contains(&a.start));
    if at - last.e > GOVERN || between {
        return None;
    }
    if !matches!(last.role, Role::Block | Role::Confirm) {
        return Some((last.role, last));
    }
    let claim = inside()
        .filter(|m| matches!(m.role, Role::Claim | Role::Offer))
        .max_by_key(|m| m.s);
    Some((claim.map_or(last.role, |c| c.role), last))
}

/// Does the sentence price something other than this poste in full: the share a
/// reduction leaves, a third-party payer's line, the whole of the harm?
fn aside(sentence: &str) -> bool {
    REDUCED
        .iter()
        .chain(&NOT_A_POSTE)
        .any(|w| sentence.contains(w))
}

/// The one value a role prices in the section. Several disagreeing readings
/// price nothing in the sure mode, and the last one in the extended mode, the
/// court closing its section on its own figure.
fn settle(values: &[f64], mode: Mode) -> Option<f64> {
    let first = *values.first()?;
    if values.iter().all(|v| (v - first).abs() < EPS) {
        return Some(first);
    }
    total_of(values)
        .or_else(|| (mode == Mode::Extended).then(|| *values.last().expect("non empty")))
}

/// One figure of the section totals the others: the court wrote the poste and
/// the components it cuts into, and the total is the poste.
fn total_of(values: &[f64]) -> Option<f64> {
    if values.len() < 3 {
        return None;
    }
    let whole: f64 = values.iter().sum();
    values
        .iter()
        .find(|t| ((whole - **t) - **t).abs() < EPS)
        .copied()
}

/// « 2 245 euros le point …, soit une somme globale de 44 900 euros » : the
/// unit price the court multiplies is not the poste, and what it multiplies it
/// by is written in words. Read once to refuse the figure, once to let the
/// court's verb reach the result across it.
fn rated(doc: Doc, a: &Amount, sec: &Section) -> bool {
    let (chars, breaks, folded) = (doc.chars, doc.breaks, doc.folded);
    let para = paragraph_start(chars, breaks, a.start, sec.s);
    let clause = sentence_start(chars, a.start, para);
    let lead = text_of(folded, a.start.saturating_sub(40).max(clause), a.start);
    if RESULT.iter().any(|w| lead.contains(w)) {
        return false;
    }
    let tail = (a.end + 40)
        .min(paragraph_end(chars, breaks, a.end, sec.e))
        .min(sentence_end(chars, a.end, sec.e));
    let rate = text_of(folded, a.start.saturating_sub(90).max(clause), tail);
    UNIT_MARKS.iter().any(|w| rate.contains(w))
}

/// Where the sentence holding `at` closes, bounded above by `to`.
fn sentence_end(chars: &[char], at: usize, to: usize) -> usize {
    let limit = to.min(chars.len());
    let mut i = at;
    while i + 2 < limit {
        let (stop, sep, next) = (chars[i], chars[i + 1], chars[i + 2]);
        if sep == ' ' && (stop == ';' || (matches!(stop, '.' | '!' | '?') && next.is_uppercase())) {
            return i + 1;
        }
        i += 1;
    }
    limit
}

/// What a court writes in the cell of a poste it refuses, in a recapitulation
/// as in a table row.
const REFUSED_CELL: [&str; 12] = [
    "deboute",
    "deboutee",
    "deboutes",
    "deboutees",
    "rejet",
    "rejete",
    "rejetee",
    "rejetes",
    "rejetees",
    "neant",
    "sans objet",
    "non retenu",
];

/// « Préjudice sexuel (PS) : débouté », « Préjudice d'agrément rejet » : the
/// court names the poste to refuse it, and the sum the next sentence pays is
/// another poste's. Read on the cell, not on a verb: the line carries no verb.
fn refused_cell(folded: &[char], sec: &Section) -> bool {
    let raw = text_of(folded, sec.label.1, (sec.label.1 + 40).min(sec.e));
    let tail = raw.trim_start();
    let tail = match (tail.starts_with('('), tail.find(')')) {
        (true, Some(i)) => tail[i + 1..].trim_start(),
        _ => tail,
    };
    let tail = tail.trim_start_matches([':', '-', ' ', ',', '.']);
    REFUSED_CELL
        .iter()
        .any(|w| tail.starts_with(w) && !tail[w.len()..].starts_with(|c: char| c.is_alphanumeric()))
}

fn read(
    doc: Doc,
    labels: &[CueTok],
    bars: &[(usize, &'static [&'static str])],
    marks: &[Mark],
    amounts: &[Amount],
    sec: &Section,
    mode: Mode,
) -> Option<Read> {
    let (chars, breaks, folded) = (doc.chars, doc.breaks, doc.folded);
    let inside: Vec<&Mark> = marks
        .iter()
        .filter(|m| m.s >= sec.s && m.e <= sec.e)
        .collect();
    if inside.iter().any(|m| m.role == Role::Reserve) {
        return None;
    }
    if refused_cell(folded, sec) {
        return Some(Read {
            alloue: (mode != Mode::Sure).then_some(0.0),
            premiere: None,
            refused: true,
            reduit: Vec::new(),
        });
    }
    let infirmed = inside.iter().any(|m| m.role == Role::Infirm);
    let rates: Vec<usize> = amounts
        .iter()
        .filter(|a| a.start >= sec.s && a.end <= sec.e && rated(doc, a, sec))
        .map(|a| a.start)
        .collect();
    let (mut awards, mut firsts) = (Vec::new(), Vec::new());
    // « 8. […] Le préjudice esthétique temporaire […] sera estimé à 200 euros.
    // 9. Il est estimé à […] 100 euros » : once a paragraph of the section
    // opens a sentence on another poste and prices it, what the next ones
    // price is not this one.
    let foreign_para = labels
        .iter()
        .filter(|c| {
            c.s >= sec.s && c.s < sec.e && !matches!(c.cue, Cue::Poste(code) if code == sec.code)
        })
        .map(|c| (c, paragraph_start(chars, breaks, c.s, sec.s)))
        .filter(|(c, para)| {
            c.s - sentence_start(chars, c.s, *para) <= 4
                && amounts
                    .iter()
                    .any(|a| a.start >= c.e && a.end <= paragraph_end(chars, breaks, c.e, sec.e))
        })
        .map(|(_, para)| para)
        .min();
    // The court adopts a figure of the judgment when it confirms it where it
    // recites it: a confirmation elsewhere in the section confirms something else.
    let mut confirmed: Vec<f64> = Vec::new();
    for a in amounts
        .iter()
        .filter(|a| a.start >= sec.s && a.end <= sec.e)
    {
        let para = paragraph_start(chars, breaks, a.start, sec.s);
        if mode == Mode::Sure && foreign_para.is_some_and(|f| para > f) {
            continue;
        }
        let clause = sentence_start(chars, a.start, para);
        let sentence = text_of(folded, clause, (a.end + 45).min(folded.len()));
        if GROUPS
            .iter()
            .any(|g| text_of(folded, clause, a.start).contains(g))
        {
            continue;
        }
        // The next heading opens one char after the figure that closes the
        // section: what takes a figure with it is a label of the same paragraph.
        let tail = (a.end + 40).min(paragraph_end(chars, breaks, a.end, sec.e));
        if rated(doc, a, sec) {
            continue;
        }
        // A label of another poste in the paragraph takes the figure with it.
        if labels.iter().any(|c| {
            c.s >= para && c.s < tail && !matches!(c.cue, Cue::Poste(code) if code == sec.code)
        }) {
            continue;
        }
        let value = a.value as f64 / 1000.0;
        let hanger = text_of(folded, a.end, (a.end + 45).min(folded.len()));
        let hung = HANGERS.iter().any(|h| hanger.contains(h))
            && !ANAPHORA.iter().any(|w| hanger.contains(w))
            && !labels
                .iter()
                .any(|c| c.s >= a.end && c.s < a.end + 90 && c.cue == Cue::Poste(sec.code));
        if hung {
            continue;
        }
        // « 473 x 18 x 2 h = 17 028 euros », « 4 000 euros (8 000 x 50 %) » : a
        // step of the court's arithmetic, which its own total closes.
        let step = text_of(
            folded,
            a.start.saturating_sub(25),
            (a.end + 30).min(folded.len()),
        );
        if step.contains('=') || step.contains(" x ") {
            continue;
        }
        let gov = governing(marks, amounts, &rates, clause, a.start);
        let voice = gov
            .map(|(role, _)| role)
            .or_else(|| following(marks, amounts, a.end, (a.end + 70).min(sec.e)));
        // What stands between the court's verb and its figure says whether the
        // figure is still the poste; what precedes the verb is the recital, and
        // an administrative decision recites the expert in the very sentence
        // that awards.
        let veil = gov.map_or_else(|| sentence.clone(), |(_, m)| text_of(folded, m.e, a.start));
        // « Après application du taux de perte de chance, il y a lieu de
        // condamner … la somme de 25 euros » : the verb awards a share.
        let cut_down = mode == Mode::Sure
            && gov.is_some_and(|(_, m)| {
                let lead = text_of(folded, clause, m.s);
                CUT_DOWN.iter().any(|w| lead.contains(w))
            });
        // A family that cannot name this poste, standing between the verb and
        // the figure, is what the verb prices: « de ce préjudice esthétique en
        // fixant son indemnisation à 200 euros », under souffrances endurées.
        // An anaphor reaches back to the family the paragraph itself named, and
        // only a compound names one: « son état de santé » names nothing.
        if let Some((_, m)) = gov {
            let anaphor = ANAPHORA.iter().any(|w| veil.contains(w));
            let back = if anaphor { para } else { m.e };
            if bars.iter().any(|(b, codes)| {
                *b >= back
                    && *b < a.start
                    && !codes.is_empty()
                    && !codes.contains(&sec.code)
                    && (*b >= m.e || compound(folded, *b))
            }) {
                continue;
            }
        }
        // « d'évaluer à la somme de 1 615 218,47 euros l'indemnisation … dans
        // le cadre des préjudices examinés en appel » : a plural object prices
        // them all.
        let object = text_of(
            folded,
            a.end,
            (a.end + 120).min(paragraph_end(chars, breaks, a.end, sec.e)),
        );
        let object = object.split(['.', ';']).next().unwrap_or("");
        if veil.contains("prejudices") || object.contains("prejudices") {
            continue;
        }
        match voice {
            Some(Role::Award) if !aside(&veil) && !cut_down => awards.push(value),
            Some(Role::Confirm | Role::First) if !aside(&sentence) => {
                firsts.push(value);
                let close = paragraph_end(chars, breaks, a.end, sec.e);
                if !infirmed
                    && !inside.iter().any(|m| m.role == Role::Reject)
                    && inside
                        .iter()
                        .any(|m| m.role == Role::Confirm && m.s >= para && m.e <= close)
                {
                    confirmed.push(value);
                }
            }
            _ => {}
        }
    }
    let priced = amounts
        .iter()
        .filter(|a| a.start >= sec.s && a.end <= sec.e)
        .count();
    let sure = mode == Mode::Sure;
    let settled = settle(&awards, mode).filter(|_| {
        !sure || total_of(&awards).is_some() || (awards.len() == 1 && priced <= AMOUNTS_PER_SECTION)
    });
    let premiere = settle(&firsts, mode);
    // A poste the court refuses outright is a poste at zero, and only a section
    // that prices nothing at all refuses outright.
    let refused = priced == 0 && inside.iter().any(|m| m.role == Role::Reject);
    let alloue = match settled {
        Some(v) => Some(v),
        None if mode == Mode::Sure => None,
        None if refused => Some(0.0),
        None => settle(&confirmed, mode),
    };
    Some(Read {
        alloue,
        premiere,
        reduit: reduction_pairs(doc, amounts, sec),
        refused: refused && settled.is_none(),
    })
}

/// One field over every section that names the poste.
fn across(reads: &[Read], field: impl Fn(&Read) -> Option<f64>, mode: Mode) -> Option<f64> {
    let values: Vec<f64> = reads.iter().filter_map(field).collect();
    settle(&values, mode)
}

/// The victim the motifs liquidate for: the single direct one of the record.
fn target(x: &mut Extraction, mode: Mode) -> Option<usize> {
    let directs: Vec<usize> = x
        .quantum
        .victimes
        .iter()
        .enumerate()
        .filter(|(_, v)| v.role == "directe")
        .map(|(i, _)| i)
        .collect();
    match directs.as_slice() {
        [k] => Some(*k),
        [] if mode == Mode::Extended => {
            let cle = format!("v{}", x.quantum.victimes.len() + 1);
            x.quantum.victimes.push(Victime {
                cle,
                role: "directe".to_string(),
                ..Default::default()
            });
            Some(x.quantum.victimes.len() - 1)
        }
        _ => None,
    }
}

/// Codes a section of the motifs never writes on the direct victim: a ricochet
/// poste belongs to a proche the record holds under no name.
fn direct_code(code: &str) -> bool {
    vocabulary::direct_side(code) != Some(false) && !matches!(code, "PR" | "FO")
}

pub fn apply(scan: &DocScan, x: &mut Extraction, mode: Mode) {
    let (ds, _) = super::dispositif_span(scan);
    // Under a reduction, what the court writes on a poste is as often the share
    // left to the victim as the poste itself: the sure mode leaves that cell.
    let reduced = mode == Mode::Sure
        && super::dispositif::reduction_cue(&scan.folded().chars().collect::<Vec<char>>());
    let zs = scan.motifs_start();
    if zs >= ds {
        return;
    }
    let folded: Vec<char> = scan.folded().chars().collect();
    let chars = scan.chars();
    let breaks = scan.breaks();
    let Some(from) = discussion_start(chars, breaks, &folded, zs, ds) else {
        return;
    };
    let labels: Vec<CueTok> = vocabulary::cues(scan.folded(), scan.byte2char(), false)
        .into_iter()
        .filter(|c| c.s >= from && c.e <= ds && matches!(c.cue, Cue::Poste(_)))
        .collect();
    let mut bars = barriers(scan.folded(), scan.byte2char());
    let doc = Doc {
        chars,
        breaks,
        folded: &folded,
    };
    bars.extend(
        person_bars(doc, from, ds)
            .into_iter()
            .map(|b| (b, &[] as &[&str])),
    );
    bars.sort_by_key(|(b, _)| *b);
    let sections = sections(doc, &labels, &bars, from, ds);
    if sections.is_empty() {
        return;
    }
    let marks = marks(scan.folded(), scan.byte2char());
    let amounts: Vec<Amount> = leftovers::amounts_in(scan.chars(), &folded)
        .into_iter()
        .filter(|a| a.start >= from && a.end <= ds)
        .collect();

    let mut reads: Vec<(&Section, Vec<Read>)> = Vec::new();
    for sec in &sections {
        if !direct_code(sec.code) {
            continue;
        }
        let Some(r) = read(doc, &labels, &bars, &marks, &amounts, sec, mode) else {
            continue;
        };
        match reads.iter_mut().find(|(s, _)| s.code == sec.code) {
            Some((_, rs)) => rs.push(r),
            None => reads.push((sec, vec![r])),
        }
    }
    // « assistance par tierce personne » in title position, under no section,
    // is a second award the record sums into the frais divers, which gather
    // before the consolidation what the vocabulary codes elsewhere.
    let open_fd = bars.iter().any(|(b, codes)| {
        codes.contains(&"FD")
            && *b >= from
            && *b < ds
            && !sections.iter().any(|s| *b >= s.s && *b < s.e)
            && heading(chars, breaks, &folded, from, *b)
    });
    // What the dispositif gives to a caisse is no poste of anyone: « FIXE la
    // créance de la CPAM à la somme de 148 161,77 euros » is the sum a section
    // on the préjudice sexuel, two lines above, took for its own. The
    // dispositif settles, and the motifs do not take it back.
    let to_caisse: Vec<f64> = x
        .obligations
        .iter()
        .filter(|o| matches!(o.nature, Nature::HorsRecord("tiers_payeur")))
        .map(|o| o.montant)
        .collect();
    let Some(k) = target(x, mode) else { return };
    let v = &mut x.quantum.victimes[k];
    for (sec, mut rs) in reads {
        if mode == Mode::Sure && sec.code == "FD" && open_fd {
            continue;
        }
        // A poste refused under one heading and priced under another is priced.
        if rs.len() > 1 && rs.iter().any(|r| !r.refused) {
            rs.retain(|r| !r.refused);
        }
        // Two sections of one code that price it differently prove nothing.
        let r = Read {
            alloue: across(&rs, |r| r.alloue, mode)
                .filter(|v| to_caisse.iter().all(|c| (c - v).abs() > EPS)),
            premiere: across(&rs, |r| r.premiere, mode),
            reduit: rs.iter().flat_map(|r| r.reduit.iter().copied()).collect(),
            refused: false,
        };
        let position = v
            .postes
            .iter()
            .position(|p| p.poste.as_deref() == Some(sec.code));
        let p = match position {
            Some(i) => &mut v.postes[i],
            None if r.alloue.is_some() && sec.code != "HORS_NOMENCLATURE" => {
                v.postes.push(Poste {
                    poste: Some(sec.code.to_string()),
                    libelle_decision: Some(scan.verbatim(sec.label.0, sec.label.1).to_lowercase()),
                    ..Default::default()
                });
                v.postes.last_mut().expect("just pushed")
            }
            None => continue,
        };
        // The dispositif writes the share alone: the operation says which poste
        // it is a share of, and the record holds both.
        let held = |v: Option<f64>, part: f64| v.is_some_and(|w| (w - part).abs() < EPS);
        let pair = r.reduit.iter().find(|(_, part)| {
            held(p.montant_alloue, *part)
                || held(p.montant_du, *part)
                || held(p.part_victime, *part)
        });
        if let Some((brut, part)) = pair.copied() {
            p.montant_alloue = Some(brut);
            if p.part_victime.is_none() {
                p.part_victime = Some(part);
            }
        }
        // The judgment's figure is recited as often as it is adopted, and the
        // section alone does not always tell the two apart: the sure mode leaves
        // that cell to the reader that comes after it.
        let premiere = r.premiere.filter(|_| mode == Mode::Extended);
        // Under a reduction the poste and the share part company: a figure the
        // motifs write and the dispositif already read as the share is the share.
        let alloue = r.alloue.filter(|v| {
            !reduced
                || p.part_victime
                    .into_iter()
                    .chain(p.montant_du)
                    .all(|w| (w - v).abs() > EPS)
        });
        for (field, value) in [
            (&mut p.montant_alloue, alloue),
            (&mut p.alloue_premiere_instance, premiere),
        ] {
            if field.is_none() {
                *field = value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::dispositif::extract;
    use super::*;
    use lj_extract::scan::scan;

    const HEAD: &str = "COUR D'APPEL\n\nARRÊT\n\nEXPOSÉ DU LITIGE\n\nM. [D] sollicite 40 000 euros au titre des souffrances endurées.\n\nMOTIFS DE LA DÉCISION\n\n";
    /// The dispositif builds the direct victim the motifs then write on, and the
    /// poste it names there stays as it left it.
    const TAIL: &str = "\n\nPAR CES MOTIFS\nLa cour,\nFixe le préjudice de M. [D] comme suit : dépenses de santé actuelles : 500 euros ;";

    fn postes(text: &str, mode: Mode) -> Vec<(String, Option<f64>, Option<f64>)> {
        let x = extract(&scan(text), mode);
        x.quantum
            .victimes
            .iter()
            .flat_map(|v| v.postes.iter())
            .map(|p| {
                (
                    p.poste.clone().unwrap_or_default(),
                    p.montant_alloue,
                    p.alloue_premiere_instance,
                )
            })
            .collect()
    }

    fn poste(text: &str, mode: Mode, code: &str) -> Option<(Option<f64>, Option<f64>)> {
        postes(text, mode)
            .into_iter()
            .find(|(c, _, _)| c == code)
            .map(|(_, a, p)| (a, p))
    }

    const TA: &str = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\n";
    const TA_TAIL: &str = "\nD E C I D E :\nArticle 1er : Le centre hospitalier est condamné à verser à Mme A la somme de 788,25 euros.";

    #[test]
    fn a_paragraph_naming_another_poste_closes_the_section_for_what_follows() {
        let text = format!(
            "{TA}Sur le préjudice :\n7. Les souffrances endurées par Mme A sur les plans physique et psychique, évaluées à 0,5 sur une échelle de 7 par l'expert, doivent être estimées à 500 euros.\n8. L'accident a causé à Mme A des dermabrasions et a nécessité des soins locaux. Le préjudice esthétique temporaire en résultant, évalué à 1 sur une échelle de 7 par l'expert, sera estimé à 200 euros.\n9. Le seul préjudice permanent subi par la victime est esthétique et consiste en une cicatrice discrète sur le coude gauche. Il est estimé à 0,5 sur 7 par l'expert et sera indemnisé à hauteur de 100 euros.{TA_TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "SE"), Some((Some(500.0), None)));
        let all = postes(&text, Mode::Sure);
        assert!(all.iter().all(|(_, a, _)| *a != Some(100.0)), "{all:?}");
    }

    #[test]
    fn the_share_left_after_a_loss_of_chance_is_not_the_poste() {
        let text = format!(
            "{TA}S'agissant du préjudice esthétique permanent :\n19. Le préjudice esthétique permanent de Mme A a été évalué à 0,5 sur 7 par l'expert, compte tenu également du port de contentions veineuses. Il sera fait une juste appréciation de ce préjudice en l'évaluant à la somme de 500 euros. Après application du taux de perte de chance, il y a lieu de condamner le centre hospitalier à verser à l'intéressée la somme de 25 euros à ce titre.\nS'agissant du préjudice sexuel :\n20. Il ne résulte pas de l'instruction que Mme A subirait un tel préjudice.{TA_TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "PEP"), Some((Some(500.0), None)));
    }

    #[test]
    fn the_global_sum_closing_the_motifs_is_not_the_last_poste_read() {
        let text = format!(
            "{TA}Sur le préjudice d'impréparation :\n28. Mme A n'établit pas avoir été privée d'une information, et les conclusions tendant à l'indemnisation du préjudice d'impréparation doivent être rejetées.\n29. Il résulte de l'ensemble de ce qui précède qu'il y a lieu de condamner le centre hospitalier à verser à Mme A la somme globale de 90 742,68 euros, déduction faite de la provision de 30 000 euros.\nD E C I D E :\nArticle 1er : Le centre hospitalier est condamné à verser à Mme A la somme de 90 742,68 euros, déduction faite de la provision de 30 000 euros allouée par les assureurs."
        );
        let all = postes(&text, Mode::Extended);
        assert!(all.iter().all(|(_, a, _)| *a != Some(90_742.68)), "{all:?}");
    }

    #[test]
    fn a_sum_the_dispositif_gives_to_a_caisse_is_no_poste_of_the_motifs() {
        let caisse = format!(
            "{HEAD}Sur le préjudice sexuel\n\nIl sera alloué à M. [D] la somme de 148 161,77 euros.\n\nPAR CES MOTIFS\nLa cour,\nFIXE la créance de la CPAM à la somme de 148 161,77 euros ;\nCONDAMNE la société AXA à payer à M. [D] la somme de 68 509,68 euros ;"
        );
        assert_eq!(poste(&caisse, Mode::Sure, "PS"), None);
        // The same passage, the claim carrying another figure: nothing says
        // the 148 161,77 are the caisse's any more.
        let autre = caisse.replace(
            "FIXE la créance de la CPAM à la somme de 148 161,77 euros",
            "FIXE la créance de la CPAM à la somme de 12 000 euros",
        );
        assert_eq!(
            poste(&autre, Mode::Sure, "PS"),
            Some((Some(148_161.77), None))
        );
    }

    #[test]
    fn a_heading_opens_a_section_the_award_verb_prices() {
        let text = format!(
            "{HEAD}Sur les souffrances endurées\n\nL'expert les cote à 4/7. Il sera alloué à M. [D] la somme de 25 000 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "SE"), Some((Some(25000.0), None)));
    }

    #[test]
    fn a_numbered_heading_closes_the_section_above_it() {
        let text = format!(
            "{HEAD}IV.2.3 Le préjudice esthétique :\n\n23. Le préjudice esthétique temporaire a été coté 3/7 par les experts. Il sera fait une juste appréciation de ce poste en l'évaluant à la somme de 8 000 euros.\n\nIV.2.4 Le déficit fonctionnel permanent :\n\n24. Il a été fixé à 50 % par l'expert. Il sera fait une juste appréciation du préjudice en l'évaluant à la somme de 170 000 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "PET"), Some((Some(8_000.0), None)));
        assert_eq!(
            poste(&text, Mode::Sure, "DFP"),
            Some((Some(170_000.0), None))
        );
    }

    #[test]
    fn an_anaphor_takes_the_family_the_paragraph_named() {
        let text = format!(
            "{HEAD}S'agissant du déficit fonctionnel permanent :\n\n9. Compte tenu du taux d'IPP de 20 %, il y a lieu d'indemniser ce chef de préjudice à hauteur de 2 245 euros le point, soit une somme globale de 44 900 euros.\n\n10. M. [D] a au bras une cicatrice de 9 cm. Le préjudice esthétique a été évalué à 0,5/7 par l'expert. Il en sera fait une juste appréciation de ce préjudice en fixant son indemnisation à 500 euros.{TAIL}"
        );
        assert_eq!(
            poste(&text, Mode::Sure, "DFP"),
            Some((Some(44_900.0), None))
        );
    }

    #[test]
    fn a_family_between_the_verb_and_the_figure_takes_it() {
        let text = format!(
            "{HEAD}Sur les souffrances endurées\n\n6. Elles sont évaluées par l'expert à 4 sur une échelle de 7. Il y a lieu d'indemniser ce chef de préjudice à hauteur de 8 000 euros.\n\n7. M. [D] a dû porter une attelle pendant 25 jours. Il en sera fait une juste appréciation de ce préjudice esthétique en fixant son indemnisation à 200 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "SE"), Some((Some(8_000.0), None)));
    }

    #[test]
    fn a_family_outside_the_sections_leaves_the_frais_divers_open() {
        let text = format!(
            "{HEAD}S'agissant des frais divers\n\nIl sera fait une juste appréciation des frais de déplacement en les évaluant à 260 euros.\n\nS'agissant de l'assistance par une tierce personne\n\nIl sera alloué à ce titre la somme de 1 100 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "FD"), None);
    }

    #[test]
    fn a_poste_the_court_refuses_takes_no_later_sum() {
        let cell = |v: &str| {
            format!(
                "{HEAD}Pour récapituler, les postes sont fixés comme suit :\n\n- préjudice sexuel (PS){v}\n\nCompte tenu de la créance de la caisse, il convient de condamner la société à verser à M. [X] la somme de 152 197,90 euros.{TAIL}"
            )
        };
        assert_eq!(
            poste(&cell(""), Mode::Sure, "PS"),
            Some((Some(152_197.90), None))
        );
        assert_eq!(poste(&cell(" : débouté"), Mode::Sure, "PS"), None);
    }

    #[test]
    fn the_claim_of_a_party_is_not_the_award() {
        let text = format!(
            "{HEAD}Sur le déficit fonctionnel permanent\n\nM. [D] sollicite la somme de 60 000 euros.\n\nLa cour fixe ce poste à 30 000 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "DFP"), Some((Some(30000.0), None)));
    }

    #[test]
    fn two_award_verbs_that_disagree_price_nothing_in_the_sure_mode() {
        let text = format!(
            "{HEAD}Sur le préjudice esthétique permanent\n\nIl sera alloué la somme de 4 000 euros.\n\nCe poste sera fixé à 5 000 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "PEP"), None);
    }

    #[test]
    fn an_approval_of_the_first_judge_prices_the_poste_in_the_sure_mode() {
        let text = format!(
            "{HEAD}Quant au préjudice d'agrément :\n\n23. Le tribunal a fait une appréciation suffisante du préjudice d'agrément, qualifié de moyen par les experts, en évaluant ce préjudice à la somme de 3 000 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Sure, "PA"), Some((Some(3000.0), None)));
    }

    #[test]
    fn a_confirmed_judgment_prices_the_poste_by_its_own_figure() {
        let text = format!(
            "{HEAD}Sur le préjudice d'agrément\n\nLe tribunal a alloué la somme de 5 000 euros de ce chef. Le jugement sera confirmé.{TAIL}"
        );
        assert_eq!(
            poste(&text, Mode::Extended, "PA"),
            Some((Some(5000.0), Some(5000.0)))
        );
    }

    #[test]
    fn a_reserved_poste_is_not_priced() {
        let text = format!(
            "{HEAD}Sur les dépenses de santé futures\n\nCe poste sera réservé. Une somme de 12 000 euros y est provisoirement estimée.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Extended, "DSF"), None);
    }

    #[test]
    fn a_ricochet_heading_writes_nothing_on_the_direct_victim() {
        let text = format!(
            "{HEAD}Sur les préjudices des proches\n\nSur le préjudice d'affection\n\nIl sera alloué à l'épouse la somme de 20 000 euros.{TAIL}"
        );
        assert_eq!(poste(&text, Mode::Extended, "PAF"), None);
    }
    #[test]
    fn the_operation_of_a_reduction_says_which_poste_the_dispositif_share_comes_from() {
        let text = "COUR D'APPEL\n\nARRÊT\n\nEXPOSÉ DU LITIGE\n\nM. [D] sollicite 40 000 euros.\n\nMOTIFS DE LA DÉCISION\n\nSur le déficit fonctionnel permanent\n\nIl est sollicité en réparation la somme de 30 375 x 75% = 22 781,25 euros. Il convient d'octroyer à M. [D] la somme de 22 781,25 euros.\n\nPAR CES MOTIFS\nLa cour,\nFixe le préjudice de M. [D] comme suit : déficit fonctionnel permanent : 22 781,25 euros ;";
        let x = extract(&scan(text), Mode::Sure);
        let p = x.quantum.victimes[0]
            .postes
            .iter()
            .find(|p| p.poste.as_deref() == Some("DFP"))
            .expect("DFP");
        assert_eq!(p.montant_alloue, Some(30375.0));
        assert_eq!(p.part_victime, Some(22781.25));
    }
}
