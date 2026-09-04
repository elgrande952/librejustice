//! Diff de deux rédactions d'un article, en vis-à-vis.
//!
//! Deux passes : diff **ligne** (alinéa) pour ancrer la structure, puis diff
//! **mot Unicode** (UAX #29) dans les blocs réécrits.

use similar::{ChangeTag, DiffTag, TextDiff};

/// Opération d'un segment du diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Equal,
    Insert,
    Delete,
}

/// Tronçon contigu du diff : un texte et l'opération qui le porte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareSegment {
    pub op: CompareOp,
    pub text: String,
}

/// Nature d'une ligne alignée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Equal,
    /// Bloc réécrit : les deux colonnes portent du texte, la différence est au
    /// mot à l'intérieur.
    Changed,
    /// Alinéas apparus dans la nouvelle rédaction, colonne gauche vide.
    Added,
    /// Alinéas disparus de l'ancienne, colonne droite vide.
    Removed,
}

/// Une ligne du vis-à-vis, au grain du bloc d'alinéas. Concaténer `left` sur
/// toutes les lignes reconstruit l'ancienne rédaction, `right` la nouvelle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareRow {
    pub kind: RowKind,
    pub left: Vec<CompareSegment>,
    pub right: Vec<CompareSegment>,
}

/// Vis-à-vis des deux rédactions, en lignes alignées (aucun segment vide,
/// adjacents de même op fusionnés). Vide si les deux textes sont vides.
pub fn align_texts(old: &str, new: &str) -> Vec<CompareRow> {
    let line_diff = TextDiff::from_lines(old, new);
    let mut rows: Vec<CompareRow> = Vec::new();
    for op in line_diff.ops() {
        let block = |tag: ChangeTag| -> String {
            line_diff
                .iter_changes(op)
                .filter(|c| c.tag() == tag)
                .map(|c| c.value())
                .collect()
        };
        match op.tag() {
            DiffTag::Equal => {
                let side = vec![CompareSegment {
                    op: CompareOp::Equal,
                    text: block(ChangeTag::Equal),
                }];
                rows.push(CompareRow {
                    kind: RowKind::Equal,
                    left: side.clone(),
                    right: side,
                });
            }
            DiffTag::Delete => rows.push(CompareRow {
                kind: RowKind::Removed,
                left: vec![CompareSegment {
                    op: CompareOp::Delete,
                    text: block(ChangeTag::Delete),
                }],
                right: Vec::new(),
            }),
            DiffTag::Insert => rows.push(CompareRow {
                kind: RowKind::Added,
                left: Vec::new(),
                right: vec![CompareSegment {
                    op: CompareOp::Insert,
                    text: block(ChangeTag::Insert),
                }],
            }),
            DiffTag::Replace => {
                let (old_block, new_block) = (block(ChangeTag::Delete), block(ChangeTag::Insert));
                let word_diff = TextDiff::from_unicode_words(&old_block, &new_block);
                let mut left = Vec::new();
                let mut right = Vec::new();
                for change in word_diff.iter_all_changes() {
                    match change.tag() {
                        ChangeTag::Equal => {
                            push(&mut left, CompareOp::Equal, change.value());
                            push(&mut right, CompareOp::Equal, change.value());
                        }
                        ChangeTag::Delete => push(&mut left, CompareOp::Delete, change.value()),
                        ChangeTag::Insert => push(&mut right, CompareOp::Insert, change.value()),
                    }
                }
                rows.push(CompareRow {
                    kind: RowKind::Changed,
                    left,
                    right,
                });
            }
        }
    }
    rows
}

/// Ajoute un tronçon en fusionnant avec le précédent s'il porte la même op.
fn push(segments: &mut Vec<CompareSegment>, op: CompareOp, text: &str) {
    if text.is_empty() {
        return;
    }
    match segments.last_mut() {
        Some(last) if last.op == op => last.text.push_str(text),
        _ => segments.push(CompareSegment {
            op,
            text: text.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(rows: &[CompareRow], left: bool) -> String {
        rows.iter()
            .flat_map(|r| if left { &r.left } else { &r.right })
            .map(|s| s.text.as_str())
            .collect()
    }

    fn changed(rows: &[CompareRow]) -> usize {
        rows.iter().filter(|r| r.kind != RowKind::Equal).count()
    }

    #[test]
    fn identical_texts_yield_one_equal_row() {
        let text = "Chacun a droit au respect de sa vie privée.\nLes juges peuvent prescrire toutes mesures.";
        let rows = align_texts(text, text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, RowKind::Equal);
        assert_eq!(changed(&rows), 0);
        assert_eq!(side(&rows, true), text);
        assert_eq!(side(&rows, false), text);
    }

    /// Chaque colonne rend UNE rédaction : aucun segment de l'autre côté ne s'y
    /// intercale, sans quoi le vis-à-vis ne se lit pas.
    #[test]
    fn each_column_reads_as_one_redaction() {
        let old = "Nul ne peut faire l'objet de discriminations en raison de ses caractéristiques génétiques.";
        let new = "Nul ne peut faire l'objet d'une discrimination en raison de ses caractéristiques génétiques.";
        let rows = align_texts(old, new);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, RowKind::Changed);
        assert_eq!(side(&rows, true), old);
        assert_eq!(side(&rows, false), new);
        assert!(rows[0].left.iter().all(|s| s.op != CompareOp::Insert));
        assert!(rows[0].right.iter().all(|s| s.op != CompareOp::Delete));
    }

    #[test]
    fn inserted_alinea_leaves_the_left_column_empty() {
        let old = "Premier alinéa.\nDernier alinéa.";
        let new = "Premier alinéa.\nAlinéa nouveau.\nDernier alinéa.";
        let rows = align_texts(old, new);
        assert_eq!(side(&rows, true), old);
        assert_eq!(side(&rows, false), new);
        let added: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Added).collect();
        assert_eq!(added.len(), 1);
        assert!(added[0].left.is_empty());
        assert_eq!(added[0].right[0].text, "Alinéa nouveau.\n");
    }

    #[test]
    fn deleted_alinea_leaves_the_right_column_empty() {
        let old = "Premier alinéa.\nAlinéa condamné.\nDernier alinéa.";
        let new = "Premier alinéa.\nDernier alinéa.";
        let rows = align_texts(old, new);
        assert_eq!(side(&rows, true), old);
        assert_eq!(side(&rows, false), new);
        let removed: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Removed).collect();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].right.is_empty());
        assert_eq!(removed[0].left[0].text, "Alinéa condamné.\n");
    }

    /// Le compte des lignes non-Equal est ce dont se sert la navigation.
    #[test]
    fn changes_are_countable_and_ordered() {
        let old = "Un.\nDeux.\nTrois.\nQuatre.";
        let new = "Un.\nDeux bis.\nTrois.\nCinq.\nQuatre.";
        let rows = align_texts(old, new);
        assert_eq!(side(&rows, true), old);
        assert_eq!(side(&rows, false), new);
        assert_eq!(changed(&rows), 2);
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Equal,
                RowKind::Changed,
                RowKind::Equal,
                RowKind::Added,
                RowKind::Equal
            ]
        );
    }

    #[test]
    fn empty_old_text_is_a_full_addition() {
        let rows = align_texts("", "Texte créé.");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, RowKind::Added);
        assert_eq!(side(&rows, false), "Texte créé.");
        assert!(align_texts("", "").is_empty());
    }
}
