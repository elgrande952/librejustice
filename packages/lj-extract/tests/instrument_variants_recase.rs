use serde::Deserialize;

#[derive(Deserialize)]
struct Variant {
    instrument: String,
    code: String,
}

#[test]
fn audited_variants_normalize_to_canonical_code() {
    let raw = include_str!("fixtures/instrument_variants.json");
    let variants: Vec<Variant> = serde_json::from_str(raw).expect("fixture valide");
    assert_eq!(
        variants.len(),
        311,
        "fixture attendue : 311 variantes auditées"
    );
    let mut fails = Vec::new();
    for v in &variants {
        let got = lj_extract::extract::normalize_instrument(&v.instrument);
        if got != v.code {
            fails.push(format!(
                "{:?} → {:?} (attendu {:?})",
                v.instrument, got, v.code
            ));
        }
    }
    assert!(
        fails.is_empty(),
        "{} variantes non recollées :\n{}",
        fails.len(),
        fails.join("\n")
    );
}
