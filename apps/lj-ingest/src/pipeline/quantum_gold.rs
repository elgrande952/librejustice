//! Charge la vérité terrain du quantum en base, à `PC_GOLD_VERSION`. La source
//! reste les JSON versionnés en LFS ; la base en est un dérivé reconstructible.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use lj_extract::quantum::observation::{observations, Observation};
use lj_extract::quantum::{spreadsheet::compute, Quantum};
use lj_store::repository::{DecisionRepository, QuantumObservationRow, QuantumWriteItem};

use crate::config::Settings;

fn cents(x: Option<f64>) -> Option<i64> {
    x.map(|v| (v * 100.0).round() as i64)
}

fn row(o: &Observation) -> QuantumObservationRow {
    QuantumObservationRow {
        victime_ord: o.victime_ord,
        poste_ord: o.poste_ord,
        poste: o.poste.clone(),
        axe: o.axe.clone(),
        hors_table: o.hors_table.clone(),
        montant_cents: cents(o.montant),
        valeur_unitaire_cents: cents(o.valeur_unitaire),
        quantite: o.quantite,
        unite: o.unite.clone(),
        taux_pct: o.taux_pct,
        cotation: o.cotation,
        categorie: o.categorie.clone(),
        categorie_manque: o.categorie_manque.clone(),
        deces: o.deces,
        comparabilite: o.comparabilite.as_ref().map(|c| {
            serde_json::to_value(c).expect("Comparabilite ne porte que des chaînes et des taux")
        }),
        taux_dfp_pct: o.taux_dfp_pct,
        age: o.age.map(|a| a.round() as i16),
        sexe: o.sexe.clone(),
        role: o.role.clone(),
        lien: o.lien.clone(),
        preuve: o.preuve.clone(),
        equations: o.equations,
        origine: o.origine.clone(),
        cite: o.cite.clone(),
        indemnites_acquises_cents: cents(o.indemnites_acquises),
    }
}

/// Le nom de fichier est le `source_uid` dont la barre est doublée en tirets bas.
fn source_uid(chemin: &Path) -> Option<String> {
    Some(chemin.file_stem()?.to_string_lossy().replace("__", "/"))
}

/// Lit les records du répertoire, les juge, et les écrit à `PC_GOLD_VERSION`.
/// Un uid que le fonds n'a pas est signalé et sauté.
pub async fn load_quantum_gold(dir: &Path) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;

    let mut chemins: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("lecture de {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    chemins.sort();

    let mut lus: Vec<(String, Quantum)> = Vec::new();
    for chemin in &chemins {
        let brut = std::fs::read_to_string(chemin)
            .with_context(|| format!("lecture de {}", chemin.display()))?;
        let v: serde_json::Value = serde_json::from_str(&brut)
            .with_context(|| format!("json invalide dans {}", chemin.display()))?;
        let Some(volet) = v.get("quantum") else {
            continue;
        };
        let uid = source_uid(chemin)
            .ok_or_else(|| anyhow!("nom de fichier sans uid : {}", chemin.display()))?;
        let q: Quantum = serde_json::from_value(volet.clone())
            .with_context(|| format!("volet quantum illisible dans {}", chemin.display()))?;
        lus.push((uid, q));
    }

    let uids: Vec<String> = lus.iter().map(|(u, _)| u.clone()).collect();
    let ids: HashMap<String, i64> = DecisionRepository::new(&conn)
        .resolve_source_uids(&uids)
        .await?
        .into_iter()
        .collect();

    let mut items = Vec::new();
    let mut absents = Vec::new();
    for (uid, q) in &lus {
        let Some(&decision_id) = ids.get(uid) else {
            absents.push(uid.clone());
            continue;
        };
        let t = compute(q);
        let verdict = t.verdict();
        items.push(QuantumWriteItem {
            decision_id,
            est_liquidation: q.est_liquidation,
            atteste: Some(verdict.holds()),
            raison_non_atteste: verdict
                .conflicts
                .first()
                .map(|c| format!("{} : {:.2} contre {:.2}", c.cell, c.first, c.second)),
            cases_confirmees: Some(verdict.confirmed as i32),
            quantum: serde_json::to_value(q)?,
            observations: observations(q, &t).iter().map(row).collect(),
        });
    }

    conn.batch_execute("BEGIN").await?;
    let ecrites = DecisionRepository::new(&conn)
        .replace_quantum_bulk(&items, lj_core::PC_GOLD_VERSION)
        .await?;
    conn.batch_execute("COMMIT").await?;

    let tenus = items.iter().filter(|i| i.atteste == Some(true)).count();
    tracing::info!(
        records = items.len(),
        tenus,
        observations = ecrites,
        version = lj_core::PC_GOLD_VERSION,
        "vérité terrain du quantum chargée"
    );
    if !absents.is_empty() {
        tracing::warn!(
            absents = absents.len(),
            uids = absents.join(", "),
            "uids sans décision dans le fonds"
        );
    }
    Ok(())
}
