//! Construction du prompt de résumé + nettoyage de sortie.
//!
//! PUR : pas d'appel LLM ici (l'I/O réseau vit dans `lj-ingest`). On expose le
//! prompt versionné, le découpage tête/queue de l'input, et le nettoyage de la
//! sortie modèle.

use std::sync::LazyLock;

use regex::Regex;

/// Prompt generation in `decisions.summary_prompt_version`. ORDERED, not a text
/// identifier: staleness is a `<`, so two live prompts of equivalent quality
/// share a number and are told apart by [`SummaryEngine`].
///
/// The collage shares the number with the rewrite: the stale query keys on this
/// alone, so a collage left one generation behind is re-collaged for nothing,
/// 161 222 rows on 20/09/2026.
pub const SUMMARY_PROMPT_VERSION: i16 = 10;

/// Prompt système de résumé (texte intégral, source de vérité).
pub const SUMMARY_PROMPT: &str = include_str!("../data/summary_prompt.txt");

/// Not interchangeable with [`SUMMARY_PROMPT`]: its per-sentence bands are held
/// to the character by Flash-Lite and read as indicative by mistral-small.
pub const SUMMARY_PROMPT_GEMINI: &str = include_str!("../data/summary_prompt_gemini.txt");

/// Names what the third sentence is for: what the rule holds over the dispute, never
/// the procedural disposition, which 3.5 wrote there 47 % of the time and 34 % once
/// told. Model-agnostic as measured, mistral-small included.
pub const SUMMARY_PROMPT_SOLUTION: &str = include_str!("../data/summary_prompt_solution.txt");

/// [`SUMMARY_PROMPT_SOLUTION`] with the length clause widened, the doctrine cut by a
/// third and the per-sentence budget shifted onto the rule. Worth 0.51 point against
/// the Cour's sommaire and 0.36 against the Conseil d'Etat's analyse; a third shorter
/// costs nothing measurable on either (15/15, p = 1.00 on the held-out bench) and
/// returns 15 % of the tokens per call, the Mistral ceiling being tokens per minute.
pub const SUMMARY_PROMPT_MISTRAL: &str = include_str!("../data/summary_prompt_mistral.txt");

/// One call where the others take three: the selection happens inside the
/// reasoning rather than in a separate sorting pass. Measured 20/09/2026 by opus
/// on luna at 0,68 of F-measure over the other jurisdictions and 0,71 over
/// cassation and the Conseil d'Etat, where the prompt it replaces scores 0,49 and
/// 0,61. Its budget of 235 characters per point of law is what holds luna on the
/// target, median 715 against a reference at 702, where the same text at 350
/// writes 892 for a substance the duels do not separate.
pub const SUMMARY_PROMPT_MONO: &str = include_str!("../data/summary_prompt_mono.txt");

/// The same text at 350 characters per point of law, which is what the two
/// `flash-lite` need. Given one instruction gemini writes about 200 characters
/// less than luna: this budget puts gemini-3.5 at 692 and 678 on the two benches
/// against references at 716 and 702, where the 235 one served 579 in production.
/// gpt-6-luna at `low` reads it too, where it outscores the 235 one 32 to 23 on
/// F-measure for 0,46 dollar per thousand, and still writes 517 on the corpus.
pub const SUMMARY_PROMPT_MONO_WIDE: &str = include_str!("../data/summary_prompt_mono_wide.txt");

/// The court's own commentary carries the law, so the model writes only what it
/// lacks: an opening sentence and an outcome clause, one per line, the second
/// often empty. [`assemble_collage`] pastes the commentary between them.
///
/// Its angles are bracketed patterns and never sentences, because a literal
/// example gets copied onto the wrong court: measured 21/09/2026 over the 280
/// commented decisions, the wording it replaces pastes one of its three examples
/// 6 times and writes the outcome in 47 % of summaries, this one 0 and 81 %.
pub const SUMMARY_PROMPT_COLLAGE: &str = include_str!("../data/summary_prompt_collage.txt");

/// The collage text for gpt-6-luna, which given the one above opens 11 summaries
/// in 280 past the 160 characters of the meta and drops the outcome clause from
/// 81 % to 69 %. This one holds the opening at 94, writes the clause at 81 %, and
/// no longer names a natural person, which the one above does in 43 % of openings.
pub const SUMMARY_PROMPT_COLLAGE_TIGHT: &str =
    include_str!("../data/summary_prompt_collage_tight.txt");

/// The mono text for `mistral-medium` at `high`, which given the two above
/// overruns the 160 characters of the meta on 51 openings in 120. This one holds
/// 2, and ties the three other doors outside cassation and the Conseil d'Etat.
pub const SUMMARY_PROMPT_MONO_MISTRAL: &str =
    include_str!("../data/summary_prompt_mono_mistral.txt");

/// Reasoning level asked of the model, frozen by the engine like the prompt.
///
/// `AV-M2` scores 0,70 of F-measure on gemini-3.5-flash-lite at `high` where the
/// shipped prompt scores 0,49; nothing says the same text holds at the door's
/// default, so the level travels with the pair or the bench describes another
/// system than the one that writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    /// The door's own default: no field is sent.
    Default,
    /// Refused by Gemini with a 400, which only knows `minimal` and above.
    None,
    Low,
    High,
}

impl Thinking {
    /// Wire value of the level, `None` when the field is left out.
    pub fn wire(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::None => Some("none"),
            Self::Low => Some("low"),
            Self::High => Some("high"),
        }
    }
}

/// The (prompt, model, reasoning) triple behind a summary, persisted in
/// `decisions.summary_engine`. `prompt_version` derives from it, so the two
/// columns cannot drift apart.
///
/// It also says what the summary was written from: a prompt shown the court's
/// own commentary carries a code of its own and never takes a decision that
/// carries none. One falling back to the plain text where the commentary is
/// missing would stop saying which summaries came from the court's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i16)]
pub enum SummaryEngine {
    MistralSmallV8 = 1,
    Gemini31FlashLiteV9 = 2,
    Gemini35FlashLiteV9 = 3,
    MistralSmallV9 = 4,
    MistralMediumV9 = 5,
    LunaMonoV9 = 6,
    LunaCollageV10 = 7,
    Gemini31MonoV10 = 8,
    Gemini35MonoV10 = 9,
    LunaMonoV10 = 10,
    LunaCollageV11 = 11,
    Luna6MonoV10 = 12,
    Luna6CollageV10 = 13,
    MistralMediumMonoV10 = 14,
}

impl SummaryEngine {
    /// Every engine that pastes the court's commentary, whatever its generation.
    pub const COLLAGES: [Self; 3] = [
        Self::LunaCollageV10,
        Self::LunaCollageV11,
        Self::Luna6CollageV10,
    ];

    pub fn code(self) -> i16 {
        self as i16
    }

    pub fn prompt_version(self) -> i16 {
        match self {
            Self::MistralSmallV8 => 8,
            Self::Gemini31FlashLiteV9
            | Self::Gemini35FlashLiteV9
            | Self::MistralSmallV9
            | Self::MistralMediumV9
            | Self::LunaMonoV9 => 9,
            Self::LunaCollageV10
            | Self::Gemini31MonoV10
            | Self::Gemini35MonoV10
            | Self::LunaMonoV10
            | Self::LunaCollageV11
            | Self::Luna6MonoV10
            | Self::Luna6CollageV10
            | Self::MistralMediumMonoV10 => 10,
        }
    }

    pub fn model(self) -> &'static str {
        match self {
            Self::MistralSmallV8 | Self::MistralSmallV9 => "mistral-small-2603",
            Self::Gemini31FlashLiteV9 | Self::Gemini31MonoV10 => "gemini-3.1-flash-lite",
            Self::Gemini35FlashLiteV9 | Self::Gemini35MonoV10 => "gemini-3.5-flash-lite",
            Self::MistralMediumV9 | Self::MistralMediumMonoV10 => "mistral-medium-latest",
            Self::LunaMonoV9 | Self::LunaCollageV10 | Self::LunaMonoV10 | Self::LunaCollageV11 => {
                "gpt-5.6-luna"
            }
            Self::Luna6MonoV10 | Self::Luna6CollageV10 => "gpt-6-luna",
        }
    }

    /// Reasoning level the bench measured this pair at.
    pub fn thinking(self) -> Thinking {
        match self {
            Self::Gemini31MonoV10 | Self::Gemini35MonoV10 | Self::MistralMediumMonoV10 => {
                Thinking::High
            }
            Self::Luna6MonoV10 | Self::Luna6CollageV10 => Thinking::Low,
            Self::LunaMonoV9 | Self::LunaCollageV10 | Self::LunaMonoV10 | Self::LunaCollageV11 => {
                Thinking::None
            }
            Self::MistralSmallV8
            | Self::Gemini31FlashLiteV9
            | Self::Gemini35FlashLiteV9
            | Self::MistralSmallV9
            | Self::MistralMediumV9 => Thinking::Default,
        }
    }

    /// The prompt is a property of the engine, never a global: a model given the
    /// other model's prompt writes something no bench ever measured. A retired
    /// code returns the current text of its slot and never generates again.
    pub fn prompt(self) -> &'static str {
        match self {
            Self::MistralSmallV8 => SUMMARY_PROMPT,
            Self::Gemini31FlashLiteV9 => SUMMARY_PROMPT_GEMINI,
            Self::Gemini35FlashLiteV9 | Self::MistralSmallV9 => SUMMARY_PROMPT_SOLUTION,
            Self::MistralMediumV9 => SUMMARY_PROMPT_MISTRAL,
            Self::LunaMonoV9 | Self::LunaMonoV10 => SUMMARY_PROMPT_MONO,
            Self::Gemini31MonoV10 | Self::Gemini35MonoV10 | Self::Luna6MonoV10 => {
                SUMMARY_PROMPT_MONO_WIDE
            }
            Self::LunaCollageV10 | Self::LunaCollageV11 => SUMMARY_PROMPT_COLLAGE,
            Self::Luna6CollageV10 => SUMMARY_PROMPT_COLLAGE_TIGHT,
            Self::MistralMediumMonoV10 => SUMMARY_PROMPT_MONO_MISTRAL,
        }
    }

    pub fn from_code(code: i16) -> Option<Self> {
        Some(match code {
            1 => Self::MistralSmallV8,
            2 => Self::Gemini31FlashLiteV9,
            3 => Self::Gemini35FlashLiteV9,
            4 => Self::MistralSmallV9,
            5 => Self::MistralMediumV9,
            6 => Self::LunaMonoV9,
            7 => Self::LunaCollageV10,
            8 => Self::Gemini31MonoV10,
            9 => Self::Gemini35MonoV10,
            10 => Self::LunaMonoV10,
            11 => Self::LunaCollageV11,
            12 => Self::Luna6MonoV10,
            13 => Self::Luna6CollageV10,
            14 => Self::MistralMediumMonoV10,
            _ => return None,
        })
    }
}

// --- Construction de l'entrée Mistral --------------------------------------
//
// mistral-small a 128k tokens de contexte : on envoie le texte INTÉGRAL par
// défaut. Au-delà du cap, on garde la TÊTE (parties, moyens, motifs) ET la QUEUE
// (dispositif) — jamais la tête seule, qui inverse l'issue.

const SUMMARY_INPUT_CAP: usize = 80_000;
const SUMMARY_INPUT_HEAD: usize = 56_000;
const SUMMARY_INPUT_TAIL: usize = 24_000;
const ELISION: &str = "\n[...]\n";

/// Borne l'input modèle : intégral si `≤ cap`, sinon tête + queue avec élision.
///
/// Découpage sur frontières de caractères (code points), pas d'octets.
pub fn build_summary_input(body_text: &str) -> String {
    let char_len = body_text.chars().count();
    if char_len <= SUMMARY_INPUT_CAP {
        return body_text.to_string();
    }
    let head: String = body_text.chars().take(SUMMARY_INPUT_HEAD).collect();
    let tail: String = body_text
        .chars()
        .skip(char_len - SUMMARY_INPUT_TAIL)
        .collect();
    format!("{head}{ELISION}{tail}")
}

// --- Collage of the court's own commentary ---------------------------------

/// Written body required after the nomenclature chain. Below it the chain is all
/// the commentary carries and there is nothing to paste.
pub const COLLAGE_PROSE_MIN: usize = 120;

/// Separators from which a head reads as a nomenclature chain and not a sentence.
const CHAIN_SEGMENTS: usize = 2;

/// Recueil footnote mark, always against the period: `... disposition (1).`
static FOOTNOTE_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d\)(\s*\.)").unwrap());
/// Standalone nomenclature enumerator, never `(CGI)` nor `C.G.I.)`.
static ENUMERATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|\s)[0-9a-z]\)\s").unwrap());
static TYPEWRITER_QUOTES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\"([^\"]{2,}?)\"").unwrap());
static WHITESPACE_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// The commentary stripped of its recueil apparatus, ready to paste verbatim.
///
/// The apparatus is a Conseil d'Etat trait: measured on the bench, 60 cassation
/// summaries need nothing, while of 60 analyses 36 open on a nomenclature chain,
/// 8 carry an enumerator, 3 a footnote mark and 12 typewriter quotes.
pub fn commentary_prose(commentary: &str) -> String {
    let trimmed = commentary.trim();
    let body = match trimmed.split_once("\n\n") {
        Some((head, rest))
            if head.matches(" - ").count() >= CHAIN_SEGMENTS
                && rest.trim().chars().count() > COLLAGE_PROSE_MIN =>
        {
            rest
        }
        _ => trimmed,
    };
    let body = WHITESPACE_RUN.replace_all(body, " ");
    let body = FOOTNOTE_MARK.replace_all(&body, "$1");
    let mut body = body.into_owned();
    while ENUMERATOR.is_match(&body) {
        body = ENUMERATOR.replace_all(&body, "$1").into_owned();
    }
    let body = TYPEWRITER_QUOTES.replace_all(&body, "« $1 »");
    let body = SPACE_BEFORE_PUNCT.replace_all(&body, "$1");
    body.trim().to_string()
}

/// The model writes the curly apostrophe, the courts the straight one. Two
/// registers inside one paragraph is what the collage makes visible and the rewrite
/// never does, the court's prose being the mass of the text.
fn straight_apostrophes(text: &str) -> String {
    text.replace('\u{2019}', "'")
}

/// Opening, the court's commentary, outcome clause.
///
/// `None` when the model wrote no opening: a commentary alone is not a summary,
/// it never says who contests what.
pub fn assemble_collage(model_output: &str, commentary: &str) -> Option<String> {
    let mut said = model_output.lines().map(str::trim).filter(|line| {
        !line.is_empty()
            && !matches!(
                line.to_lowercase().as_str(),
                "-" | "aucune" | "vide" | "néant"
            )
    });
    let opening = said.next()?;
    let mut out = String::with_capacity(commentary.len() + 256);
    out.push_str(&straight_apostrophes(opening.trim_end_matches('.')));
    out.push_str(". ");
    out.push_str(&commentary_prose(commentary));
    if let Some(outcome) = said.next() {
        out.push(' ');
        out.push_str(&straight_apostrophes(&capitalize_first(
            outcome.trim_end_matches('.'),
        )));
        out.push('.');
    }
    Some(out)
}

// --- Nettoyage déterministe des codes d'anonymisation ----------------------
//
// Le LLM recopie parfois la pseudonymisation de la source. Deux familles :
// - placeholders sans valeur de nom — lieu (« [Localité 6] ») ou code numérique
//   (« la société [7] », « [22] ») : on supprime le code ET sa préposition
//   introductrice éventuelle, sinon « le comité économique du [7] » laisse « le
//   comité économique du, » orphelin.
// - initiales d'une personne : « M. [W] [Y] » — c'est le nom anonymisé, on garde
//   le contenu et on retire juste les crochets → « M. W Y ».
//
// Le LLM recopie aussi parfois l'en-tête « [Décision] <jur>, <date>, <num> »
// (interdit par le prompt) en première ligne : on le retire.

/// En-tête recopié en tête de résumé : ligne « [Décision]/Décision … <date ISO>
/// … » suivie d'un saut de ligne. La date ISO (signature de la citation injectée
/// en en-tête) évite de confondre avec une vraie phrase commençant par « Décision ».
static HEADER_ECHO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[?\s*Décision\b[^\n]*?\d{4}-\d{2}-\d{2}[^\n]*\n+").unwrap());
/// Placeholder pseudonymisé sans valeur de nom — lieu ou code numérique — retiré
/// avec sa préposition introductrice éventuelle.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[ \t]*(?:\b(?:de la|de l'|du|des|de|à la|aux|au|à|en|dans|sur|vers|chez)\s*)?\[(?:(?:Localité|Adresse|Commune|Ville)[^\]]*|\d[^\]]*)\]",
    )
    .unwrap()
});
/// Initiales de personne « [W] » → contenu gardé, crochets retirés.
static INITIAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\d][^\]]{0,29})\]").unwrap());
static SPACE_BEFORE_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+([,.;:!?])").unwrap());
static MULTISPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
/// Ponctuation/espaces orphelins en tête (placeholder retiré en début de phrase).
static LEADING_JUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\s,;:.–—-]+").unwrap());
/// Outside the repertoire French legal prose uses. The luna door glues a spam
/// token of its vocabulary to the last sentence on 1,9 % of its calls: ideographs,
/// private use area, fullwidth forms, a repeated female sign. Greek stays, the
/// CJUE quotes Greek trademarks; so do the comparison operators, a summary writes
/// `effectif salarié ≤ 5`. Measured 21/09/2026 over 3 982 rows, Gemini emitting
/// none on 17 406.
static FOREIGN_GLYPH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "[^\t\n\r\x20-\x7e\u{a0}-\u{24f}\u{300}-\u{3ff}\u{1e00}-\u{1eff}\
         \u{2000}-\u{209f}\u{20a0}-\u{20bf}\u{2100}-\u{218f}\u{2200}-\u{22ff}]",
    )
    .unwrap()
});

/// Nettoie la sortie modèle (echo d'en-tête, placeholders, initiales, ponctuation).
///
/// Trois choses : (1) un en-tête « [Décision] … » recopié en tête ; (2) les
/// placeholders (numéros, lieux) avec leur préposition introductrice ; en gardant
/// (3) les noms (initiales) — crochets retirés. Préserve les sauts de ligne
/// (séparateur de phrases) et est idempotent / no-op sur un résumé déjà propre.
pub fn clean_summary(text: &str) -> String {
    let text = FOREIGN_GLYPH.replace_all(text, "");
    let text = HEADER_ECHO.replace(&text, "");
    let text = PLACEHOLDER.replace_all(&text, "");
    let text = INITIAL.replace_all(&text, "$1");
    let text = SPACE_BEFORE_PUNCT.replace_all(&text, "$1");
    let text = MULTISPACE.replace_all(&text, " ");
    let leading_removed = LEADING_JUNK.is_match(&text);
    let text = LEADING_JUNK.replace(&text, "");
    let text = text.trim();

    // Un placeholder retiré en tête de phrase laisse espace/ponctuation orphelins
    // (`leading_removed`), puis une minuscule en première position : un résumé ouvre
    // toujours une phrase, on recapitalise.
    if leading_removed && first_char_is_lower(text) {
        capitalize_first(text)
    } else {
        text.to_string()
    }
}

/// Vrai ssi le premier caractère est une minuscule (au sens Unicode). Faux sur
/// chaîne vide ou premier char non-cased.
fn first_char_is_lower(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_lowercase())
}

/// Majuscule du premier caractère.
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINES: [SummaryEngine; 14] = [
        SummaryEngine::MistralSmallV8,
        SummaryEngine::Gemini31FlashLiteV9,
        SummaryEngine::Gemini35FlashLiteV9,
        SummaryEngine::MistralSmallV9,
        SummaryEngine::MistralMediumV9,
        SummaryEngine::LunaMonoV9,
        SummaryEngine::LunaCollageV10,
        SummaryEngine::Gemini31MonoV10,
        SummaryEngine::Gemini35MonoV10,
        SummaryEngine::LunaMonoV10,
        SummaryEngine::LunaCollageV11,
        SummaryEngine::Luna6MonoV10,
        SummaryEngine::Luna6CollageV10,
        SummaryEngine::MistralMediumMonoV10,
    ];

    #[test]
    fn prompt_version_is_the_newest_generation() {
        let newest = ENGINES.map(SummaryEngine::prompt_version).into_iter().max();
        assert_eq!(newest, Some(SUMMARY_PROMPT_VERSION));
    }

    /// Two engines may share a prompt, a text measured on both models being one text.
    /// What may never happen is a generation reading another one's: the number would
    /// then stop saying what was sent.
    #[test]
    fn every_engine_round_trips_and_carries_its_generation_prompt() {
        for engine in ENGINES {
            assert_eq!(SummaryEngine::from_code(engine.code()), Some(engine));
            assert_eq!(
                engine.prompt() == SUMMARY_PROMPT,
                engine.prompt_version() == 8
            );
        }
        assert_eq!(SummaryEngine::from_code(0), None);
    }

    #[test]
    fn clean_strips_the_spam_token_and_keeps_greek_and_operators() {
        assert_eq!(
            clean_summary("La requête est rejetée.娱乐网址"),
            "La requête est rejetée."
        );
        assert_eq!(
            clean_summary("Le pourvoi est rejeté. 天天中彩票官方 La cassation est acquise."),
            "Le pourvoi est rejeté. La cassation est acquise."
        );
        assert_eq!(
            clean_summary("Le code de justice administrative s'applique.♀♀♀"),
            "Le code de justice administrative s'applique."
        );
        assert_eq!(
            clean_summary("La marque υγεία est enregistrée pour un effectif salarié ≤ 5."),
            "La marque υγεία est enregistrée pour un effectif salarié ≤ 5."
        );
    }

    #[test]
    fn build_input_passthrough_under_cap() {
        let body = "petite décision";
        assert_eq!(build_summary_input(body), body);
    }

    #[test]
    fn build_input_head_tail_over_cap() {
        let body: String = "x".repeat(SUMMARY_INPUT_CAP + 1);
        let out = build_summary_input(&body);
        assert_eq!(
            out.chars().count(),
            SUMMARY_INPUT_HEAD + ELISION.chars().count() + SUMMARY_INPUT_TAIL
        );
        assert!(out.contains("\n[...]\n"));
    }

    #[test]
    fn build_input_at_cap_is_passthrough() {
        let body: String = "y".repeat(SUMMARY_INPUT_CAP);
        assert_eq!(build_summary_input(&body), body);
    }

    #[test]
    fn clean_keeps_names_strips_brackets() {
        assert_eq!(
            clean_summary("M. [W] [Y] conteste le refus."),
            "M. W Y conteste le refus."
        );
    }

    #[test]
    fn clean_strips_placeholder_with_preposition() {
        assert_eq!(clean_summary("le comité du [7] décide"), "le comité décide");
        assert_eq!(
            clean_summary("la cour de [Localité 5] statue"),
            "la cour statue"
        );
    }

    #[test]
    fn clean_strips_locality_placeholder() {
        assert_eq!(
            clean_summary("La société [Localité 6] agit."),
            "La société agit."
        );
    }

    #[test]
    fn clean_strips_header_echo() {
        let input = "[Décision] Cour de cassation, 2024-03-12, 22-10.123\nUn salarié conteste son licenciement.";
        assert_eq!(
            clean_summary(input),
            "Un salarié conteste son licenciement."
        );
    }

    #[test]
    fn clean_recapitalizes_after_leading_junk() {
        assert_eq!(
            clean_summary("[7] saisit le tribunal."),
            "Saisit le tribunal."
        );
    }

    #[test]
    fn clean_is_noop_on_clean_summary() {
        let clean = "Un salarié licencié conteste la rupture de son contrat.";
        assert_eq!(clean_summary(clean), clean);
    }

    #[test]
    fn clean_removes_space_before_punct() {
        assert_eq!(clean_summary("le comité [7] , statue"), "le comité, statue");
    }

    #[test]
    fn collage_pastes_the_commentary_between_what_the_model_wrote() {
        let commentary = "Champ d'application - Inclusion - Conséquence\n\n1) Il résulte de \
            l'article L. 136-3 du CSS que les revenus tirés d'une activité non salariée sont \
            assujettis dès lors qu'elle est exercée à titre professionnel (1). 2) Indemnité pour \
            \"gestion d'affaire\" regardée comme un revenu professionnel.";
        let assembled = assemble_collage(
            "Des contribuables contestent l'assujettissement d'une indemnité\n\nDécharge des \
             cotisations supplémentaires",
            commentary,
        )
        .unwrap();
        assert!(assembled.starts_with(
            "Des contribuables contestent l'assujettissement d'une indemnité. Il résulte"
        ));
        assert!(assembled.ends_with("Décharge des cotisations supplémentaires."));
        assert!(assembled.contains("« gestion d'affaire »"));
        for apparatus in ["Champ d'application", "1)", "2)", "(1)", "\n"] {
            assert!(!assembled.contains(apparatus), "reste {apparatus:?}");
        }
    }

    #[test]
    fn collage_keeps_a_commentary_that_is_prose_from_its_first_word() {
        let commentary = "L'infra petita ne se soulève pas d'office.";
        let assembled = assemble_collage("Un requérant conteste un jugement", commentary).unwrap();
        assert_eq!(
            assembled,
            "Un requérant conteste un jugement. L'infra petita ne se soulève pas d'office."
        );
    }

    #[test]
    fn collage_without_an_opening_writes_nothing() {
        assert_eq!(assemble_collage("", "Un sommaire officiel."), None);
        assert_eq!(assemble_collage("\n  \n", "Un sommaire officiel."), None);
    }

    #[test]
    fn collage_aligns_the_model_apostrophe_on_the_court_one() {
        let assembled =
            assemble_collage("Une soci\u{e9}t\u{e9} conteste l\u{2019}assiette d\u{2019}une taxe\nd\u{2019}o\u{f9} la cassation", "Le sommaire.")
                .unwrap();
        assert_eq!(
            assembled,
            "Une société conteste l'assiette d'une taxe. Le sommaire. D'où la cassation."
        );
    }

    #[test]
    fn collage_drops_the_word_a_model_writes_for_an_empty_clause() {
        let assembled = assemble_collage("Une ouverture\naucune", "Le sommaire.").unwrap();
        assert_eq!(assembled, "Une ouverture. Le sommaire.");
    }
}
