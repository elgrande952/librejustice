//! Invariants de fin de chaîne : des contrôles qui publient un nombre de lignes
//! fautives sans faire échouer la chaîne.
//!
//! Chaque contrôle émet `ingest.invariant.breach`, attribut `invariant`, valeur
//! = nombre de lignes fautives, 0 = sain. Le diagnostic va au journal, nommant
//! chaque fautif ; la métrique ne porte que le compte, pour qu'une règle
//! d'alerte lise un seuil et non une chaîne de caractères.

use std::future::Future;

use anyhow::{anyhow, Result};
use lj_store::repository::{DecisionRepository, InstrumentResolution};
use opentelemetry::KeyValue;

use crate::config::Settings;

/// Sources de décisions qu'une chaîne synchronise, et le nombre de jours sans
/// nouvelle ligne au-delà duquel la source est tenue pour muette. Chaque seuil
/// vaut environ le double du plus grand écart observé sur 120 jours entre deux
/// journées d'ingestion (judilibre 3, opendata 3, dila-jade 4, cjue 6,
/// dila-cnil 11, cnda 13, cedh 15, dila-constit 27) : le contrôle vise une
/// source cassée, pas une source lente.
///
/// Une source absente de cette liste n'est surveillée par rien : `adde` parce
/// qu'aucune chaîne ne le synchronise, `ariane-web` et `ariane-dce` parce que
/// leur job se mesure contre le fond et qu'une journée sans nouvelle ligne y est
/// l'état normal. C'est ici que se lit la différence entre dormante et cassée.
const SOURCES_SUIVIES: &[(&str, i64)] = &[
    ("judilibre", 7),
    ("opendata", 7),
    ("dila-jade", 10),
    ("cjue", 14),
    ("dila-cnil", 25),
    ("cedh", 30),
    ("dila-constit", 55),
    // cnda publishes in bursts: its catalogue shows a 47-day hole (26/02 → 14/04
    // 2026) that the calibration window started just after, so the 30 it yielded
    // alerted on 08/09/2026 against a source already fully ingested.
    ("cnda", 95),
];

/// Channels of the Annonces pot, and the days without a new row past which the
/// channel is taken for broken. The pot is 44 days old on 16/09/2026, a third
/// of the window the decision sources were calibrated on, so the floor is a
/// week rather than twice the widest gap seen: at that date the widest was 8
/// days on atline, 4 on achatpublic and klekoon, 2 or 3 everywhere else.
///
/// `nicematin` and `provincenord` are watched by nothing: 65 and 11 rows since
/// they opened, a press dump and a territorial portal publishing in bursts,
/// where a silent week is the normal state.
const CANAUX_SUIVIS: &[(&str, i64)] = &[
    ("boamp", 7),
    ("ted", 7),
    ("aws", 7),
    ("atexo", 7),
    ("dematis", 7),
    ("aji", 7),
    ("modula", 7),
    ("xmarches", 7),
    ("safetender", 7),
    ("synapse", 7),
    ("centraledesmarches", 7),
    ("achatpublic", 8),
    ("klekoon", 8),
    ("atline", 16),
    // The only channel the chain computes instead of harvesting: it posts when
    // data.economie.gouv.fr republishes, most days, and a wedged quick exit
    // would otherwise stop it in silence.
    ("decp", 7),
];

/// Joue tous les invariants. Rend toujours `Ok` : un contrôle qui échoue est un
/// observateur en panne, pas une chaîne en panne — le faire remonter
/// confondrait les deux.
pub async fn run() -> Result<()> {
    mesurer("fraicheur_source", fraicheur_des_sources()).await;
    mesurer("jurisdiction_code_tire", jurisdiction_code_tire()).await;
    mesurer("alias_sur_slug_mort", alias_sur_slug_mort()).await;
    mesurer("alias_sur_uid_mort", alias_sur_uid_mort()).await;
    mesurer("alias_sans_ancre_resoluble", alias_sans_ancre_resoluble()).await;
    mesurer("version_article_mal_situee", version_article_mal_situee()).await;
    mesurer("version_intervalle_inverse", version_intervalle_inverse()).await;
    mesurer("decision_sans_charge", decision_sans_charge()).await;
    mesurer("uid_ue_permute", uid_ue_permute()).await;
    mesurer("dataset_ue_uid_ferme", dataset_ue_uid_ferme()).await;
    mesurer("base_reinitialisee", base_reinitialisee()).await;
    mesurer("denorm_derive", denorm_derive()).await;
    mesurer("fraicheur_canal_annonce", fraicheur_des_canaux()).await;
    mesurer("marche_decp_sans_resultat", marche_decp_sans_resultat()).await;
    Ok(())
}

/// Counts the watched channels whose last posted row is past their threshold.
/// The Annonces counterpart of [`fraicheur_des_sources`], where a broken
/// connector shows up nowhere else: the harvest re-enumerates everything every
/// night, and an engine yielding zero row exits as successfully as a live one.
async fn fraicheur_des_canaux() -> Result<u64> {
    let conn = repo_conn().await?;
    let vues: std::collections::HashMap<String, chrono::NaiveDate> = DecisionRepository::new(&conn)
        .derniere_moisson_par_canal()
        .await
        .map_err(|e| anyhow!("derniere_moisson_par_canal: {e}"))?
        .into_iter()
        .collect();

    let aujourdhui = chrono::Utc::now().date_naive();
    let mut fautifs = 0;
    for (canal, seuil) in CANAUX_SUIVIS {
        let jours = match vues.get(*canal) {
            Some(dernier) => (aujourdhui - *dernier).num_days(),
            None => i64::MAX,
        };
        if jours > *seuil {
            fautifs += 1;
            tracing::warn!(canal, jours, seuil, "canal d'annonces muet");
        }
    }
    Ok(fautifs)
}

/// Counts the markets a `decp:` channel calls awarded with no `resultat` block
/// to show it. The pose writes both together; the channel alone takes the
/// market out of the matching queue for good, and its page stays empty.
async fn marche_decp_sans_resultat() -> Result<u64> {
    let conn = repo_conn().await?;
    let fautifs = DecisionRepository::new(&conn)
        .marches_decp_sans_resultat()
        .await
        .map_err(|e| anyhow!("marches_decp_sans_resultat: {e}"))?;
    for uid in &fautifs {
        tracing::warn!(uid, "marché on the DECP channel without a result block");
    }
    Ok(fautifs.len() as u64)
}

/// Counts 1 while a crash recovery younger than the chain's own period is
/// visible. On 09/09/2026 the cluster killed and re-initialised every backend
/// for one to two minutes and nothing said so: the postmaster survives the
/// death of a child it does not track, so its start time never moves. The
/// window is the chain's period, so the alert fires once and clears itself.
async fn base_reinitialisee() -> Result<u64> {
    let conn = repo_conn().await?;
    let quand = DecisionRepository::new(&conn)
        .derniere_recovery_a_chaud()
        .await
        .map_err(|e| anyhow!("derniere_recovery_a_chaud: {e}"))?;
    match quand.filter(|t| chrono::Utc::now() - *t < chrono::Duration::hours(25)) {
        Some(t) => {
            tracing::warn!(quand = %t, "database: hot recovery, every connection killed");
            Ok(1)
        }
        None => Ok(0),
    }
}

/// Counts the articles whose denormalised copies disagree with their parent
/// text. The end-of-pass refresh is bounded to the texts a pass touched, so a
/// drift it never saw is invisible to it and visible only here.
async fn denorm_derive() -> Result<u64> {
    let conn = repo_conn().await?;
    let n = DecisionRepository::new(&conn)
        .denorm_derive()
        .await
        .map_err(|e| anyhow!("denorm_derive: {e}"))?;
    if n > 0 {
        tracing::warn!(articles = n, "articles: denormalised copy drifting");
    }
    Ok(n as u64)
}

async fn mesurer(invariant: &'static str, mesure: impl Future<Output = Result<u64>>) {
    match mesure.await {
        Ok(fautifs) => {
            publier(invariant, fautifs);
            publier_lisibilite(invariant, 0);
        }
        Err(e) => {
            tracing::error!(invariant, error = %e, "invariant unreadable");
            publier_lisibilite(invariant, 1);
        }
    }
}

/// Ouvre une connexion de lecture. Chaque invariant a la sienne : ils sont
/// indépendants, et un contrôle illisible ne doit pas emporter les autres.
async fn repo_conn() -> Result<deadpool_postgres::Object> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))
}

fn publier(invariant: &'static str, fautifs: u64) {
    lj_telemetry::meter()
        .u64_gauge("ingest.invariant.breach")
        .with_description("Lignes fautives relevées par un invariant de fin de chaîne.")
        .build()
        .record(fautifs, &[KeyValue::new("invariant", invariant)]);
    tracing::info!(invariant, fautifs, "invariant measured");
}

fn publier_lisibilite(invariant: &'static str, illisible: u64) {
    lj_telemetry::meter()
        .u64_gauge("ingest.invariant.unreadable")
        .with_description("1 quand un invariant de fin de chaîne n'a pas pu se mesurer.")
        .build()
        .record(illisible, &[KeyValue::new("invariant", invariant)]);
}

/// Compte les décisions qu'aucune provenance ne peut ré-extraire. Le rang 0 dit
/// qu'une ligne de `decision_sources` ne porte aucune décision (commentaires
/// ArianeWeb, analyses ADDE) ; une décision qui n'a que celles-là garde
/// éternellement sa version d'extraction de création, sans qu'aucune erreur ne
/// remonte. C'est ainsi que 93 décisions du Conseil d'État sont restées sur une
/// version de 2026-08.
async fn decision_sans_charge() -> Result<u64> {
    let conn = repo_conn().await?;
    let fautives = DecisionRepository::new(&conn)
        .decisions_sans_charge()
        .await
        .map_err(|e| anyhow!("decisions_sans_charge: {e}"))?;
    for public_id in &fautives {
        tracing::warn!(public_id, "decision without a load provenance");
    }
    Ok(fautives.len() as u64)
}

/// Compte les codes de juridiction portant un tiret. Le rattrapage MCP qui
/// remplace le tiret par le souligné n'est sans ambiguïté que tant
/// que ce compte vaut zéro : un seul code tiré au référentiel, et un code tiré
/// inconnu cesse d'être forcément une faute de frappe.
async fn jurisdiction_code_tire() -> Result<u64> {
    let conn = repo_conn().await?;
    let fautifs = DecisionRepository::new(&conn)
        .jurisdictions_au_code_tire()
        .await
        .map_err(|e| anyhow!("jurisdictions_au_code_tire: {e}"))?;
    for code in &fautifs {
        tracing::warn!(code, "juridiction code with a hyphen");
    }
    Ok(fautifs.len() as u64)
}

/// Compte les alias du dataset curé qui nomment un slug qu'aucun texte ne porte
/// plus. Le fichier est écrit à la main, `assign-slugs` renomme sous lui, et
/// tant que la redirection existe l'alias résout quand même : la faute est
/// entièrement invisible depuis le produit.
async fn alias_sur_slug_mort() -> Result<u64> {
    let vises: Vec<String> = lj_norm::aliases::canonical_slugs()
        .map(str::to_string)
        .collect();
    let conn = repo_conn().await?;
    let fautifs = DecisionRepository::new(&conn)
        .slugs_sans_texte_vivant(&vises)
        .await
        .map_err(|e| anyhow!("slugs_sans_texte_vivant: {e}"))?;
    for slug in &fautifs {
        tracing::warn!(slug, "alias pointing at a dead slug");
    }
    Ok(fautifs.len() as u64)
}

/// Compte les uid du dataset curé qu'aucun texte ne porte. Seules les entrées
/// sans slug sont visées : ailleurs le slug gagne, et l'uid n'est lu par rien.
/// C'est le pendant de [`alias_sur_slug_mort`] sur l'autre ancre, mais sans
/// filet — un uid ne redirige pas, il désigne ou ne désigne rien.
async fn alias_sur_uid_mort() -> Result<u64> {
    let vises: Vec<String> = lj_norm::aliases::canonical_uids()
        .map(str::to_string)
        .collect();
    let conn = repo_conn().await?;
    let fautifs = DecisionRepository::new(&conn)
        .uids_sans_texte(&vises)
        .await
        .map_err(|e| anyhow!("uids_sans_texte: {e}"))?;
    for uid in &fautifs {
        tracing::warn!(uid, "alias pointing at a dead uid");
    }
    Ok(fautifs.len() as u64)
}

/// Counts unanchored aliases whose text is in the catalogue after all. The two
/// checks above see an anchor die, never a text be born: twelve acronyms stayed
/// null for months after their text entered the corpus.
async fn alias_sans_ancre_resoluble() -> Result<u64> {
    let conn = repo_conn().await?;
    let repo = DecisionRepository::new(&conn);
    let mut fautifs = 0u64;
    for (alias, expansion) in lj_norm::aliases::unanchored() {
        let vise = repo
            .resolve_instrument(expansion, None)
            .await
            .map_err(|e| anyhow!("resolve_instrument: {e}"))?;
        let slug = match vise {
            InstrumentResolution::Direct { slug, .. }
            | InstrumentResolution::Rattrape { slug, .. } => slug,
            InstrumentResolution::Inconnu { .. } => continue,
        };
        tracing::warn!(
            alias,
            slug,
            "alias without an anchor, text in the catalogue"
        );
        fautifs += 1;
    }
    Ok(fautifs)
}

/// Counts the EU acts held twice under swapped number and year, and only those
/// the CELEX proves: the same swap matches seven pairs of really distinct acts,
/// 2015/2016 and 2016/2015 both existing, so the shape of the uids proves
/// nothing and this count would never reach zero if it took them in.
async fn uid_ue_permute() -> Result<u64> {
    let conn = repo_conn().await?;
    let twins = DecisionRepository::new(&conn)
        .eu_uid_twins()
        .await
        .map_err(|e| anyhow!("eu_uid_twins: {e}"))?;
    let mut fautifs = 0u64;
    for pair in &twins {
        let Some(canonique) = crate::pipeline::canonical_of(pair) else {
            continue;
        };
        let doublon = if canonique == pair.0.text_uid {
            &pair.1.text_uid
        } else {
            &pair.0.text_uid
        };
        tracing::warn!(canonique, doublon, "european act under two swapped uids");
        fautifs += 1;
    }
    Ok(fautifs)
}

/// Counts the EU datasets that would re-seed a uid the base has closed. The base
/// alone says nothing: the loader keys on the `text_uid` inside the file and not
/// on its name, so a dataset named after the JORFTEXT it came from survives a
/// merge and reopens the identity at the next sweep.
async fn dataset_ue_uid_ferme() -> Result<u64> {
    let settings = Settings::from_env()?;
    let conn = repo_conn().await?;
    let en_base: std::collections::HashSet<String> = DecisionRepository::new(&conn)
        .eu_text_uids()
        .await
        .map_err(|e| anyhow!("eu_text_uids: {e}"))?
        .into_iter()
        .collect();
    let fautifs = crate::pipeline::datasets_a_retirer(&settings.legal_corpus_dir(), &en_base)?;
    for (path, uid, nomme) in &fautifs {
        tracing::warn!(dataset = %path.display(), uid, nomme, "dataset reseeding a closed uid");
    }
    Ok(fautifs.len() as u64)
}

/// Compte les versions d'article qui ne sont pas situées entre leurs voisines :
/// sentinelle doublonnée, ou version qu'aucune suivante ne ferme. Dans les deux
/// cas deux textes se disent applicables le même jour et la lecture n'en sert
/// qu'un, sans le dire.
///
/// L'écriture situe la version, `reconcile` rattrape la chaîne une fois par
/// semaine et échoue s'il a eu à le faire : cet invariant est l'œil quotidien
/// entre deux passages, il ne répare rien.
async fn version_article_mal_situee() -> Result<u64> {
    let conn = repo_conn().await?;
    let (sentinelles, non_fermees) = DecisionRepository::new(&conn)
        .versions_mal_situees()
        .await
        .map_err(|e| anyhow!("versions_mal_situees: {e}"))?;
    if sentinelles + non_fermees > 0 {
        tracing::warn!(sentinelles, non_fermees, "article versions misplaced");
    }
    Ok(sentinelles + non_fermees)
}

/// Counts article versions whose end precedes their start: the row leaves every
/// dated read without a word, and `version_article_mal_situee` does not see it,
/// overlapping no successor. Stillborns excluded, so the expected count is zero.
async fn version_intervalle_inverse() -> Result<u64> {
    let conn = repo_conn().await?;
    let fautifs = DecisionRepository::new(&conn)
        .versions_intervalle_inverse()
        .await
        .map_err(|e| anyhow!("versions_intervalle_inverse: {e}"))?;
    if fautifs > 0 {
        tracing::warn!(fautifs, "article versions with an inverted interval");
    }
    Ok(fautifs)
}

/// Compte les sources suivies dont la dernière ingestion dépasse leur seuil.
async fn fraicheur_des_sources() -> Result<u64> {
    let conn = repo_conn().await?;
    let repo = DecisionRepository::new(&conn);

    let vues: std::collections::HashMap<String, chrono::NaiveDate> = repo
        .derniere_ingestion_par_source()
        .await
        .map_err(|e| anyhow!("derniere_ingestion_par_source: {e}"))?
        .into_iter()
        .collect();

    let aujourdhui = chrono::Utc::now().date_naive();
    let mut fautifs = 0;
    for (source, seuil) in SOURCES_SUIVIES {
        // Une source jamais vue est aussi fautive qu'une source arrêtée : dans
        // les deux cas la chaîne ne l'alimente pas.
        let jours = match vues.get(*source) {
            Some(dernier) => (aujourdhui - *dernier).num_days(),
            None => i64::MAX,
        };
        if jours > *seuil {
            fautifs += 1;
            tracing::warn!(source, jours, seuil, "source d'ingest muette");
        }
    }
    Ok(fautifs)
}
