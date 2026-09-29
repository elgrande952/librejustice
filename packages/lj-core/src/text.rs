//! Primitives de chaînes pures partagées (recasse, collapse d'espaces,
//! lookahead manuel), consommées côté serve et côté ingest. Le pliage vit dans
//! `lj-fold`.

use std::sync::LazyLock;

use regex::Regex;

pub fn uppercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `tok[0].upper() + tok[1:].lower()`.
pub fn capitalize_only(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Émule `re.finditer` avec borne droite *lookahead* consommée. Le crate `regex`
/// n'a pas de lookahead zéro-largeur (`(?=…)`) : nos patterns (citations
/// d'articles côté opendata, cabinets chaînés des observations côté judilibre)
/// consomment la borne droite, ce qui avancerait le curseur trop loin et
/// perdrait le match suivant. On rejoue donc `finditer` à la main en avançant à
/// la fin du groupe *borne* renvoyée par `advance`, pas à la fin du match
/// consommé ; garde-fou anti-boucle si la borne est zéro-largeur. Partagé :
/// opendata passe `group(2)`, judilibre `group(name)`.
pub fn captures_iter_lookahead<'t>(
    re: &Regex,
    text: &'t str,
    advance: impl Fn(&regex::Captures) -> Option<usize>,
) -> Vec<regex::Captures<'t>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start <= text.len() {
        match re.captures_at(text, start) {
            Some(c) => {
                let full_end = c.get(0).unwrap().end();
                let next = advance(&c).unwrap_or(full_end);
                let next = if next > start {
                    next
                } else {
                    full_end.max(start + 1)
                };
                out.push(c);
                start = next;
            }
            None => break,
        }
    }
    out
}

static RE_WS_GLOBAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

pub fn normalize_spaces(value: &str) -> String {
    RE_WS_GLOBAL
        .replace_all(value.trim(), " ")
        .trim()
        .to_string()
}
