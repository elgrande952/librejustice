//! Whose voice writes a figure in the decision: the court's, a party's, the
//! expert's.
//!
//! Lexical, and it says so: a wrong voice is worse than no voice, so the
//! classifier answers `Unclear` far more often than it guesses.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    Court,
    Party,
    Expert,
    /// Before the reasoning opens: the parties' claims and the judgment under
    /// appeal, which the dispositif may confirm without writing its sums again.
    Recital,
    Unclear,
}

const PARTY: [&str; 24] = [
    "soutient",
    "soutiennent",
    "fait valoir",
    "font valoir",
    "sollicite",
    "sollicitent",
    "conclut a",
    "concluent a",
    "reclame",
    "reclament",
    "allegue",
    "alleguent",
    "pretend que",
    "pretendent que",
    "demande que",
    "demandent que",
    "demande la condamnation",
    "demande au tribunal",
    "demande a la cour",
    "dans ses ecritures",
    "dans son memoire",
    "aux termes de ses conclusions",
    "selon le requerant",
    "selon l'appelant",
];

const COURT: [&str; 36] = [
    "fonde a solliciter",
    "fondee a solliciter",
    "fonde a reclamer",
    "fondee a reclamer",
    "fonde a demander",
    "fondee a demander",
    "fonde a obtenir",
    "fondee a obtenir",
    "en octroyant",
    "en l'evaluant",
    "en fixant",
    "il resulte de l'instruction",
    "il ressort de l'instruction",
    "il resulte de ce qui precede",
    "il sera fait une juste appreciation",
    "il y a lieu d",
    "il convient d",
    "il n'y a pas lieu d",
    "considerant que",
    "eu egard a",
    "compte tenu de",
    "des lors",
    "en consequence",
    "sera condamne",
    "est condamne",
    "sont condamne",
    "condamne a verser",
    "sera fixe a",
    "en lui allouant",
    "il sera alloue",
    "lui sera alloue",
    "en retenant",
    "il est retenu",
    "ne conteste",
    "la cour",
    "le tribunal",
];

const EXPERT: [&str; 8] = [
    "l'expert",
    "les experts",
    "le docteur",
    "le dr ",
    "le medecin",
    "le sapiteur",
    "rapport d'expertise",
    "expertise judiciaire",
];

const DISPOSITIF: [&str; 3] = ["par ces motifs", "d e c i d e", "decide :"];

const MOTIFS: [&str; 8] = [
    "considerant ce qui suit",
    "motifs de la decision",
    "motifs du jugement",
    "motifs de l'arret",
    "motifs",
    "motivation",
    "sur ce",
    "discussion",
];

/// Beyond a paragraph the last marker says nothing about the figure.
const REACH: usize = 600;

/// A decision folded once, so a record's hundred anchors cost one pass. The raw
/// characters stay beside: the fold lowercases and blanks the newlines, which
/// are what a sentence boundary reads.
pub struct Speech {
    raw: Vec<char>,
    folded: Vec<char>,
    motifs: usize,
    dispositif: usize,
}

impl Speech {
    pub fn new(text: &str) -> Self {
        let raw: Vec<char> = text.chars().collect();
        let folded: Vec<char> = raw.iter().copied().map(lj_fold::fold_one).collect();
        let motifs = motifs_start(&raw, &folded);
        let dispositif = dispositif_start(&folded);
        Self {
            raw,
            folded,
            motifs,
            dispositif,
        }
    }

    /// Position decides before the reasoning opens and inside the dispositif.
    /// Between the two, the last marker before the figure does, never the mere
    /// presence of one: a sentence often carries two voices, the nearer governs.
    pub fn voice_at(&self, char_start: usize) -> Voice {
        if char_start < self.motifs {
            return Voice::Recital;
        }
        if char_start >= self.dispositif {
            return Voice::Court;
        }
        let end = char_start.min(self.folded.len());
        let floor = self.sentence_floor(end);
        let window: String = self.folded[floor..end].iter().collect();

        // Ranked by where a marker ends: « fondée à solliciter » is the court's
        // and embeds the party's verb.
        let last = |needles: &[&str]| -> Option<usize> {
            needles
                .iter()
                .filter_map(|n| window.rfind(n).map(|at| at + n.len()))
                .max()
        };
        [
            (last(&PARTY), Voice::Party),
            (last(&COURT), Voice::Court),
            (last(&EXPERT), Voice::Expert),
        ]
        .into_iter()
        .filter_map(|(at, v)| at.map(|a| (a, v)))
        .max_by_key(|(a, _)| *a)
        .map_or(Voice::Unclear, |(_, v)| v)
    }

    /// The dispositif confirms the judgment under appeal, so a sum written in
    /// the recital alone may still be what this court awards.
    pub fn confirms(&self) -> bool {
        let tail: String = self.folded[self.dispositif..].iter().collect();
        tail.contains("confirme")
    }

    fn sentence_floor(&self, end: usize) -> usize {
        let hard = end.saturating_sub(REACH);
        let mut i = end;
        while i > hard {
            i -= 1;
            let c = self.raw[i];
            if c == '\n' {
                return i + 1;
            }
            if c == '.' && self.ends_sentence(i) {
                return i + 1;
            }
        }
        hard
    }

    /// A full stop followed by a space and a capital. Abbreviations do not cut,
    /// which keeps the pleading verb in reach of the figure it introduces.
    fn ends_sentence(&self, at: usize) -> bool {
        let Some(&next) = self.raw.get(at + 1) else {
            return true;
        };
        if next != ' ' {
            return false;
        }
        self.raw
            .get(at + 2)
            .is_some_and(|c| c.is_uppercase() || *c == '\n')
    }
}

/// The first line that opens the court's reasoning, in the first 80 % of the
/// text, or 0 when none is written. A heading is short; a paragraph opening on
/// « Considérant que » is not, and opens the reasoning of an older decision.
fn motifs_start(raw: &[char], folded: &[char]) -> usize {
    let cap = raw.len() * 80 / 100;
    let mut start = 0;
    while start < cap {
        let end = raw[start..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(raw.len(), |p| start + p);
        let line: String = folded[start..end].iter().collect();
        let line = line
            .trim_start_matches(|c: char| !c.is_alphabetic())
            .trim_end();
        if line.starts_with("considerant qu")
            || (line.len() <= 60 && MOTIFS.iter().any(|h| line.starts_with(h)))
        {
            return start;
        }
        start = end + 1;
    }
    0
}

/// The last opening of the dispositif in the final 65 % of the text, or the
/// text's end when none is written.
fn dispositif_start(folded: &[char]) -> usize {
    let s: String = folded.iter().collect();
    let floor = s.len() * 35 / 100;
    let byte = DISPOSITIF
        .iter()
        .filter_map(|a| s.rfind(a))
        .filter(|&b| b >= floor)
        .max();
    match byte {
        Some(b) => s[..b].chars().count(),
        None => folded.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(text: &str, needle: &str) -> Voice {
        let at = text.chars().count() - text[text.find(needle).unwrap()..].chars().count();
        Speech::new(text).voice_at(at)
    }

    #[test]
    fn a_rate_pleaded_by_a_party_is_not_the_court_s() {
        let t = "M. D soutient que son déficit fonctionnel permanent doit être évalué à 60 %.";
        assert_eq!(voice(t, "60 %"), Voice::Party);
    }

    #[test]
    fn a_sum_the_court_awards_is_the_court_s() {
        let t = "Il sera fait une juste appréciation de ce chef en l'évaluant à 16 000 euros.";
        assert_eq!(voice(t, "16 000"), Voice::Court);
    }

    #[test]
    fn the_last_marker_before_the_figure_decides() {
        let t = "Mme B fait valoir que la cour doit lui allouer 14 000 euros.";
        assert_eq!(voice(t, "14 000"), Voice::Court);
        let t = "La cour relève que Mme B sollicite 14 000 euros.";
        assert_eq!(voice(t, "14 000"), Voice::Party);
    }

    #[test]
    fn an_elided_turn_of_phrase_still_reads() {
        let t = "M. A sollicite 400 000 euros. Il convient d'évaluer ce préjudice à 345 100 euros.";
        assert_eq!(voice(t, "345 100"), Voice::Court);
    }

    #[test]
    fn the_expert_s_figure_is_neither_the_court_s_nor_a_party_s() {
        let t = "Mme B soutient que l'expert a évalué le déficit fonctionnel permanent à 83 %.";
        assert_eq!(voice(t, "83 %"), Voice::Expert);
    }

    #[test]
    fn a_pleading_in_the_previous_sentence_does_not_carry() {
        let t = "M. D sollicite 5 000 euros. Le préjudice est de 3 000 euros.";
        assert_eq!(voice(t, "3 000"), Voice::Unclear);
        let t = "M. D sollicite 5 000 euros.\nLe préjudice est de 3 000 euros.";
        assert_eq!(voice(t, "3 000"), Voice::Unclear);
    }

    #[test]
    fn an_abbreviation_does_not_end_the_sentence() {
        let t = "M. D soutient au titre de l'art. 700 du code qu'il lui est dû 3 000 euros.";
        assert_eq!(voice(t, "3 000"), Voice::Party);
    }

    #[test]
    fn a_passage_naming_nobody_stays_unclear() {
        let t = "Le rapport mentionne un taux de 12 %.";
        assert_eq!(voice(t, "12 %"), Voice::Unclear);
    }

    #[test]
    fn a_marker_out_of_reach_does_not_carry() {
        let t = format!(
            "M. D soutient ceci.{}\nLe montant est de 500 euros.",
            "x".repeat(700)
        );
        assert_eq!(voice(&t, "500 euros"), Voice::Unclear);
    }

    #[test]
    fn before_the_reasoning_opens_every_figure_is_recited() {
        let t = "Vu la requête.\nIl soutient que :\n- la somme demandée est excessive ;\n\
                 - les sommes demandées au titre du déficit fonctionnel permanent de 60 % \
                 devraient être ramenées à de plus justes proportions.\n\
                 Considérant ce qui suit :\n1. Il y a lieu de retenir un taux de 40 %.";
        assert_eq!(voice(t, "60 %"), Voice::Recital);
        assert_eq!(voice(t, "40 %"), Voice::Court);
        assert!(!Speech::new(t).confirms());
    }

    #[test]
    fn a_confirming_dispositif_is_read() {
        let t = format!(
            "Le tribunal a alloué 5 000 euros.\nMOTIFS DE LA DÉCISION\n{}\nPAR CES MOTIFS\nLa cour, CONFIRME le jugement.",
            "motifs ".repeat(20)
        );
        let s = Speech::new(&t);
        assert_eq!(s.voice_at(t.find("5 000").unwrap()), Voice::Recital);
        assert!(s.confirms());
    }

    #[test]
    fn the_dispositif_is_the_court_s_whatever_it_says() {
        let t = format!(
            "{}\nM. D sollicite 5 000 euros.\nPAR CES MOTIFS\nLa cour, condamne X à payer 5 000 euros.",
            "motifs ".repeat(40)
        );
        let at = t.rfind("5 000").unwrap();
        let at = t[..at].chars().count();
        assert_eq!(Speech::new(&t).voice_at(at), Voice::Court);
    }
}
