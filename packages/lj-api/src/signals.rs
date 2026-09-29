//! Adaptive RRF weights. The base is parity `(title=1.0, bm25=1.0, ann=1.0)`,
//! the key leg at 0 outside `reference` mode, then one of two multipliers read
//! on the query or on the embedding geometry: an article in the query, bm25
//! x1.5; `d_centroids > 0.06` and `pair_mu < 0.18`, bm25 x0.4 and ann x2.5.

use std::sync::LazyLock;

use regex::Regex;

/// Longueur considérée pour les centroïdes / la matrice pairwise.
pub const TOP_K: usize = 50;

/// Seuils issus de la calibration empirique.
const D_CENTROIDS_THRESHOLD: f64 = 0.06;
const PAIR_MU_THRESHOLD: f64 = 0.18;

/// `\b[LRD]\.?\s*\d+(?:[-]\d+)+` matche L 531-1, R. 222-1, D.521-2… (IGNORECASE).
static ARTICLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[LRD]\.?\s*\d+(?:-\d+)+").unwrap());

/// RRF weight per leg. `docket` is 0 outside `reference` mode, where the leg
/// does not enter the fusion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub title: f64,
    pub bm25: f64,
    pub ann: f64,
    pub docket: f64,
}

impl Weights {
    fn scale(self, bm25: f64, ann: f64) -> Weights {
        Weights {
            title: self.title,
            bm25: self.bm25 * bm25,
            ann: self.ann * ann,
            docket: self.docket,
        }
    }
}

/// Base par défaut : 3 jambes à parité, la clé inerte.
pub const BASE_WEIGHTS: Weights = Weights {
    title: 1.0,
    bm25: 1.0,
    ann: 1.0,
    docket: 0.0,
};

pub fn has_article_reference(query: &str) -> bool {
    ARTICLE_RE.is_match(query)
}

/// Distance cosinus (1 − cos) entre les centroïdes des deux jeux d'embeddings.
///
/// `ann_emb` / `bm25_emb` sont des lignes (un vecteur par chunk). Vide → 0.0.
pub fn compute_d_centroids(ann_emb: &[Vec<f32>], bm25_emb: &[Vec<f32>]) -> f64 {
    if ann_emb.is_empty() || bm25_emb.is_empty() {
        return 0.0;
    }
    let a = mean_axis0(ann_emb);
    let b = mean_axis0(bm25_emb);
    let a_norm = l2_norm(&a);
    let b_norm = l2_norm(&b);
    if a_norm == 0.0 || b_norm == 0.0 {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(&b).map(|(x, y)| *x * *y).sum();
    1.0 - dot / (a_norm * b_norm)
}

/// 1 − moyenne des similarités cosinus pairwise (triangle supérieur, k=1).
pub fn compute_pair_mu(ann_emb: &[Vec<f32>]) -> f64 {
    let n = ann_emb.len();
    if n < 2 {
        return 0.0;
    }
    let normed: Vec<Vec<f64>> = ann_emb
        .iter()
        .map(|row| {
            let norm = l2_norm_f32(row);
            let denom = if norm == 0.0 { 1.0 } else { norm };
            row.iter().map(|x| (*x as f64) / denom).collect()
        })
        .collect();
    let mut sum = 0.0;
    let mut count = 0usize;
    for i in 0..n {
        for j in (i + 1)..n {
            let sim: f64 = normed[i].iter().zip(&normed[j]).map(|(x, y)| x * y).sum();
            sum += sim;
            count += 1;
        }
    }
    1.0 - sum / count as f64
}

fn mean_axis0(rows: &[Vec<f32>]) -> Vec<f64> {
    let dim = rows[0].len();
    let mut acc = vec![0.0f64; dim];
    for row in rows {
        for (a, x) in acc.iter_mut().zip(row) {
            *a += *x as f64;
        }
    }
    let n = rows.len() as f64;
    for a in &mut acc {
        *a /= n;
    }
    acc
}

fn l2_norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

fn l2_norm_f32(v: &[f32]) -> f64 {
    v.iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt()
}

/// Content-mode weights: [`BASE_WEIGHTS`] scaled by the one signal that fires.
pub fn compute_weights(query: &str, ann_emb: &[Vec<f32>], bm25_emb: &[Vec<f32>]) -> Weights {
    let mut w = BASE_WEIGHTS;
    if has_article_reference(query) {
        w = w.scale(1.5, 1.0);
    } else {
        let d_cent = compute_d_centroids(ann_emb, bm25_emb);
        let p_mu = compute_pair_mu(ann_emb);
        if d_cent > D_CENTROIDS_THRESHOLD && p_mu < PAIR_MU_THRESHOLD {
            w = w.scale(0.4, 2.5);
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_signal_detected() {
        assert!(has_article_reference("article L 531-1 du CESEDA"));
        assert!(has_article_reference("R. 222-1"));
        assert!(has_article_reference("D.521-2"));
        assert!(!has_article_reference("congés payés"));
    }

    fn designates_a_number(query: &str) -> bool {
        !lj_core::docket_key::query_numbers(query).is_empty()
    }

    #[test]
    fn docket_signal_all_formats() {
        assert!(designates_a_number("pourvoi 07-11.687"));
        assert!(designates_a_number("19-23.664"));
        assert!(designates_a_number("19-23664"));
        assert!(designates_a_number("23/01001"));
        assert!(designates_a_number("requête 2202166"));
        assert!(designates_a_number("CAA Paris 21PA01295"));
        assert!(designates_a_number("CJUE C-369/90"));
        assert!(designates_a_number("SAN-2022-019"));
        assert!(!designates_a_number("juste du texte"));
        assert!(!designates_a_number("article L761-1"));
        assert!(!designates_a_number("décret 2020-1310"));
    }

    #[test]
    fn article_scales_bm25_up_and_leaves_the_rest() {
        let w = compute_weights("article L 531-1", &[], &[]);
        assert_eq!(w.bm25, 1.5);
        assert_eq!(w.title, 1.0);
        assert_eq!(w.ann, 1.0);
        assert_eq!(w.docket, 0.0);
    }

    #[test]
    fn base_weights_when_no_signal_no_embeddings() {
        let w = compute_weights("congés payés licenciement", &[], &[]);
        assert_eq!(w, BASE_WEIGHTS);
    }

    #[test]
    fn boost_ann_when_centroids_diverge_and_ann_tight() {
        let ann = vec![vec![1.0f32, 0.0, 0.0], vec![1.0, 0.01, 0.0]];
        let bm25 = vec![vec![0.0f32, 1.0, 0.0], vec![0.0, 1.0, 0.0]];
        let w = compute_weights("question naturelle", &ann, &bm25);
        assert_eq!(w.bm25, 0.4);
        assert_eq!(w.ann, 2.5);
    }

    #[derive(serde::Deserialize)]
    struct SigCase {
        query: String,
        article: bool,
        docket: bool,
    }
    #[derive(serde::Deserialize)]
    struct SigFixture {
        cases: Vec<SigCase>,
    }

    #[test]
    fn detection_signal_parity_oracle() {
        let raw = include_str!("../tests/fixtures/oracle/signals_detect.json");
        let fix: SigFixture = serde_json::from_str(raw).expect("fixture signals_detect");
        for c in &fix.cases {
            assert_eq!(
                has_article_reference(&c.query),
                c.article,
                "article {:?}",
                c.query
            );
            assert_eq!(
                designates_a_number(&c.query),
                c.docket,
                "docket {:?}",
                c.query
            );
        }
    }

    #[test]
    fn centroid_and_pair_mu_match_frozen_values() {
        let ann = vec![
            vec![1.0f32, 0.0, 0.0, 0.0],
            vec![0.5, 0.5, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
        ];
        let bm = vec![vec![0.0f32, 0.0, 1.0, 0.0], vec![0.0, 0.0, 0.5, 0.5]];
        assert!((compute_d_centroids(&ann, &bm) - 1.0).abs() < 1e-6);
        assert!((compute_pair_mu(&ann) - 0.5285954773426056).abs() < 1e-6);
    }

    #[derive(serde::Deserialize)]
    struct WeightCase {
        name: String,
        query: String,
        ann_emb: Vec<Vec<f32>>,
        bm25_emb: Vec<Vec<f32>>,
        title: f64,
        bm25: f64,
        ann: f64,
        docket: f64,
    }
    #[derive(serde::Deserialize)]
    struct WeightFixture {
        cases: Vec<WeightCase>,
    }

    #[test]
    fn compute_weights_full_matrix_parity_oracle() {
        let raw = include_str!("../tests/fixtures/oracle/signals_weights.json");
        let fix: WeightFixture = serde_json::from_str(raw).expect("fixture signals_weights");
        for c in &fix.cases {
            let w = compute_weights(&c.query, &c.ann_emb, &c.bm25_emb);
            assert!(
                (w.title - c.title).abs() < 1e-12,
                "{} title {} != {}",
                c.name,
                w.title,
                c.title
            );
            assert!(
                (w.bm25 - c.bm25).abs() < 1e-12,
                "{} bm25 {} != {}",
                c.name,
                w.bm25,
                c.bm25
            );
            assert!(
                (w.ann - c.ann).abs() < 1e-12,
                "{} ann {} != {}",
                c.name,
                w.ann,
                c.ann
            );
            assert!(
                (w.docket - c.docket).abs() < 1e-12,
                "{} docket {} != {}",
                c.name,
                w.docket,
                c.docket
            );
        }
    }
}
