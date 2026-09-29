//! The address of every node of a text's table of contents: an article keeps
//! its key, a section takes the path of its segments (`livre-i/titre-v`), read
//! off the tree as it stands at one date.
//!
//! A segment comes from the numbering of the label (« Titre V : Du mariage »
//! gives `titre-v`), otherwise from the label cut on a word. Five rules make it
//! unique without an ordinal: a section with no article has no address; sibling
//! sections with the same label, up to its spelling, are one node; two siblings
//! with the same number carry their title; two labels equal on the cut take a
//! longer cut, up to the whole label; a first-level section holding the article
//! its segment names is that article. What still collides takes `-2`, `-3`, flagged by
//! [`TocNodePath::ordinal`].

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

/// First cut of an unnumbered label, and of the title a numbered one carries
/// when its number is shared.
const SHORT_CUT: usize = 40;
/// Second cut, for a segment still shared on the first.
const LONG_CUT: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TocKind {
    Section,
    Article,
}

/// One row of the tree in reading order, the nesting carried by `depth`
/// (1 = first level).
#[derive(Debug, Clone, Copy)]
pub struct TocRow<'a> {
    pub kind: TocKind,
    pub depth: i32,
    pub label: &'a str,
    pub num_key: Option<&'a str>,
}

/// A node of the merged tree, in reading order. `rows` are the input rows it
/// stands for: one for an article, every merged sibling for a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocNodePath {
    pub kind: TocKind,
    pub depth: i32,
    pub rows: Vec<usize>,
    /// The section's path, `None` for an article and for a section with no
    /// address of its own.
    pub path: Option<String>,
    /// The last segment needed an ordinal.
    pub ordinal: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TocPaths {
    pub nodes: Vec<TocNodePath>,
}

impl TocPaths {
    /// Index in `nodes` of the section at `path`.
    pub fn find(&self, path: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.path.as_deref() == Some(path))
    }

    /// The node at `index` and its descendants, a contiguous run of `nodes`.
    pub fn subtree(&self, index: usize) -> &[TocNodePath] {
        let depth = self.nodes[index].depth;
        let end = self.nodes[index + 1..]
            .iter()
            .position(|n| n.depth <= depth)
            .map_or(self.nodes.len(), |p| index + 1 + p);
        &self.nodes[index..end]
    }
}

struct Tree {
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
    has_article: Vec<bool>,
}

fn build_tree(rows: &[TocRow<'_>]) -> Tree {
    let mut children = vec![Vec::new(); rows.len()];
    let mut roots = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        while stack.last().is_some_and(|&p| rows[p].depth >= row.depth) {
            stack.pop();
        }
        match stack.last() {
            Some(&p) => children[p].push(i),
            None => roots.push(i),
        }
        if row.kind == TocKind::Section {
            stack.push(i);
        }
    }
    let mut has_article = vec![false; rows.len()];
    for i in (0..rows.len()).rev() {
        has_article[i] =
            rows[i].kind == TocKind::Article || children[i].iter().any(|&c| has_article[c]);
    }
    Tree {
        children,
        roots,
        has_article,
    }
}

/// Computes the address of every node. `rows` is the tree of one text at one
/// date, in reading order; `article_keys` holds the key of every article the
/// text ever had, which a first-level segment must not resolve to.
pub fn toc_paths(rows: &[TocRow<'_>], article_keys: &HashSet<&str>) -> TocPaths {
    let tree = build_tree(rows);
    let mut out = TocPaths::default();
    let roots = tree.roots.clone();
    walk(rows, &tree, &roots, None, 1, article_keys, &mut out);
    out
}

enum Item {
    Article(usize),
    Section(Vec<usize>),
}

fn walk(
    rows: &[TocRow<'_>],
    tree: &Tree,
    siblings: &[usize],
    parent: Option<&str>,
    depth: i32,
    article_keys: &HashSet<&str>,
    out: &mut TocPaths,
) {
    let mut items: Vec<Item> = Vec::new();
    let mut merged: HashMap<String, usize> = HashMap::new();
    for &i in siblings {
        match rows[i].kind {
            TocKind::Article => items.push(Item::Article(i)),
            TocKind::Section => {
                let key = merge_key(rows[i].label);
                match merged.get(&key) {
                    Some(&at) => {
                        if let Item::Section(members) = &mut items[at] {
                            members.push(i);
                        }
                    }
                    None => {
                        merged.insert(key, items.len());
                        items.push(Item::Section(vec![i]));
                    }
                }
            }
        }
    }

    let sections: Vec<&Vec<usize>> = items
        .iter()
        .filter_map(|it| match it {
            Item::Section(m) if m.iter().any(|&i| tree.has_article[i]) => Some(m),
            _ => None,
        })
        .filter(|m| {
            depth > 1
                || !ladder(rows[m[0]].label)
                    .iter()
                    .any(|seg| holds_its_article(rows, tree, m, seg))
        })
        .collect();
    let labels: Vec<&str> = sections.iter().map(|m| rows[m[0]].label).collect();
    let segments = sibling_segments(&labels, depth == 1, article_keys);
    let segment_of: HashMap<usize, (String, bool)> = sections
        .iter()
        .zip(segments)
        .map(|(members, segment)| (members[0], segment))
        .collect();

    for item in items {
        match item {
            Item::Article(i) => out.nodes.push(TocNodePath {
                kind: TocKind::Article,
                depth,
                rows: vec![i],
                path: None,
                ordinal: false,
            }),
            Item::Section(members) => {
                let (path, ordinal) = match segment_of.get(&members[0]).cloned() {
                    Some((s, ordinal)) => (
                        Some(match parent {
                            Some(p) => format!("{p}/{s}"),
                            None => s,
                        }),
                        ordinal,
                    ),
                    None => (None, false),
                };
                let kids: Vec<usize> = members
                    .iter()
                    .flat_map(|&m| tree.children[m].iter().copied())
                    .collect();
                out.nodes.push(TocNodePath {
                    kind: TocKind::Section,
                    depth,
                    rows: members,
                    path: path.clone(),
                    ordinal,
                });
                let below = path.as_deref().or(parent);
                walk(rows, tree, &kids, below, depth + 1, article_keys, out);
            }
        }
    }
}

/// A first-level section holding the article one of its segments names: the
/// article's address already names it.
fn holds_its_article(rows: &[TocRow<'_>], tree: &Tree, members: &[usize], segment: &str) -> bool {
    let wanted = resolved_keys(segment);
    let mut stack: Vec<usize> = members.to_vec();
    while let Some(i) = stack.pop() {
        match rows[i].kind {
            TocKind::Article => {
                if rows[i]
                    .num_key
                    .is_some_and(|k| wanted.iter().any(|w| w == k))
                {
                    return true;
                }
            }
            TocKind::Section => stack.extend(tree.children[i].iter().copied()),
        }
    }
    false
}

/// The keys an article reading of `segment` tries.
fn resolved_keys(segment: &str) -> Vec<String> {
    crate::article_key::candidate_keys(&crate::article_key::identity_key(segment))
}

/// The segments of addressed sibling sections, in order, each flagged when it
/// needed an ordinal. A segment shared with a sibling, reserved, or at the
/// first level resolving to an article, climbs its ladder of longer cuts; what
/// still collides at the top takes an ordinal.
fn sibling_segments(
    labels: &[&str],
    first_level: bool,
    article_keys: &HashSet<&str>,
) -> Vec<(String, bool)> {
    let ladders: Vec<Vec<String>> = labels.iter().map(|l| ladder(l)).collect();
    let blocked = |s: &str| {
        reserved(s)
            || (first_level
                && resolved_keys(s)
                    .iter()
                    .any(|k| article_keys.contains(k.as_str())))
    };
    let mut level = vec![0usize; labels.len()];
    loop {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (i, l) in ladders.iter().enumerate() {
            *counts.entry(l[level[i]].as_str()).or_default() += 1;
        }
        let climb: Vec<usize> = (0..labels.len())
            .filter(|&i| {
                let s = ladders[i][level[i]].as_str();
                level[i] + 1 < ladders[i].len() && (counts[s] > 1 || blocked(s))
            })
            .collect();
        if climb.is_empty() {
            break;
        }
        for i in climb {
            level[i] += 1;
        }
    }

    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(labels.len());
    for (i, l) in ladders.iter().enumerate() {
        let s = &l[level[i]];
        let mut chosen = s.clone();
        let mut n = 2;
        while taken.contains(&chosen) || blocked(&chosen) {
            chosen = format!("{s}-{n}");
            n += 1;
        }
        taken.insert(chosen.clone());
        let ordinal = chosen != *s;
        out.push((chosen, ordinal));
    }
    out
}

/// The segments a label may take, shortest first: the number alone then with
/// its title, or the label, each cut at [`SHORT_CUT`], [`LONG_CUT`], then whole.
fn ladder(label: &str) -> Vec<String> {
    let seg = segment(label);
    let mut out = vec![if seg.base.is_empty() {
        "section".to_string()
    } else {
        seg.base.clone()
    }];
    let tail = match &seg.title {
        Some(title) => Some(lj_fold::slug(title, '-')),
        None if seg.numbered => None,
        None => Some(lj_fold::slug(label, '-')),
    };
    if let Some(tail) = tail.filter(|t| !t.is_empty()) {
        for max in [SHORT_CUT, LONG_CUT, usize::MAX] {
            let t = cut(&tail, max);
            let s = if seg.numbered {
                format!("{}-{t}", seg.base)
            } else {
                t
            };
            if out.last() != Some(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// A segment the article route reads otherwise: a date, the comparator.
fn reserved(segment: &str) -> bool {
    let b = segment.as_bytes();
    segment == "compare"
        || (b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()))
}

struct Segment {
    base: String,
    numbered: bool,
    title: Option<String>,
}

static NUMBERED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(partie|livre|titre|sous-titre|chapitre|sous-chapitre|section|sous-section|paragraphe|sous-paragraphe|annexe|tome|division|article)\s+(pr[ée]liminaire|premi[eè]re?|unique|[0-9]+(?:er|re|e)?|[ivxlcdm]+(?:er|re)?|[a-z])\b(?:[\s-]+(bis|ter|quater|quinquies|sexies|septies|octies|nonies|decies)\b)?(?:[\s.:\-\x{2013}\x{2014})]*)(.*)$",
    )
    .expect("numbered label regex")
});

fn segment(label: &str) -> Segment {
    if let Some(c) = NUMBERED.captures(label) {
        let kind = lj_fold::slug(&c[1], '-');
        let raw = lj_fold::slug(&c[2], '-');
        let number = match raw.as_str() {
            "premier" | "premiere" => "i".to_string(),
            r => r
                .strip_suffix("er")
                .or_else(|| r.strip_suffix("re"))
                .filter(|n| {
                    !n.is_empty()
                        && n.chars()
                            .all(|ch| ch.is_ascii_digit() || "ivxlcdm".contains(ch))
                })
                .or_else(|| {
                    r.strip_suffix('e')
                        .filter(|n| !n.is_empty() && n.chars().all(|ch| ch.is_ascii_digit()))
                })
                .unwrap_or(r)
                .to_string(),
        };
        let mut base = format!("{kind}-{number}");
        if let Some(latin) = c.get(3) {
            base.push('-');
            base.push_str(&latin.as_str().to_ascii_lowercase());
        }
        let title = c
            .get(4)
            .map(|t| t.as_str().trim())
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        return Segment {
            base,
            numbered: true,
            title,
        };
    }
    Segment {
        base: cut(&lj_fold::slug(label, '-'), SHORT_CUT),
        numbered: false,
        title: None,
    }
}

/// Two sibling labels with the same key are one division: the numbering read
/// as the segment reads it (« Chapitre Ier » is « Chapitre premier »), the
/// words folded and taken in the singular (« III OBLIGATIONS » is « III.
/// Obligation »), the versions of one heading the source spelt apart.
fn merge_key(label: &str) -> String {
    let seg = segment(label);
    let words = match (&seg.title, seg.numbered) {
        (Some(title), _) => format!("{}-{}", seg.base, lj_fold::slug(title, '-')),
        (None, true) => seg.base,
        (None, false) => lj_fold::slug(label, '-'),
    };
    words
        .split('-')
        .map(|w| match w.strip_suffix(['s', 'x']) {
            Some(stem) if stem.len() >= 3 => stem,
            _ => w,
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// `slug` cut to at most `max` bytes, on a separator when one falls inside.
fn cut(slug: &str, max: usize) -> String {
    if slug.len() <= max {
        return slug.to_string();
    }
    match slug[..=max].rfind('-') {
        Some(at) if at > 0 => slug[..at].to_string(),
        _ => slug[..max].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(depth: i32, label: &str) -> TocRow<'_> {
        TocRow {
            kind: TocKind::Section,
            depth,
            label,
            num_key: None,
        }
    }
    fn a(depth: i32, key: &'static str) -> TocRow<'static> {
        TocRow {
            kind: TocKind::Article,
            depth,
            label: key,
            num_key: Some(key),
        }
    }
    fn keys<'a>(rows: &[TocRow<'a>]) -> HashSet<&'a str> {
        rows.iter().filter_map(|r| r.num_key).collect()
    }
    fn paths(rows: &[TocRow<'_>]) -> Vec<Option<String>> {
        toc_paths(rows, &keys(rows))
            .nodes
            .into_iter()
            .filter(|n| n.kind == TocKind::Section)
            .map(|n| n.path)
            .collect()
    }
    fn some(p: &str) -> Option<String> {
        Some(p.to_string())
    }

    #[test]
    fn un_segment_se_tire_de_la_numerotation() {
        for (label, seg) in [
            ("Titre V : Du mariage", "titre-v"),
            ("Chapitre Ier : Dispositions générales.", "chapitre-i"),
            ("Chapitre premier", "chapitre-i"),
            (
                "Livre préliminaire : Dispositions communes",
                "livre-preliminaire",
            ),
            ("Section 2 : De la preuve", "section-2"),
            ("Sous-section 1 bis : Délais", "sous-section-1-bis"),
            ("Article 1er", "article-1"),
            ("Partie législative", "partie-legislative"),
            ("Annexe", "annexe"),
            ("VI CONTENTIEUX", "vi-contentieux"),
        ] {
            assert_eq!(segment(label).base, seg, "{label}");
        }
    }

    #[test]
    fn un_chemin_suit_l_arbre() {
        let rows = [
            s(1, "Livre Ier : Des personnes"),
            s(2, "Titre V : Du mariage"),
            a(3, "144"),
            s(1, "Livre II : Des biens"),
            a(2, "516"),
        ];
        assert_eq!(
            paths(&rows),
            vec![some("livre-i"), some("livre-i/titre-v"), some("livre-ii")]
        );
    }

    #[test]
    fn les_soeurs_au_meme_intitule_sont_un_seul_noeud() {
        let rows = [
            s(1, "Chapitre III : Compte professionnel de prévention"),
            s(2, "Section 1 : Principes"),
            a(3, "l4163-1"),
            s(1, "Chapitre III : Compte professionnel de prévention"),
            s(2, "Section 4 : Contrôle"),
            a(3, "l4163-16"),
        ];
        let out = toc_paths(&rows, &keys(&rows));
        assert!(out.nodes.iter().all(|n| !n.ordinal));
        let chapter = out.find("chapitre-iii").expect("merged chapter");
        assert_eq!(out.nodes[chapter].rows, vec![0, 3]);
        let under: Vec<_> = out
            .subtree(chapter)
            .iter()
            .filter_map(|n| n.path.clone())
            .collect();
        assert_eq!(
            under,
            vec![
                "chapitre-iii",
                "chapitre-iii/section-1",
                "chapitre-iii/section-4"
            ]
        );
    }

    #[test]
    fn les_graphies_d_une_meme_division_sont_un_seul_noeud() {
        let rows = [
            s(1, "III OBLIGATIONS"),
            a(2, "1"),
            s(1, "III. Obligation"),
            a(2, "2"),
            s(1, "Chapitre Ier : Dispositions générales"),
            a(2, "3"),
            s(1, "Chapitre premier : Disposition générale"),
            a(2, "4"),
        ];
        assert_eq!(
            paths(&rows),
            vec![some("iii-obligations"), some("chapitre-i")]
        );
    }

    #[test]
    fn deux_numeros_egaux_prennent_leur_titre() {
        let rows = [
            s(1, "Chapitre III : Comités et commissions"),
            a(2, "1"),
            s(1, "Chapitre III : Dispositions financières"),
            a(2, "2"),
        ];
        assert_eq!(
            paths(&rows),
            vec![
                some("chapitre-iii-comites-et-commissions"),
                some("chapitre-iii-dispositions-financieres")
            ]
        );
    }

    #[test]
    fn deux_intitules_egaux_a_la_coupe_prennent_une_coupe_plus_longue() {
        let rows = [
            s(
                1,
                "Dispositions relatives aux établissements publics de santé mentale",
            ),
            a(2, "1"),
            s(
                1,
                "Dispositions relatives aux établissements publics de santé et médico-sociaux",
            ),
            a(2, "2"),
        ];
        let out = paths(&rows);
        assert_ne!(out[0], out[1]);
        assert_eq!(
            out[0],
            some("dispositions-relatives-aux-etablissements-publics-de-sante-mentale")
        );
    }

    #[test]
    fn une_section_vide_n_a_pas_d_adresse() {
        let rows = [s(1, "Annexe"), s(1, "Annexe II"), a(2, "1")];
        assert_eq!(paths(&rows), vec![None, some("annexe-ii")]);
    }

    #[test]
    fn une_section_qui_n_enveloppe_que_son_article_est_cet_article() {
        let rows = [s(1, "Annexe"), a(2, "annexe"), s(1, "Titre Ier"), a(2, "1")];
        assert_eq!(paths(&rows), vec![None, some("titre-i")]);
    }

    #[test]
    fn une_section_qui_tient_l_article_de_son_nom_est_cet_article() {
        let rows = [
            s(1, "Annexes"),
            a(2, "annexe"),
            a(2, "annexes"),
            s(1, "Annexe IV.1"),
            a(2, "annexe-iv-1"),
            s(3, "Tableau"),
            a(3, "tableau-1"),
        ];
        assert_eq!(paths(&rows)[..2], [None, None]);
    }

    #[test]
    fn deux_intitules_egaux_sur_la_coupe_longue_prennent_l_intitule_entier() {
        let long = "Convention conclue entre l'Etat et les personnes morales ou physiques qui bénéficient à titre principal d'un prêt conventionné";
        let a_ = format!("{long} pour l'amélioration des logements");
        let b_ = format!("{long} pour la construction");
        let rows = [s(1, &a_), a(2, "1"), s(1, &b_), a(2, "2")];
        let out = toc_paths(&rows, &keys(&rows));
        assert!(out.nodes.iter().all(|n| !n.ordinal));
        let p: Vec<_> = out.nodes.iter().filter_map(|n| n.path.clone()).collect();
        assert!(p[0].ends_with("amelioration-des-logements"), "{}", p[0]);
        assert!(p[1].ends_with("pour-la-construction"), "{}", p[1]);
    }

    #[test]
    fn un_segment_qui_reste_pris_prend_un_ordinal_compte() {
        let rows = [s(1, "Annexe"), a(2, "1"), a(2, "2"), a(1, "annexe")];
        let out = toc_paths(&rows, &keys(&rows));
        assert!(out.nodes[0].ordinal);
        assert_eq!(out.nodes[0].path, some("annexe-2"));
    }
}
