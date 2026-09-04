//! Binaire `lj-ingest` : CLI d'ingest (clap).

mod alias_candidats;
mod analyze;
mod case_names;
mod chain;
mod chunking;
mod citations_maintenance;
mod config;
mod indexnow;
mod invariants;
mod logging;
mod pipeline;
mod renvois;
mod reverses;
mod sitemap;
mod summary;
mod tombstones;
mod usage_terms;

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use lj_telemetry::{InitOpts, OtlpCreds, TelemetryGuard};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::EnvFilter;

use crate::config::Settings;

/// Juridictions Judilibre par défaut.
const JUDILIBRE_DEFAULT_JURISDICTIONS: &[&str] = &["cc", "ca", "tj", "tcom"];
/// URL de prod de l'API PISTE Judilibre.
const JUDILIBRE_BASE_URL: &str = "https://api.piste.gouv.fr/cassation/judilibre/v1.0";

/// Construit un client Judilibre depuis les identifiants OAuth2 PISTE partagés
/// (`LIBREJUSTICE_PISTE_CLIENT_ID`/`_SECRET`, mêmes que Légifrance).
fn judilibre_client(settings: &Settings) -> Result<lj_sources::judilibre::JudilibreClient> {
    let client_id = settings
        .piste_client_id
        .clone()
        .ok_or_else(|| anyhow!("LIBREJUSTICE_PISTE_CLIENT_ID requis pour la source judilibre."))?;
    let client_secret = settings.piste_client_secret.clone().ok_or_else(|| {
        anyhow!("LIBREJUSTICE_PISTE_CLIENT_SECRET requis pour la source judilibre.")
    })?;
    Ok(lj_sources::judilibre::JudilibreClient::new(
        JUDILIBRE_BASE_URL,
        client_id,
        client_secret,
    ))
}

#[derive(Parser)]
#[command(
    name = "librejustice",
    about = "CLI ingest LibreJustice (pipelines offline)."
)]
struct Cli {
    /// Niveau de log global (trace/debug/info/warning/error ou entier).
    #[arg(
        long,
        global = true,
        env = "LIBREJUSTICE_LOG_LEVEL",
        default_value = "INFO"
    )]
    log_level: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Déroule une chaîne planifiée (cron). Chaque étape a son propre délai de
    /// watchdog et un échec n'interrompt pas les suivantes ; la chaîne sort
    /// non-zéro après un `error!` récapitulatif.
    Chain {
        #[arg(value_enum)]
        kind: chain::ChainKind,
    },
    /// Sync incrémental des sources (opendata ZIP + Judilibre).
    Sync,
    /// Calcule les plages de dates Judilibre à synchroniser.
    JudilibreRanges,
    /// Analyse statistique d'un corpus local.
    Analyze,
    /// Applique les migrations Postgres.
    Migrate,
    /// Projette les renvois des analyses du Conseil d'État en relation de
    /// citation (`decision_renvoi`) et résout les cibles.
    Renvois,
    /// Remplit `alias_candidate` : surnoms d'arrêts et de lois minés depuis les
    /// renvois du Conseil d'État et la locution « dite loi ». N'active rien.
    AliasCandidats {
        /// N'extrait rien : relit les N candidats de tête, par exclusivité.
        #[arg(long, num_args = 0..=1, default_missing_value = "60")]
        revue: Option<i64>,
    },
    /// Matérialise le champ `legal_article.usage_terms` (sacs de n-grammes
    /// des contextes de citation + seeds curatés).
    UsageTerms {
        /// Nombre de chunks du pool (grain de progression et de répartition).
        #[arg(long, default_value_t = 32)]
        chunks: usize,
        /// Seuil de citations pour matérialiser un sac.
        #[arg(long, default_value_t = 10)]
        min_citations: i64,
        /// Connexions parallèles (le coût est I/O-bound : lecture TOAST des
        /// full_text échantillonnés).
        #[arg(long, default_value_t = 6)]
        workers: usize,
    },
    /// Ingère les archives locales en base (parse → chunk → upsert).
    Ingest {
        #[arg(long)]
        with_embeddings: bool,
        /// Mode de triage. `missing-hash` (défaut) : skip si hash identique +
        /// fast-skip manifeste. `all` : relit tout, ignore hash et manifeste,
        /// force un UPDATE complet (ré-chunk/ré-embed après un changement du
        /// pipeline pur).
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
    },
    /// Re-fetch ciblé de décisions Judilibre par id (`/decision`) puis ingest.
    /// Répare quelques décisions désynchronisées (ex. résurrection) sans reculer
    /// le watermark global.
    Refetch {
        /// Ids Judilibre à re-fetch (ex. `6a28fc8ecdc6046d47cb0167`).
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long)]
        with_embeddings: bool,
    },
    /// Ré-extrait les champs structurés depuis les payloads stockés.
    ReextractFields {
        /// Sonde : n'écrit RIEN, compte les lignes que la passe réécrirait
        /// (même comparaison NULL-safe que le skip-tuple de l'UPDATE) et les
        /// couches satellites qu'elle purgerait.
        #[arg(long)]
        dry_run: bool,
        /// Passe intégrale : TOUT le fonds `extract_version < 1000`, même déjà à
        /// la version courante — c'est le relink hebdomadaire (un texte nouveau au
        /// catalogue attire ses citations anciennes ; le skip des sets de citations
        /// inchangés borne les écritures au delta).
        #[arg(long, conflicts_with_all = ["jurisdiction_type", "citing_ref_uid"])]
        full: bool,
        /// Cible des `jurisdiction_type` (CSV, ex. `CNDA,CEDH,CJUE,CONSTIT,TC`) à
        /// re-extraire **quelle que soit la `extract_version`** (sans bump global).
        /// Pour activer un comportement d'extraction nouveau sur un sous-ensemble.
        /// Vide = comportement par défaut (toutes les versions divergentes).
        #[arg(long, value_delimiter = ',')]
        jurisdiction_type: Vec<String>,
        /// Champs à re-extraire (CSV, ex. `legal_references`). Vide = tous les
        /// champs ré-extractibles. Restreindre à `legal_references` ne ré-écrit
        /// que les citations (via `replace_citations`), sans toucher les colonnes.
        #[arg(long, value_delimiter = ',')]
        field: Vec<String>,
        /// Cible les décisions **citant** ce texte (`ref_text_uid`), quelle que
        /// soit la `extract_version`. Pour rejouer extract+link sur le seul gisement
        /// d'un instrument, sans bump global.
        #[arg(long)]
        citing_ref_uid: Option<String>,
        /// Expression BM25 sur le texte (`parse_with_field`, termes pliés sans
        /// accent : `pourvoi`, `"decision deferee"`), avec
        /// `--jurisdiction-type` : ne repasse que le gisement du littéral.
        #[arg(long, requires = "jurisdiction_type")]
        text_query: Option<String>,
        /// Workers concurrents = connexions DB simultanées (chacun fait
        /// fetch→extract→write en parallèle). C'est le levier de pression sur la
        /// base : `--workers 2` reste doux sur une prod qui sert du trafic ; vide =
        /// défaut agressif (cœurs−4, borné [2,8]).
        #[arg(long)]
        workers: Option<usize>,
    },
    /// Replays the room resolution on the stored seat, without parsing:
    /// rewrites `seat_paths` where the court vocabulary moved.
    ResolveSeats {
        /// Counts the rows that would change, writes nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Re-embed ciblé des décisions orphelines (chunks sans embedding).
    /// Reconstruit depuis `(full_text, source_fields)` et ne remplit que les
    /// embeddings manquants, local strict.
    EmbedMissing {
        /// Borne le nombre de décisions traitées (défaut : toutes).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Purge les liens de réversion (reverses).
    PurgeReverses,
    /// Génère les résumés LLM manquants.
    GenerateSummaries {
        #[arg(long)]
        limit: Option<usize>,
        /// Appels Mistral concurrents (sémaphore). Le débit réel est cadencé
        /// par le throttle du client ; ce plafond borne juste les appels en vol.
        /// Défaut (absent) : clés vivantes × 7.
        #[arg(long)]
        concurrency: Option<usize>,
        /// Régénère AUSSI les résumés dont le prompt est plus ancien que la
        /// version courante (`summary_prompt_version < SUMMARY_PROMPT_VERSION`).
        /// Sans ce flag, seuls les résumés manquants (`summary IS NULL`) sont
        /// générés.
        #[arg(long)]
        refresh_stale: bool,
    },
    /// Recompute des arrays légaux dénormalisés depuis `legal_citation`
    /// (réparation). Idempotent, lots autocommit ; en régime normal l'écrivain
    /// de citations tient les arrays à jour.
    ResyncLegalArrays,
    /// Instantané unique du fonds : enchaîne les trois recomptes que rien ne
    /// tient à l'écriture, compteurs par juridiction, référentiel `chamber` et
    /// `corpus_counts`. Idempotent ; planifié dans la chaîne quotidienne.
    Precompute,
    /// Recompte `jurisdiction_year_counts` depuis `decisions` : les compteurs
    /// que lisent les hubs juridiction et les nombres de pages du sitemap.
    /// Idempotent.
    ResyncJurisdictionCounts,
    /// Recompte `corpus_counts` : total et facettes du fonds entier que sert le
    /// mode parcours, et les trois totaux de la page d'accueil. Idempotent.
    ResyncCorpusCounts,
    /// Recompose le référentiel `chamber` depuis `decisions` : une chambre par
    /// paire (cour, clé), sa spécialisation votée par le fonds. Idempotent.
    ResyncChambers,
    /// Re-key one-shot du namespace cnb: vers l'identité CNBF : ponte les uids
    /// legacy {barreau}:{nom}-{prenom} vers cnb:{avCnbfCode} (snapshot courant),
    /// purge les entités pieds de fichier, réhydrate les liens danglants.
    /// Dry-run par défaut ; préalable au premier reload upsert de
    /// `registries avocats`.
    RekeyCnb {
        /// Écrit réellement (sans ce flag : plan + compteurs, aucune écriture).
        #[arg(long)]
        apply: bool,
    },
    /// Rejeu des snapshots CNB publiés depuis 2022 : plie la présence observée
    /// en spans d'activité (come and go) et le siren de structure en arêtes
    /// d'affiliation datées, sous la provenance `cnb-hist`. Recalcule ses
    /// évidences en entier à chaque run et remplace les précédentes. Dry-run par
    /// défaut.
    CnbHistory {
        /// Écrit réellement (sans ce flag : plan + compteurs, aucune écriture).
        #[arg(long)]
        apply: bool,
    },
    /// Filet de dérive des acteurs, unique rattrapage de la relation :
    /// re-dérive `decision_party` depuis les colonnes NER de `decisions` et
    /// n'écrit que ce qui a changé — les sets inchangés gardent leurs
    /// `entity_uid`. Sharde sur N workers.
    PartiesReconcile {
        /// Workers concurrents (défaut : cœurs − 4, borné [2, 8]).
        #[arg(long)]
        workers: Option<usize>,
    },
    /// Affiliations avocat ↔ cabinet dérivées des appositions des décisions,
    /// sous la provenance `decision` : couples `firm_ord` dont les deux bouts
    /// sont résolus, attestés par ≥ 2 décisions distinctes. Fonction du corpus —
    /// recalcule en entier à chaque run. Dry-run par défaut.
    DeriveAffiliations {
        /// Écrit réellement (sans ce flag : compteurs, aucune écriture).
        #[arg(long)]
        apply: bool,
    },
    /// Filet unique de dérive : enchaîne tous les filets du régime « dérivé à
    /// l'écriture » — rôles de textes + dénorm articles, arrays légaux,
    /// liens/citations pendants. Idempotent, ~no-op en régime ; planifié en fin
    /// de chaîne dominicale. Le volet parties vit dans `parties-relink`.
    Reconcile,
    /// Génère les sitemaps et les publie en base (servis par lj-server).
    Sitemap {
        /// Construit les sitemaps sans les écrire en base (vérification locale).
        #[arg(long)]
        dry_run: bool,
    },
    /// Pousse les URLs modifiées via IndexNow.
    Indexnow {
        /// Pousse les décisions dont updated_at est plus récent que N heures.
        #[arg(long, default_value_t = 24.0)]
        since_hours: f64,
        /// Borne dure sur le nombre d'URLs soumises (évite de flooder IndexNow).
        #[arg(long, default_value_t = 10_000)]
        max_urls: usize,
    },
    /// Ingère un stock ou un incrément LEGI (référentiel versionné) depuis un
    /// `tar.gz` bulk DILA local (`Freemium_legi_global_*.tar.gz` ou incrément
    /// `LEGI_*.tar.gz`, téléchargé hors-bande).
    Legi {
        /// Chemin local de l'archive `tar.gz` LEGI.
        #[arg(required = true)]
        path: std::path::PathBuf,
    },
    /// Sync incrémental LEGI : télécharge les incréments `LEGI_*.tar.gz`
    /// postérieurs au watermark puis les ingère (le stock global se bootstrape
    /// hors-bande via `legi <path>`). Idempotent.
    SyncLegi,
    /// Backfill du graphe `legal_link` : streame des tarballs DILA locaux
    /// (stock global puis incréments, LEGI et/ou KALI, dans l'ordre
    /// chronologique) et remplace les arêtes `<LIENS>` par propriétaire, sans
    /// upsert d'articles (hors gate `content_checksum`). Rejouable ; purge les
    /// arêtes orphelines en fin de run.
    BackfillLinks {
        /// Archives `tar.gz` DILA, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Backfill de l'arbre structurel `legal_toc_edge` : streame des tarballs
    /// LEGI locaux (stock global puis incréments, ordre chronologique) et
    /// remplace les arêtes `TEXTELR`/`SECTION_TA` par propriétaire, hors gate
    /// `content_checksum`. Rejouable ; purge les arbres orphelins en fin de run.
    BackfillToc {
        /// Archives `tar.gz` LEGI, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Backfill des métadonnées de textes : rejoue l'upsert `legal_text` +
    /// `upcoming_versions` depuis les `TEXTE_VERSION` des tarballs LEGI locaux.
    /// Rejouable.
    BackfillTextes {
        /// Archives `tar.gz` LEGI, dans l'ordre chronologique.
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Ingère un stock ou un incrément KALI (conventions collectives
    /// nationales, bulk DILA) depuis un `tar.gz` local
    /// (`Freemium_kali_global_*.tar.gz` ou incrément `KALI_*.tar.gz`).
    /// KALICONT → `legal_text`, KALIARTI → `legal_article` (ancrés sur la
    /// convention). Idempotent.
    Kali {
        /// Chemin local de l'archive `tar.gz` KALI.
        #[arg(required = true)]
        path: std::path::PathBuf,
    },
    /// Sync incrémental KALI : cold start télécharge le stock global puis les
    /// incréments `KALI_*.tar.gz` postérieurs au watermark, chacun ingéré.
    /// Idempotent.
    SyncKali,
    /// Fait passer les conventions collectives à la clé qualifiée par la pièce :
    /// requalification sur place puis entrée des articles que l'ancienne clé
    /// refusait, un lot de recueils par passage. Reprenable et idempotent.
    RequalifierKali {
        /// Recueils traités par passage. Borne la mémoire du parcours du cache.
        #[arg(long, default_value_t = 200)]
        lot: i64,
    },
    /// Moisson quotidienne du fond marchés du pot Annonces : les 14 canaux
    /// (BOAMP Opendatasoft, TED, 11 moteurs de profils d'acheteurs, et le dump
    /// de presse quand une session en a laissé un) ré-énumérés en entier,
    /// réconciliés en lignes canoniques, upsertés. Aucun état stampé ni
    /// supprimé : la moisson n'écrit que ce qu'elle voit, la fenêtre vivante se
    /// calcule à la lecture. Idempotent.
    Annonces,
    /// Mesure la complétude de la moisson des plateformes : les consultations
    /// ouvertes que seuls les canaux de publication portent (BOAMP, TED,
    /// presse), classées par tenant et par cause — tenant non recensé, moteur
    /// non connecté, ou faux négatif d'un connecteur. Rapport pur, n'écrit rien.
    AnnoncesCouverture {
        /// Tenants détaillés dans le rapport, les plus gros d'abord.
        #[arg(long, default_value_t = 40)]
        limite: usize,
    },
    /// Backfill de l'historique BOAMP dans le fond `marche` : mois par mois,
    /// les avis de marché puis les résultats de la même fenêtre, depuis 2010.
    /// Manuel, jamais planifié. Idempotent et reprenable : relancer sur une
    /// fenêtre déjà passée ne réécrit rien.
    AnnoncesBackfill {
        /// Premier mois inclus (`YYYY-MM-DD`, ramené au 1er du mois). Défaut :
        /// 2010-01-01, le plancher du dataset. Après une coupure, reprendre au
        /// dernier `mois` du journal.
        #[arg(long)]
        depuis: Option<String>,
        /// Borne de fin exclue (`YYYY-MM-DD`).
        #[arg(long)]
        jusqu: String,
    },
    /// Enrichit les annonces ouvertes par leur page de détail : le code CPV et
    /// la procédure, que les 11 moteurs de scraping ne portent pas en liste.
    /// File dérivée (ouvertes, sans bloc `detail`, sans canal BOAMP), une page
    /// par annonce et une seule fois dans sa vie. Quotidien. Idempotent.
    AnnoncesEnrichir {
        /// Pages tirées au plus par run.
        #[arg(long, default_value_t = 2_000)]
        limite: i64,
        /// Minutes accordées à chaque moteur. Le défaut borne la quotidienne ; un
        /// rattrapage manuel monte le plafond pour vider une file rationnée d'une
        /// traite, à la cadence de son hôte.
        #[arg(long, default_value_t = 15)]
        budget_min: u64,
        /// Rattrapage : tire les annonces sans libellé d'acheteur au lieu des
        /// non visitées. Rouvre les fiches lues avant que leur extracteur ne
        /// sache lire l'acheteur (Atexo). Manuel, jamais planifié.
        #[arg(long, default_value_t = false)]
        sans_acheteur: bool,
    },
    /// Recense les profils d'acheteurs cités par les avis BOAMP actifs et les
    /// classe par moteur (`profil_acheteur`) — les moteurs multi-tenants
    /// (Atexo, Safetender, Modula) en dépendent pour savoir quels hôtes énumérer.
    /// Hebdomadaire. Idempotent.
    AnnoncesDiscover,
    /// Tient le stock `marche` en conformité : chaque ligne orpheline (uid
    /// abandonné par un re-groupage, plus aucun canal) est résolue par ses
    /// `details.liens` vers la ligne qui porte aujourd'hui ses canaux, puis
    /// absorbée (redirection 301 + suppression). Étape quotidienne de la chaîne.
    /// Idempotent.
    AnnoncesRegulariser {
        /// Réapplique aussi les règles de forme (titres, acheteurs, lots
        /// d'attribution) à tout le stock — le filet hebdomadaire qui porte
        /// une règle nouvelle au stock clos, que la moisson ne revisite pas.
        #[arg(long)]
        formes: bool,
    },
    /// Va chercher sur la plateforme, par la référence de marché que l'avis
    /// publie, les consultations des lignes `marche` ouvertes qui n'ont que la
    /// racine d'un hôte. L'énumération d'une instance et son index de recherche
    /// ne couvrent pas le même ensemble : une consultation absente de
    /// la liste sort de la recherche. Les lignes trouvées entrent par le chemin
    /// d'écriture de la moisson ; c'est `annonces-jumeler` qui les rattache.
    /// **Dry-run par défaut**. Idempotent.
    AnnoncesResoudre {
        /// Écrit réellement les lignes trouvées. Sans ce flag : dry-run.
        #[arg(long)]
        applique: bool,
        /// Plafond de références interrogées (0 = pas de plafond).
        #[arg(long, default_value_t = 0)]
        limite: i64,
    },
    /// Fusionne par leur jumelle les lignes `marche` ouvertes que seul le
    /// BOAMP publie et dont l'avis déclare une plateforme de dépôt **sans
    /// identifiant de dossier** : sans clé de jointure, la seule attache est
    /// l'annonce que cette plateforme publie sous les mêmes titre et acheteur.
    /// La ligne plateforme est l'élue, la ligne BOAMP est absorbée (redirection
    /// 301 + suppression). **Dry-run par défaut** : sans `--applique`, la passe
    /// journalise les paires et n'écrit rien. Idempotent.
    AnnoncesJumeler {
        /// Applique réellement les fusions (write). Sans ce flag : dry-run.
        #[arg(long)]
        applique: bool,
    },
    /// Ingère un export JSONL `bofip-vigueur` local (doctrine fiscale DGFiP) :
    /// document BOI → `legal_text` (nature=BOFIP, préambule en `body`), §
    /// numérotés → `legal_article`. Idempotent (snapshot : purge des versions/§
    /// hors export).
    Bofip {
        /// Chemin local de l'export `bofip-vigueur.jsonl`.
        #[arg(required = true)]
        path: std::path::PathBuf,
    },
    /// Sync BOFiP : re-télécharge l'export open data `bofip-vigueur`
    /// (data.economie.gouv.fr, snapshot des publications en vigueur) puis
    /// l'ingère. Idempotent.
    SyncBofip,
    /// Sync du fond DILA CIRCULAIRES : stocks 2009-2014 + abrogations
    /// historiques + flux chronologiques (last-write-wins par ID) →
    /// `legal_text` nature=CIRCULAIRE (métadonnées + résumé ; corps = passe
    /// séparée sync-circulaires-bodies). Reprend au dernier fichier ingéré
    /// (manifest). Idempotent.
    SyncCirculaires,
    /// Passe corps des circulaires : stocks PDF + PDF compagnons des flux
    /// (last-write-wins par ID) → `legal_text.body` (pdftotext natif, OCR
    /// Mistral cache-first en repli scanné). Reprend au dernier tarball rejoué
    /// (manifest pdf). Idempotent.
    SyncCirculairesBodies,
    /// Corps des traités (TRAITE) et accords interprofessionnels (TI) depuis
    /// les blocs sans numéro des stocks bulk JORF/KALI du cache :
    /// concaténation dans l'ordre du document → `legal_text.body` des fiches
    /// sans articles ni corps. Aucun téléchargement. Idempotent.
    BackfillTreatyBodies,
    /// Sync ArianeWeb, les trois fonds en un job : le recensement (une requête
    /// par fond, facette d'année comptée serveur) désigne les décisions CE que le
    /// fonds n'a pas et les années dont les commentaires ont bougé, et rien
    /// d'autre n'est visité. Cache HTML sous state_dir. Idempotent.
    SyncAriane {
        /// Années à traiter (défaut : celles que le recensement désigne).
        #[arg(long, value_delimiter = ',')]
        years: Vec<u16>,
        /// S'arrêter au tableau du recensement, sans rien télécharger.
        #[arg(long)]
        census_only: bool,
        /// Plafond de documents AJCE téléchargés (sonde) : le run s'arrête
        /// proprement sans marquer d'année au manifeste.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Sync des grandes décisions que le Conseil d'État publie : le titre de
    /// l'entrée nomme la décision, le lien ArianeWeb de sa fiche l'ancre
    /// → `decisions.case_name` en `case_name_source = 'ce-list'`. Cache des
    /// fiches sous state_dir. Idempotent.
    SyncCeList,
    /// Sync ADDE (plan commentaires généralisé) : analyses de jurisprudence de
    /// l'ADDE → commentaires doctrine web `kind:"note"` (liens sortants) des
    /// décisions rattachées par (dossier, date). Idempotent, non cherchable.
    SyncAdde,
    /// Charge les commentaires de norme curés depuis
    /// `<state_dir>/ingest/article-commentaires.json` → `article_commentaire`
    /// (liens `kind:"note"` sur des articles de loi/traité). Entrées ancrées par
    /// `slug` d'URL, résolues et vérifiées à chaque passage ; les entrées
    /// absentes du dataset sont tombstonées. Idempotent, réconciliant.
    SeedArticleCommentaires,
    /// Extrait les renvois norme→article (text_legal_citation) et les
    /// citations de jurisprudence (text_case_citation) de TOUS les corps du
    /// référentiel — versions d'articles + corps monolithiques — en une passe
    /// doc_extract. Idempotent.
    ExtractTextRefs,
    /// Ingère un stock ou un incrément JORF (Journal officiel, bulk DILA)
    /// depuis un `tar.gz` local (`Freemium_jorf_global_*.tar.gz` ou incrément
    /// `JORF_*.tar.gz`). Deux passes : textes (`referential_texts`, tag
    /// `treaty`/`jorf`) puis articles numérotés (`referential_articles`).
    /// Idempotent.
    Jorf {
        /// Chemins locaux d'archives `tar.gz` JORF (traitées dans l'ordre,
        /// finisseurs une fois en fin de lot).
        #[arg(required = true)]
        paths: Vec<std::path::PathBuf>,
    },
    /// Sync du fond JORF complet (fiches + rôles + liens) : télécharge
    /// stock/incréments DILA postérieurs au watermark puis les ingère (mécanique
    /// `sync-legi`). Idempotent, reprenable.
    SyncJorf,
    /// Assigne les slugs manquants de `legal_text` : backfill puis no-op (la
    /// passe tourne aussi en fin de chaque ingest référentiel).
    AssignSlugs,
    /// Recalcule `legal_text.role` (classification v1 conservatrice) et aligne
    /// `legal_link.verb` sur le repli verbe/nom courant. Idempotent.
    BackfillTextRoles,
    /// Déménage la chronique LEGI d'un instrument consolidée par DILA sous le
    /// cid du véhicule : lignes LEGIARTI + arêtes TOC vers la fiche instrument
    /// (arête publie). Liste fermée vérifiée à la main ; re-runnable après une
    /// retouche DILA du dossier.
    MoveChronicle {
        /// Uid JORFTEXT du véhicule dont le dossier LEGI porte la chronique.
        #[arg(long)]
        vehicle: String,
        /// Couples `<num_key>@<YYYY-MM-DD>` qui restent au véhicule (jumeaux
        /// LEGI du dispositif dans un dossier mixte). Répétable.
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Re-frappe des uids de fiches par dataset JSONL
    /// `{from, to, jurisdiction?}` (droit étranger par ordre juridique, stubs
    /// EUR-Lex → EU/). Cascade articles/TOC/liens/citations texte, slug
    /// inchangé ; suivre du drain reextract-fields --full --field
    /// legal_references. Résumable.
    RenameTexts {
        /// Chemin du dataset JSONL de re-frappe.
        dataset: std::path::PathBuf,
    },
    /// Rebase les clés d'article (num_key et colonnes liées) vers la clé
    /// publique slug. One-shot, idempotent.
    RekeyArticleKeys,
    /// Rebase `legal_article.num_key` (+ liens possédés) et
    /// `legal_toc_edge.child_num_key` vers la clé d'IDENTITÉ recalculée depuis
    /// le num source. One-shot, idempotent.
    RekeyIdentityKeys,
    /// Retire l'astérisque Légifrance (`R*600-1`) des clés d'article, colonnes
    /// porteuses et blobs de citation compris ; `num` la garde. **Dry-run par
    /// défaut**. Idempotent.
    DestarArticleKeys {
        /// Applique réellement le désétoilage. Sans ce flag : dry-run.
        #[arg(long)]
        applique: bool,
    },
    LoadLegalCorpus {
        /// Restreint le chargement aux datasets dont le nom contient cette sous-chaîne
        /// (chargement chirurgical, ex. `--only eu-rproc`). Défaut : tous les datasets.
        #[arg(long)]
        only: Option<String>,
    },
    /// Génère les datasets catalogue EUR-Lex (règlements/directives UE cités
    /// mais absents) sous `<state_dir>/ingest/corpus/`, via SPARQL Cellar,
    /// pilotés par les slashnums réellement cités (spans non liés de
    /// `legal_citation`). Entrées catalogue-seul (`articles: []`) ; auto-validées
    /// (le titre FR doit porter le slashnum cité). À enchaîner avec
    /// `load-legal-corpus` ; les citations s'attachent à la passe intégrale
    /// suivante. `--limit` borne aux N slashnums les plus cités (rodage).
    /// Idempotent (skip datasets présents).
    IngestEuCatalog {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Balayage du registre EUR-Lex : tout règlement ou directive (secteur 3,
    /// actes de base, délégués et d'exécution) publié depuis `--depuis` entre au
    /// catalogue, cité ou non, en entrée catalogue-seul chargée dans la foulée.
    /// C'est l'autre porte que `ingest-eu-catalog`, qui ne voit que ce que nos
    /// décisions nomment. `--depuis` défaut à 90 jours : la fenêtre se recouvre
    /// d'un jour sur l'autre, ce qui rattrape une passe manquée sans watermark.
    /// Un rattrapage de fond se demande par une date ancienne. Idempotent.
    SyncEuCatalog {
        #[arg(long)]
        depuis: Option<chrono::NaiveDate>,
        #[arg(long = "jusqu-a")]
        jusqu_a: Option<chrono::NaiveDate>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Third door onto the EU corpus, the one the French JO opens: every DILA
    /// fiche of European nature is a shell, an act named with no body. Each is
    /// paired with its EUR-Lex act by the number its title carries, that act is
    /// minted at the catalogue when the corpus lacks it, then the shell
    /// collapses into it, carrying over the date and the NOR that EUR-Lex does
    /// not publish and leaving its slug as a redirect. `--limit` bounds the pass
    /// (rodage). Idempotent, the merged shells leaving the source query.
    SyncEuJorf {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Merges the `EU/` fiches an old ingest left under two uids, number and year
    /// swapped, the CELEX their articles carry naming the one that survives. The
    /// other keeps its slug as a redirect, and its body moves when it is the one
    /// that has it. Dry run listing the pairs unless `--apply`. One-off: no code
    /// produces the swapped form any more.
    MergeEuSwappedUids {
        #[arg(long)]
        apply: bool,
    },
    /// Sync quotidien du fond UE : chaque acte du registre (datasets
    /// `eu-*.json`) suit sa dernière version consolidée applicable, sourcée en
    /// Formex 4 via CELLAR (notice branch → manifestation `.FRA.fmx4` → item
    /// cellar). En régime, un **delta SPARQL** (consolidations parues depuis le
    /// watermark + versions futures à échéance) borne les sondes aux actes
    /// touchés ; `--full` force le balayage intégral des notices (filet hebdo
    /// auto-réparant, aussi le bootstrap quand le manifeste est vierge). Seuls
    /// les datasets dont la version disponible diffère du marqueur
    /// `consolidated_celex` sont régénérés puis rechargés (loader legal-corpus,
    /// purge par texte). Les actes sans Formex gardent leur stock. Le skip est
    /// gagé sur l'état DB (celex de `source_url`), pas sur le fichier.
    /// `--limit` borne aux N premiers actes, `--only` filtre par sous-chaîne du
    /// nom de dataset (rodage) — tous deux sans effet sur le manifeste —,
    /// `--force` re-fetche et recharge tout.
    SyncEuLaw {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        only: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        full: bool,
    },
    /// Ingestion catalogue des règlements de procédure des juridictions UE
    /// (Cour de justice, Tribunal, ex-TFP). Écrit des datasets catalogue curés
    /// (variantes = alias TSV du linker) ; enchaîner avec `load-legal-corpus`.
    /// `--dry-run` liste sans écrire. Idempotent.
    IngestEuRproc {
        #[arg(long)]
        dry_run: bool,
    },
    /// Rafraîchit la fraîcheur « as-of » des sources *live* autoritaires
    /// (legifrance/kali/jorf) à la date du jour dans `ingest_freshness` :
    /// re-confirme que la copie reflète le droit en vigueur. À lancer après
    /// chaque ingest LEGI/KALI/JORF (cron). Idempotent (une ligne par source).
    StampFreshness,
    /// Joue les invariants de fin de chaîne : des contrôles de données qui
    /// publient leur nombre de lignes fautives dans `ingest.invariant.breach`
    /// et nomment chaque fautif au journal. Ne fait jamais échouer la chaîne.
    Invariants,
    /// Recalcule l'ordre de lecture (`legal_article.reading_rank`) des textes
    /// sales — ceux dont un article a un rang `NULL`. Appelé en fin de chaque
    /// pipeline qui écrit des articles ; en manuel pour le rattrapage.
    /// Idempotent : une seconde passe n'écrit rien.
    ReadingRanks,
    /// Reclasse les `source` catégorie/méthode hérités (`treaty`, `eu-law`,
    /// `official-fr`, `traduction-automatique`) en libellés de diffuseur réels,
    /// dérivés de `source_url`. Idempotent ; lignes sans URL hors bulk JORF
    /// laissées au reload de curation (loguées). One-shot après déploiement.
    RelabelSources,
    /// Canonicalise les variantes d'hôte d'un diffuseur (`jafbase.fr` →
    /// `jafbase`) vers son libellé unique. Idempotent. One-shot
    /// post-déploiement.
    CanonicalizeSourceLabels,
    /// Ingère un fond bulk DILA (`tar.gz` locaux sous
    /// `<cache>/dila/<fond>/tarballs/`) : `jade` (admin CE/CAA/TC), `constit`
    /// (Conseil constitutionnel). Idempotent ; dédup par `canonical_ref`.
    IngestDila {
        /// Fond DILA à ingérer.
        #[arg(value_enum)]
        fond: pipeline::Fond,
    },
    /// Sync incrémental d'un fond DILA (télécharge les incréments postérieurs
    /// au watermark). N'ingère rien — `ingest-dila` consomme les archives.
    SyncDila {
        /// Fond DILA à synchroniser.
        #[arg(value_enum)]
        fond: pipeline::Fond,
    },
    /// Backfill de la colonne `decisions.ecli` depuis
    /// `source_fields->>'ecli'` : expose l'ECLI sur les lignes historiques
    /// (Judilibre le porte dans son payload, colonne NULL).
    /// Keyset batché, idempotent.
    BackfillEcli,
    /// Construit le vocabulaire d'autocomplétion : n-grammes 1-3 comptés sur
    /// un échantillon des décisions + articles + titres, FST déposé en blob dans
    /// `suggest_index`. Relançable (remplace le blob).
    BuildSuggest,
    /// Re-dérive `decisions.canonical_ref`. Sans `--force` : ne traite que les
    /// `NULL` (peuplement). Avec `--force` : **re-dérive toutes** les décisions
    /// (`full_text` présent) pour migrer les clés 3-champs historiques
    /// `{nom}|{rg}|{date}` en 4-champs `{type}|{location}|{rg}|{date}`. Pur
    /// calcul, aucun re-embed ; une clé `None` laisse l'existante (jamais
    /// d'écrasement vers `NULL`).
    BackfillCanonicalRef {
        /// Re-dérive toutes les décisions (pas seulement `canonical_ref IS NULL`).
        #[arg(long)]
        force: bool,
    },
    /// Fusion rétroactive des **faux splits cross-source** : décisions
    /// distinctes au même `canonical_ref` avec sources **disjointes** = même
    /// décision multi-sources. Garde l'autorité (rang max), fusionne les autres
    /// dedans. Jamais de fusion intra-source. Aucun re-embed. Idempotent.
    MergeCrossSourceDuplicates,
    /// Re-ingest **ciblé** des décisions opendata dont l'autorité a basculé
    /// sur opendata mais dont le `full_text` reste figé sur une provenance de
    /// rang inférieur (jade/constit/cedh/cjue/cnda). Re-parse leur payload
    /// opendata (seuls les ZIPs portant une cible sont ouverts) en mode All :
    /// re-chunk + re-embed (local strict) + réécriture du `full_text` opendata.
    /// One-shot, hors cron. Idempotent.
    ReingestStaleOpendata,
    /// Analyse **read-only** des faux merges judilibre : recalcule
    /// `canonical_ref` par provenance sur les décisions multi-provenances
    /// tout-judilibre et compte légitimes / ambiguës / faux merges (clés
    /// divergentes). N'écrit rien — mesure l'ampleur avant le re-split.
    AnalyzeFalseMerges,
    /// Réparation des faux merges judilibre. **`--dry-run` par défaut**
    /// (read-only) : recalcule `canonical_ref` par provenance, planifie le
    /// scinde (groupe gardé = celui de l'autorité ; groupes divergents à
    /// reconstituer) et émet le PLAN sans **aucune** écriture. `--execute`
    /// (write, gaté JADE + GPU libre) : crée les décisions scindées, re-fetch
    /// les `source_uid` divergents + le groupe gardé + ré-ingère (re-chunk +
    /// re-embed ciblé local). Transaction par cluster, reprise idempotente.
    /// `--limit` borne le 1ᵉʳ rollout (s'arrête après N clusters réparés).
    ResplitFalseMerges {
        /// Applique réellement le scinde (write). Sans ce flag : dry-run read-only.
        #[arg(long)]
        execute: bool,
        /// Taille de l'échantillon de plan imprimé.
        #[arg(long, default_value_t = 25)]
        audit_sample: usize,
        /// Borne le nombre de clusters **réparés** (write) avant arrêt — rollout
        /// prudent. Sans valeur : pas de borne. Ignoré en dry-run.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Défait les faux merges **intra-source** judilibre : détache chaque
    /// provenance perdante (≥2 judilibre/décision) vers sa propre décision et la
    /// re-matérialise **depuis le cache local** (re-chunk + re-embed local).
    UnmergeSameSource {
        /// Applique réellement (write + GPU). Sans ce flag : dry-run read-only.
        #[arg(long)]
        execute: bool,
        /// Borne le nombre de splits écrits avant arrêt (rollout prudent). Sans
        /// valeur : pas de borne. Ignoré en dry-run.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Défait les faux merges **intra-source** `dila-jade` : analogue de
    /// `unmerge-same-source` pour le fond DILA JADE. Re-matérialise chaque
    /// perdante depuis les **tarballs locaux** (streaming, re-embed local).
    UnmergeSameSourceDila {
        /// Applique réellement (write + GPU). Sans ce flag : dry-run read-only.
        #[arg(long)]
        execute: bool,
        /// Borne le nombre de splits écrits avant arrêt (rollout prudent). Sans
        /// valeur : pas de borne. Ignoré en dry-run.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Bootstrap CEDH (HUDOC) : balaye toutes les années, fetch les arrêts FR
    /// (HFJUD) et les ingère. Idempotent par `content_checksum`.
    IngestCedh {
        /// Première année balayée (défaut : 1960). Backfill ciblé d'un trou
        /// d'ingest sans re-lister le fonds ancien.
        #[arg(long)]
        from_year: Option<i32>,
    },
    /// Réchauffe le cache disque des corps CEDH (`<cache_dir>/cedh/bodies/`) sans
    /// toucher la base — découple le fetch réseau throttlé de l'ingest. Ensuite
    /// `ingest-cedh` relit le cache (sans réseau), seul l'embedding reste.
    CacheCedh {
        /// Première année réchauffée (défaut : 1960).
        #[arg(long)]
        from_year: Option<i32>,
    },
    /// Bootstrap CJUE (EUR-Lex) : balaye toutes les années, fetch le texte FR
    /// des arrêts/ordonnances et les ingère. Idempotent.
    IngestCjue,
    /// Sync incrémental CEDH : ne balaye que l'année courante.
    SyncCedh,
    /// Sync incrémental CJUE : ne balaye que l'année courante.
    SyncCjue,
    /// Bootstrap CNDA (Cour nationale du droit d'asile) : crawl la liste
    /// jurisprudentielle paginée de `cnda.fr`, fusionne fiche HTML + PDF lié par
    /// numéro et les ingère (`source_uid = cnda/<numero>`, ECLI fabriqué).
    /// Idempotent par `content_checksum`. `--mode all` force la ré-extraction
    /// complète (re-OCR + ré-chunk/ré-embed) de toutes les décisions, même à
    /// PDF inchangé. `--only <numéro>` (répétable) : ne ré-ingère que ces
    /// décisions précises — crawl la liste mais ne traite/persiste que les
    /// numéros visés (re-OCR forcé, ne touche pas le watermark).
    IngestCnda {
        #[arg(long, value_enum, default_value_t = pipeline::IngestMode::MissingHash)]
        mode: pipeline::IngestMode,
        #[arg(long)]
        only: Vec<String>,
    },
    /// Sync incrémental CNDA : reprend le crawl à la page suivant le
    /// watermark.
    SyncCnda,
    /// Sonde JuriCAF : charge les pages « arrêt » téléchargées sous
    /// `<state_dir>/ingest/juricaf-probe/html/` (repérage du fond francophone,
    /// référentiel `jurisdiction` minté depuis l'URN:LEX des pages).
    JuricafProbe {
        /// Dossier des pages HTML (défaut : `<state_dir>/ingest/juricaf-probe/html`).
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Backfill `article_scope` depuis les compteurs d'applicabilité
    /// outre-mer : `etend`/`ecarte` vers NC/PF/WF/TF, version cible résolue par
    /// l'ancre de rédaction ou gelée à la date du compteur. Dry-run par défaut.
    ScopeBackfillCompteurs {
        /// Collectivité : PF, NC, WF ou TF.
        #[arg(long, default_value = "PF")]
        collectivite: String,
        /// Restreint à un texte (LEGITEXT…) pour un pilote ciblé.
        #[arg(long)]
        text_uid: Option<String>,
        /// Écrit en base (sinon dry-run).
        #[arg(long)]
        apply: bool,
    },
    /// Backfill Alsace-Moselle : clauses des trois départements →
    /// restreint/ecarte `geo:FR-ALS`, textes entièrement locaux →
    /// `legal_text.default_scope`. Dry-run par défaut.
    ScopeBackfillAlsace {
        /// Écrit en base (sinon dry-run).
        #[arg(long)]
        apply: bool,
    },
    /// Backfill `solution_uid` dans les `source_fields` CNDA : les provenances
    /// écrites avant que le parser ne pose la clé ne la portent pas, et la
    /// ré-extraction rend alors la déduction générique au lieu du dispositif lu.
    /// Recalculé depuis `(full_text, source_fields)`, sans re-télécharger.
    /// Dry-run par défaut.
    BackfillCndaSolution {
        /// Écrit en base (sinon dry-run).
        #[arg(long)]
        apply: bool,
    },
    /// Charge un registre d'entités : SIRENE, RNA, annuaire des avocats (CNB)
    /// ou avocats aux Conseils (`oacc:`, snapshot curé) — découverte data.gouv +
    /// download streamé (bulk) ou lecture du JSON curé sous
    /// `ingest/corpus/registries/`, remplacement de namespace.
    Registries {
        #[arg(value_enum)]
        source: pipeline::RegistrySource,
        /// SIRENE : charge aussi l'historique des dénominations (périodes closes).
        #[arg(long)]
        with_history: bool,
    },
    /// Résout les clés pendantes decision_party → entity (post-rechargement
    /// mensuel de registre).
    PartiesRelink {
        /// Toutes les passes pleines (dépliage SIRENE/RNA) — one-shot post-reload
        /// de registre. Sans le flag : passe « cabinets normalisés » + recount
        /// annuaire (hebdo).
        #[arg(long)]
        full: bool,
    },
    /// Charge la vérité terrain du quantum en base, à `PC_GOLD_VERSION`. La
    /// source reste les JSON versionnés en LFS. Idempotent.
    LoadQuantumGold {
        /// Répertoire des records, un JSON par décision nommé de son
        /// `source_uid`, barre doublée en tirets bas.
        #[arg(long)]
        dir: std::path::PathBuf,
    },
    /// Maintenance DB (sous-commandes).
    #[command(subcommand)]
    Db(DbCommand),
}

#[derive(Subcommand)]
enum DbCommand {
    /// ANALYZE des tables.
    Analyze,
    /// Maintenance hebdomadaire des tables de recherche : REINDEX TABLE
    /// CONCURRENTLY + VACUUM, sans indisponibilité.
    ReindexSearch,
    /// VACUUM FULL des tables de recherche — repli MANUEL : compacte heap et
    /// TOAST en rendant l'espace à l'OS, au prix d'un lock exclusif par table.
    /// Hors chaîne planifiée.
    VacuumFull,
    /// GC des tokens/clients MCP expirés.
    GcMcp,
    /// Préchauffe le cache (prewarm).
    Prewarm,
    /// Purge les tombstones **orphelins** (retraits totaux : retirés, vidés,
    /// sans provenance active). Dry-run par défaut ; `--apply` pour supprimer.
    PurgeOrphanTombstones {
        /// Supprime réellement. Sans ce flag : compte + échantillon, aucune
        /// suppression.
        #[arg(long)]
        apply: bool,
    },
}

impl Command {
    /// Libellé stable de phase pour les breadcrumbs / le watchdog (clé de requête
    /// Loki : `phase="ingest"`). Statique, indépendant des args.
    fn label(&self) -> &'static str {
        match self {
            Command::Chain { .. } => "chain",
            Command::Sync => "sync",
            Command::JudilibreRanges => "judilibre-ranges",
            Command::Analyze => "analyze",
            Command::Migrate => "migrate",
            Command::Renvois => "renvois",
            Command::AliasCandidats { .. } => "alias-candidats",
            Command::UsageTerms { .. } => "usage-terms",
            Command::Ingest { .. } => "ingest",
            Command::Refetch { .. } => "refetch",
            Command::ReextractFields { .. } => "reextract-fields",
            Command::ResolveSeats { .. } => "resolve-seats",
            Command::EmbedMissing { .. } => "embed-missing",
            Command::PurgeReverses => "purge-reverses",
            Command::GenerateSummaries { .. } => "generate-summaries",
            Command::ResyncLegalArrays => "resync-legal-arrays",
            Command::Precompute => "precompute",
            Command::ResyncJurisdictionCounts => "resync-jurisdiction-counts",
            Command::ResyncCorpusCounts => "resync-corpus-counts",
            Command::ResyncChambers => "resync-chambers",
            Command::RekeyCnb { .. } => "rekey-cnb",
            Command::CnbHistory { .. } => "cnb-history",
            Command::PartiesReconcile { .. } => "parties-reconcile",
            Command::DeriveAffiliations { .. } => "derive-affiliations",
            Command::Reconcile => "reconcile",
            Command::Sitemap { .. } => "sitemap",
            Command::Indexnow { .. } => "indexnow",
            Command::Legi { .. } => "legi",
            Command::SyncLegi => "sync-legi",
            Command::BackfillLinks { .. } => "backfill-links",
            Command::BackfillToc { .. } => "backfill-toc",
            Command::BackfillTextes { .. } => "backfill-textes",
            Command::Kali { .. } => "kali",
            Command::SyncKali => "sync-kali",
            Command::RequalifierKali { .. } => "requalifier-kali",
            Command::Annonces => "annonces",
            Command::AnnoncesCouverture { .. } => "annonces-couverture",
            Command::AnnoncesBackfill { .. } => "annonces-backfill",
            Command::AnnoncesEnrichir { .. } => "annonces-enrichir",
            Command::AnnoncesDiscover => "annonces-discover",
            Command::AnnoncesRegulariser { .. } => "annonces-regulariser",
            Command::AnnoncesResoudre { .. } => "annonces-resoudre",
            Command::AnnoncesJumeler { .. } => "annonces-jumeler",
            Command::Bofip { .. } => "bofip",
            Command::SyncBofip => "sync-bofip",
            Command::SyncCirculaires => "sync-circulaires",
            Command::SyncCirculairesBodies => "sync-circulaires-bodies",
            Command::BackfillTreatyBodies => "backfill-treaty-bodies",
            Command::SyncAriane { .. } => "sync-ariane",
            Command::SyncCeList => "sync-ce-list",
            Command::SyncAdde => "sync-adde",
            Command::SeedArticleCommentaires => "seed-article-commentaires",
            Command::ExtractTextRefs => "extract-text-refs",
            Command::Jorf { .. } => "jorf",
            Command::SyncJorf => "sync-jorf",
            Command::AssignSlugs => "assign-slugs",
            Command::BackfillTextRoles => "backfill-text-roles",
            Command::MoveChronicle { .. } => "move-chronicle",
            Command::RenameTexts { .. } => "rename-texts",
            Command::RekeyArticleKeys => "rekey-article-keys",
            Command::RekeyIdentityKeys => "rekey-identity-keys",
            Command::DestarArticleKeys { .. } => "destar-article-keys",
            Command::LoadLegalCorpus { .. } => "load-legal-corpus",
            Command::IngestEuCatalog { .. } => "ingest-eu-catalog",
            Command::SyncEuCatalog { .. } => "sync-eu-catalog",
            Command::SyncEuJorf { .. } => "sync-eu-jorf",
            Command::MergeEuSwappedUids { .. } => "merge-eu-swapped-uids",
            Command::SyncEuLaw { .. } => "sync-eu-law",
            Command::IngestEuRproc { .. } => "ingest-eu-rproc",
            Command::StampFreshness => "stamp-freshness",
            Command::Invariants => "invariants",
            Command::ReadingRanks => "reading-ranks",
            Command::RelabelSources => "relabel-sources",
            Command::CanonicalizeSourceLabels => "canonicalize-source-labels",
            Command::IngestDila { .. } => "ingest-dila",
            Command::SyncDila { .. } => "sync-dila",
            Command::BackfillEcli => "backfill-ecli",
            Command::BuildSuggest => "build-suggest",
            Command::BackfillCanonicalRef { .. } => "backfill-canonical-ref",
            Command::MergeCrossSourceDuplicates => "merge-cross-source-duplicates",
            Command::ReingestStaleOpendata => "reingest-stale-opendata",
            Command::AnalyzeFalseMerges => "analyze-false-merges",
            Command::ResplitFalseMerges { .. } => "resplit-false-merges",
            Command::UnmergeSameSource { .. } => "unmerge-same-source",
            Command::UnmergeSameSourceDila { .. } => "unmerge-same-source-dila",
            Command::IngestCedh { .. } => "ingest-cedh",
            Command::CacheCedh { .. } => "cache-cedh",
            Command::IngestCjue => "ingest-cjue",
            Command::SyncCedh => "sync-cedh",
            Command::SyncCjue => "sync-cjue",
            Command::IngestCnda { .. } => "ingest-cnda",
            Command::SyncCnda => "sync-cnda",
            Command::JuricafProbe { .. } => "juricaf-probe",
            Command::ScopeBackfillCompteurs { .. } => "scope-backfill-compteurs",
            Command::ScopeBackfillAlsace { .. } => "scope-backfill-alsace",
            Command::BackfillCndaSolution { .. } => "backfill-cnda-solution",
            Command::Registries { .. } => "registries",
            Command::PartiesRelink { .. } => "parties-relink",
            Command::LoadQuantumGold { .. } => "load-quantum-gold",
            Command::Db(DbCommand::Analyze) => "db:analyze",
            Command::Db(DbCommand::ReindexSearch) => "db:reindex-search",
            Command::Db(DbCommand::VacuumFull) => "db:vacuum-full",
            Command::Db(DbCommand::GcMcp) => "db:gc-mcp",
            Command::Db(DbCommand::Prewarm) => "db:prewarm",
            Command::Db(DbCommand::PurgeOrphanTombstones { .. }) => "db:purge-orphan-tombstones",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = Settings::from_env()?;

    let _telemetry_guard = init_telemetry(&cli.log_level, &settings)?;
    install_panic_hook();

    let result = match cli.command {
        Command::Chain { kind } => chain::run(kind).await,
        command => run_with_watchdog(command, settings.watchdog_secs).await,
    };
    arm_teardown_sentinel();
    result
}

/// Délai accordé au teardown post-commande (flush télémétrie + drop du
/// runtime) avant que la sentinelle ne considère le process comme coincé.
const TEARDOWN_SENTINEL_SECS: u64 = 60;

/// Arme un thread détaché qui, si le process est encore vivant
/// [`TEARDOWN_SENTINEL_SECS`] après la fin de la commande, dumpe l'état de
/// tous les threads (`/proc/self/task/*` : comm, état, wchan) sur **stderr**
/// puis `abort()`.
///
/// Le teardown (Drop du `TelemetryGuard` puis du runtime tokio) est la seule
/// phase qui ne peut ni logger sa fin ni être couverte par le watchdog. À la
/// sortie normale le thread, détaché et endormi, meurt avec le process.
fn arm_teardown_sentinel() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(TEARDOWN_SENTINEL_SECS));
        eprintln!(
            "lj-ingest : teardown bloqué > {TEARDOWN_SENTINEL_SECS} s — état des threads avant abort :"
        );
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for task in tasks.flatten() {
                let path = task.path();
                let read = |f: &str| {
                    std::fs::read_to_string(path.join(f))
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                };
                // stat : champ 3 = état (R/S/D/…), juste après le `comm`
                // parenthésé (qui peut contenir espaces et parenthèses → on
                // coupe après la DERNIÈRE parenthèse fermante).
                let stat = read("stat");
                let state = stat
                    .rsplit(')')
                    .next()
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or("?")
                    .to_string();
                eprintln!(
                    "  tid={} comm={} état={} wchan={}",
                    task.file_name().to_string_lossy(),
                    read("comm"),
                    state,
                    read("wchan"),
                );
            }
        }
        std::process::abort();
    });
}

/// Exécute la commande sous un watchdog + breadcrumbs de cycle de vie.
///
/// - **Watchdog** : au-delà de `watchdog_secs`, on logge une erreur et on
///   abandonne avec un `Err` (code non nul) au lieu de rester pendu. Le `&&`
///   du cron casse alors proprement. `0` = désactivé.
/// - **Breadcrumbs** : `info!` début + `info!`/`error!` fin, shippés vers
///   Loki. Un « début » sans « fin » localise un hang.
///
/// Le timer du watchdog vit sur le runtime multi-thread : même si la
/// commande bloque un worker, le timeout se déclenche, `main` rend `Err`, le
/// guard flush les logs et le process sort.
async fn run_with_watchdog(command: Command, watchdog_secs: u64) -> Result<()> {
    let phase = command.label();
    let started = Instant::now();
    tracing::info!(phase, "commande lj-ingest : début");

    let result = if watchdog_secs == 0 {
        dispatch(command).await.map(Some)
    } else {
        match tokio::time::timeout(Duration::from_secs(watchdog_secs), dispatch(command)).await {
            Ok(inner) => inner.map(Some),
            Err(_) => Ok(None),
        }
    };

    let elapsed_s = started.elapsed().as_secs();
    match result {
        Ok(Some(())) => {
            tracing::info!(phase, elapsed_s, "commande lj-ingest : succès");
            Ok(())
        }
        Ok(None) => {
            tracing::error!(
                phase,
                elapsed_s,
                watchdog_secs,
                "commande lj-ingest : watchdog — dépassement de délai, abandon"
            );
            Err(anyhow!(
                "watchdog: la commande `{phase}` a dépassé {watchdog_secs} s"
            ))
        }
        Err(e) => {
            // `{e:#}` et non `{e}` : l'alternate déplie la chaîne anyhow sur une ligne.
            tracing::error!(phase, elapsed_s, error = %format!("{e:#}"), "commande lj-ingest : échec");
            Err(e)
        }
    }
}

/// Hook de panique : logge la panique en `error!` (donc shippée vers Loki via le
/// bridge) avant de déléguer au hook par défaut (backtrace stderr). Le binaire
/// `lj-ingest` déroule (`panic=unwind`, seul le wasm est `abort`), donc le `Drop`
/// du guard flush la pile de logs — y compris cet `error!` — pendant l'unwind.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "panic lj-ingest");
        default(info);
    }));
}

/// Construit les `InitOpts` (niveau + format + creds OTLP via `Settings`)
/// et installe la telemetrie partagee. `service.name = librejustice-ingest`.
fn init_telemetry(log_level: &str, settings: &Settings) -> Result<TelemetryGuard> {
    let level = logging::resolve_level(Some(log_level));
    // `rustls_platform_verifier` loggue en ERROR chaque certificat invalide : le
    // recensement des profils d'acheteurs sonde des milliers de sites publics dont
    // beaucoup ont un certificat qui ne couvre pas leur propre nom. Le niveau global
    // est répété dans la chaîne de directives : la directive par défaut du builder ne
    // s'applique que si la chaîne est vide.
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .parse_lossy(format!("{level},rustls_platform_verifier=off"));

    let otlp = match (
        settings.grafana_otlp_endpoint.clone(),
        settings.grafana_otlp_user.clone(),
        settings.grafana_cloud_api_key.clone(),
    ) {
        (Some(endpoint), Some(user), Some(api_key)) => Some(OtlpCreds {
            endpoint,
            user,
            api_key,
        }),
        _ => None,
    };

    lj_telemetry::init(InitOpts {
        filter,
        json: logging::json_format(),
        service_name: settings.otel_service_name.clone(),
        deployment_environment: settings.deployment_environment.clone(),
        host_name: settings.host_name.clone(),
        container_name: settings.container_name.clone(),
        // Ingest (cron, bas volume) : on ship INFO+ pour les breadcrumbs de phase.
        otlp_log_filter: Targets::new().with_default(LevelFilter::INFO),
        otlp,
    })
}

async fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Chain { .. } => unreachable!("Command::Chain est déroulée par main"),
        Command::Sync => cmd_sync().await,
        Command::JudilibreRanges => cmd_judilibre_ranges().await,
        Command::Analyze => cmd_analyze(),
        Command::Migrate => cmd_migrate().await,
        Command::Renvois => renvois::run().await,
        Command::AliasCandidats { revue } => alias_candidats::run(revue).await,
        Command::UsageTerms {
            chunks,
            min_citations,
            workers,
        } => usage_terms::run(chunks, min_citations, workers).await,
        Command::Ingest {
            with_embeddings,
            mode,
        } => cmd_ingest(with_embeddings, mode).await,
        Command::Refetch {
            ids,
            with_embeddings,
        } => cmd_refetch(ids, with_embeddings).await,
        Command::ReextractFields {
            dry_run,
            full,
            jurisdiction_type,
            field,
            citing_ref_uid,
            text_query,
            workers,
        } => {
            cmd_reextract_fields(
                dry_run,
                full,
                jurisdiction_type,
                field,
                citing_ref_uid,
                text_query,
                workers,
            )
            .await
        }
        Command::ResolveSeats { dry_run } => pipeline::resolve_seats(dry_run).await,
        Command::EmbedMissing { limit } => pipeline::embed_missing(limit).await,
        Command::PurgeReverses => cmd_purge_reverses().await,
        Command::GenerateSummaries {
            limit,
            concurrency,
            refresh_stale,
        } => summary_pipeline_generate(limit, concurrency, refresh_stale).await,
        Command::ResyncLegalArrays => citations_maintenance::resync_arrays().await,
        Command::Precompute => pipeline::precompute_all().await,
        Command::ResyncJurisdictionCounts => pipeline::resync_jurisdiction_counts().await,
        Command::ResyncCorpusCounts => pipeline::resync_corpus_counts().await,
        Command::ResyncChambers => pipeline::resync_chambers().await,
        Command::RekeyCnb { apply } => pipeline::rekey_cnb(apply).await,
        Command::CnbHistory { apply } => pipeline::replay_cnb_history(apply).await,
        Command::PartiesReconcile { workers } => pipeline::reconcile_parties(workers).await,
        Command::DeriveAffiliations { apply } => pipeline::derive_affiliations(apply).await,
        Command::Reconcile => pipeline::reconcile_all().await,
        Command::Sitemap { dry_run } => cmd_sitemap(dry_run).await,
        Command::Indexnow {
            since_hours,
            max_urls,
        } => cmd_indexnow(since_hours, max_urls).await,
        Command::Legi { path } => pipeline::ingest_legi(&path).await,
        Command::SyncLegi => pipeline::sync_legi().await,
        Command::BackfillLinks { paths } => pipeline::backfill_links(&paths).await,
        Command::BackfillToc { paths } => pipeline::backfill_toc(&paths).await,
        Command::BackfillTextes { paths } => pipeline::backfill_textes(&paths).await,
        Command::Kali { path } => pipeline::ingest_kali(&path).await,
        Command::SyncKali => pipeline::sync_kali().await,
        Command::RequalifierKali { lot } => pipeline::requalifier_kali(lot).await,
        Command::Annonces => pipeline::annonces().await,
        Command::AnnoncesCouverture { limite } => pipeline::annonces_couverture(limite).await,
        Command::AnnoncesBackfill { depuis, jusqu } => {
            let date = |s: &str| {
                s.parse::<jiff::civil::Date>()
                    .map_err(|e| anyhow!("date {s} illisible: {e}"))
            };
            let depuis = depuis.as_deref().map(date).transpose()?;
            pipeline::annonces_backfill(depuis, date(&jusqu)?).await
        }
        Command::AnnoncesEnrichir {
            limite,
            budget_min,
            sans_acheteur,
        } => pipeline::annonces_enrichir(limite, budget_min, sans_acheteur).await,
        Command::AnnoncesDiscover => pipeline::annonces_discover().await,
        Command::AnnoncesRegulariser { formes } => pipeline::annonces_regulariser(formes).await,
        Command::AnnoncesResoudre { applique, limite } => {
            pipeline::annonces_resoudre(limite, applique).await
        }
        Command::AnnoncesJumeler { applique } => pipeline::annonces_jumeler(applique).await,
        Command::Bofip { path } => pipeline::ingest_bofip(&path).await,
        Command::SyncBofip => pipeline::sync_bofip().await,
        Command::SyncCirculaires => pipeline::sync_circulaires().await,
        Command::SyncCirculairesBodies => pipeline::sync_circulaires_bodies().await,
        Command::BackfillTreatyBodies => pipeline::backfill_treaty_bodies().await,
        Command::SyncAriane {
            years,
            census_only,
            limit,
        } => pipeline::sync_ariane(years, census_only, limit).await,
        Command::SyncCeList => pipeline::sync_ce_list().await,
        Command::SyncAdde => pipeline::sync_adde().await,
        Command::SeedArticleCommentaires => pipeline::seed_article_commentaires().await,
        Command::ExtractTextRefs => pipeline::extract_text_refs().await,
        Command::Jorf { paths } => pipeline::ingest_jorf(&paths).await,
        Command::SyncJorf => pipeline::sync_jorf().await,
        Command::AssignSlugs => pipeline::assign_slugs().await,
        Command::BackfillTextRoles => pipeline::backfill_text_roles().await,
        Command::MoveChronicle { vehicle, exclude } => {
            pipeline::move_chronicle(&vehicle, &exclude).await
        }
        Command::RenameTexts { dataset } => pipeline::rename_texts(&dataset).await,
        Command::RekeyArticleKeys => pipeline::rekey_article_keys().await,
        Command::RekeyIdentityKeys => pipeline::rekey_identity_keys().await,
        Command::DestarArticleKeys { applique } => pipeline::destar_article_keys(applique).await,
        Command::LoadLegalCorpus { only } => pipeline::load_legal_corpus(only.as_deref()).await,
        Command::IngestEuCatalog { limit } => pipeline::ingest_eu_catalog(limit).await,
        Command::SyncEuCatalog {
            depuis,
            jusqu_a,
            limit,
        } => {
            let aujourdhui = chrono::Utc::now().date_naive();
            let depuis = depuis.unwrap_or(aujourdhui - chrono::Duration::days(90));
            pipeline::sync_eu_catalog(depuis, jusqu_a.unwrap_or(aujourdhui), limit).await
        }
        Command::SyncEuJorf { limit } => pipeline::sync_eu_jorf(limit).await,
        Command::MergeEuSwappedUids { apply } => pipeline::merge_eu_swapped_uids(apply).await,
        Command::SyncEuLaw {
            limit,
            only,
            force,
            full,
        } => pipeline::sync_eu_law(limit, only.as_deref(), force, full).await,
        Command::IngestEuRproc { dry_run } => pipeline::ingest_eu_rproc(dry_run).await,
        Command::StampFreshness => pipeline::stamp_freshness().await,
        Command::Invariants => invariants::run().await,
        Command::ReadingRanks => pipeline::reading_ranks().await,
        Command::RelabelSources => pipeline::relabel_sources().await,
        Command::CanonicalizeSourceLabels => pipeline::canonicalize_source_labels().await,
        Command::IngestDila { fond } => pipeline::ingest_dila(fond).await,
        Command::SyncDila { fond } => pipeline::sync_dila(fond).await,
        Command::BackfillEcli => pipeline::backfill_ecli().await,
        Command::BuildSuggest => pipeline::build_suggest().await,
        Command::BackfillCanonicalRef { force } => pipeline::backfill_canonical_ref(force).await,
        Command::MergeCrossSourceDuplicates => pipeline::merge_cross_source_duplicates().await,
        Command::ReingestStaleOpendata => cmd_reingest_stale_opendata().await,
        Command::AnalyzeFalseMerges => pipeline::analyze_false_merges().await,
        Command::ResplitFalseMerges {
            execute,
            audit_sample,
            limit,
        } => cmd_resplit_false_merges(execute, audit_sample, limit).await,
        Command::UnmergeSameSourceDila { execute, limit } => {
            pipeline::unmerge_same_source_dila(execute, limit).await
        }
        Command::UnmergeSameSource { execute, limit } => {
            pipeline::unmerge_same_source(execute, limit).await
        }
        Command::IngestCedh { from_year } => pipeline::ingest_cedh(from_year).await,
        Command::CacheCedh { from_year } => pipeline::cache_cedh(from_year).await,
        Command::IngestCjue => pipeline::ingest_cjue().await,
        Command::SyncCedh => pipeline::sync_cedh().await,
        Command::SyncCjue => pipeline::sync_cjue().await,
        Command::IngestCnda { mode, only } => pipeline::ingest_cnda(mode, only).await,
        Command::SyncCnda => pipeline::sync_cnda().await,
        Command::JuricafProbe { dir } => pipeline::juricaf_probe(dir).await,
        Command::ScopeBackfillCompteurs {
            collectivite,
            text_uid,
            apply,
        } => pipeline::scope_backfill_compteurs(collectivite, text_uid, apply).await,
        Command::ScopeBackfillAlsace { apply } => pipeline::scope_backfill_alsace(apply).await,
        Command::BackfillCndaSolution { apply } => pipeline::backfill_cnda_solution(apply).await,
        Command::Registries {
            source,
            with_history,
        } => pipeline::load_registries(source, with_history).await,
        Command::PartiesRelink { full } => pipeline::relink_parties(full).await,
        Command::LoadQuantumGold { dir } => pipeline::load_quantum_gold(&dir).await,
        Command::Db(db_command) => cmd_db(db_command).await,
    }
}

async fn pool_with_migrations(settings: &Settings) -> Result<deadpool_postgres::Pool> {
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    Ok(pool)
}

/// Pool deadpool sans migrations (pour les commandes de maintenance DB qui ne
/// veulent pas toucher au schéma). `pool_max` : 4 suffit partout sauf
/// `db vacuum-full` (une connexion par table en parallèle + la connexion
/// partagée de `cmd_db`).
async fn pool_only(settings: &Settings, pool_max: usize) -> Result<deadpool_postgres::Pool> {
    lj_store::db::build_pool(&settings.db_url, pool_max).map_err(|e| anyhow!("build_pool: {e}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// sync
// ─────────────────────────────────────────────────────────────────────────────

async fn cmd_sync() -> Result<()> {
    let settings = Settings::from_env()?;
    let cache_dir = settings.cache_dir();

    // Non-fatal : une panne transitoire opendata ne doit pas abandonner le
    // reste de la chaîne nocturne. On log et on continue.
    tracing::info!("=== sync opendata_conseil_etat ===");
    if let Err(e) = lj_sources::downloader::sync_opendata(&cache_dir, false) {
        tracing::error!(error = %e, "sync opendata échoué — on poursuit avec Judilibre");
    }

    tracing::info!("=== sync judilibre ===");
    let client = judilibre_client(&settings)?;
    // `cmd_sync` n'expose pas `--from` → `date_start=None` (chaîne vide =
    // défaut par juridiction MONTHLY_START).
    lj_sources::downloader::sync_judilibre(&client, &cache_dir, "")
        .await
        .map_err(|e| anyhow!("sync judilibre: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// judilibre-ranges
// ─────────────────────────────────────────────────────────────────────────────

/// Une plage bornée par bisection : `(date_start, date_end, total)`.
type JudilibreRange = (String, String, i64);

/// Bisection one-shot des plages de bootstrap Judilibre (toutes les
/// juridictions, `max_chunk=50_000`, `date_from="1900-01-01"`).
async fn cmd_judilibre_ranges() -> Result<()> {
    use chrono::{Duration, NaiveDate, Utc};

    let settings = Settings::from_env()?;

    let jurs: Vec<&str> = JUDILIBRE_DEFAULT_JURISDICTIONS.to_vec();
    let max_chunk: i64 = 50_000;
    let date_from = "1900-01-01";
    let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();

    let client = judilibre_client(&settings)?;

    async fn scan_total(
        client: &lj_sources::judilibre::JudilibreClient,
        jur: &str,
        d_start: &str,
        d_end: &str,
    ) -> Result<i64> {
        let params: &[(&str, String)] = &[
            ("jurisdiction", jur.to_string()),
            ("date_type", "creation".to_string()),
            ("date_start", d_start.to_string()),
            ("date_end", d_end.to_string()),
            ("batch_size", "1".to_string()),
        ];
        let page = client
            .scan(params)
            .await
            .map_err(|e| anyhow!("scan: {e}"))?;
        Ok(page.get("total").and_then(|v| v.as_i64()).unwrap_or(0))
    }

    fn mid_date(d_start: &str, d_end: &str) -> Result<String> {
        let a = NaiveDate::parse_from_str(d_start, "%Y-%m-%d")?;
        let b = NaiveDate::parse_from_str(d_end, "%Y-%m-%d")?;
        let days = (b - a).num_days() / 2;
        Ok((a + Duration::days(days)).format("%Y-%m-%d").to_string())
    }

    // Bisection dichotomique itérative (pile explicite). Ordre de sortie
    // gauche-d'abord.
    async fn bisect(
        client: &lj_sources::judilibre::JudilibreClient,
        jur: &str,
        start: &str,
        end: &str,
        max_chunk: i64,
    ) -> Result<Vec<JudilibreRange>> {
        use chrono::Duration;
        let mut ranges: Vec<JudilibreRange> = Vec::new();
        let mut stack: Vec<(String, String)> = vec![(start.to_string(), end.to_string())];
        while let Some((d_start, d_end)) = stack.pop() {
            let total = scan_total(client, jur, &d_start, &d_end).await?;
            tracing::info!(%jur, %d_start, %d_end, total, "judilibre_range_scan");
            if total == 0 {
                continue;
            }
            if total <= max_chunk || d_start == d_end {
                ranges.push((d_start, d_end, total));
                continue;
            }
            let mid = mid_date(&d_start, &d_end)?;
            let next_day = (NaiveDate::parse_from_str(&mid, "%Y-%m-%d")? + Duration::days(1))
                .format("%Y-%m-%d")
                .to_string();
            // Empile la moitié droite d'abord pour traiter la gauche en premier.
            stack.push((next_day, d_end));
            stack.push((d_start, mid));
        }
        Ok(ranges)
    }

    let mut all_ranges: Vec<(String, Vec<JudilibreRange>)> = Vec::new();
    for jur in &jurs {
        tracing::info!(%jur, "--- bisect ---");
        let ranges = bisect(&client, jur, date_from, &today, max_chunk).await?;
        all_ranges.push((jur.to_string(), ranges));
    }

    println!("\n# BOOTSTRAP_RANGES");
    println!("BOOTSTRAP_RANGES: dict[str, list[tuple[str, str]]] = {{");
    for (jur, ranges) in &all_ranges {
        println!("    \"{jur}\": [");
        for (d_start, d_end, total) in ranges {
            println!("        (\"{d_start}\", \"{d_end}\"),  # {total}");
        }
        println!("    ],");
    }
    println!("}}");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// analyze
// ─────────────────────────────────────────────────────────────────────────────

/// Passe d'analyse du corpus local (défauts : `extra_dir=[]`, `sample=20`,
/// `seed=0`, sortie stdout).
fn cmd_analyze() -> Result<()> {
    let settings = Settings::from_env()?;
    let report = analyze::run(&settings.paths().opendata(), &[], 20, 0)?;
    let payload = serde_json::to_string_pretty(&report)?;
    println!("{payload}");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// migrate
// ─────────────────────────────────────────────────────────────────────────────

async fn cmd_migrate() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = pool_only(&settings, 4).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let applied = lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    if applied.is_empty() {
        println!("Base déjà à jour.");
    } else {
        println!("Migrations appliquées : {applied:?}");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ingest
// ─────────────────────────────────────────────────────────────────────────────

/// Ingère les deux sources connues.
///
/// On itère les sources dans l'ordre `opendata_conseil_etat` puis
/// `judilibre`, chacune sur son sous-dossier de cache. `pipeline::ingest_*`
/// gère pool, migrations et embedder en interne. `mode` propage la trappe
/// ALL (re-traitement total) jusqu'au triage.
async fn cmd_ingest(with_embeddings: bool, mode: pipeline::IngestMode) -> Result<()> {
    let settings = Settings::from_env()?;
    pipeline::ingest_opendata(&settings.paths().opendata(), with_embeddings, mode).await?;
    pipeline::ingest_judilibre(&settings.paths().judilibre(), with_embeddings, mode).await?;
    Ok(())
}

/// Re-ingest ciblé des décisions opendata à `full_text` figé sur une
/// provenance de rang inférieur. Scanne le dossier opendata, mais ne
/// ré-embed que les cibles (CAA/CE) ; local strict.
async fn cmd_reingest_stale_opendata() -> Result<()> {
    let settings = Settings::from_env()?;
    pipeline::reingest_stale_opendata(&settings.paths().opendata()).await
}

// ─────────────────────────────────────────────────────────────────────────────
// reextract-fields
// ─────────────────────────────────────────────────────────────────────────────

/// Ré-extrait les champs structurés depuis les payloads stockés.
/// `--field` (vide → tous les champs ré-extractibles) ; `--juridiction-type`
/// (vide → keyset par version) cible un sous-ensemble quelle que soit la
/// version ; `--citing-ref-uid` cible les décisions citant un texte donné ;
/// `--full` = passe intégrale/relink hebdo (tout le fonds < 1000).
async fn cmd_reextract_fields(
    dry_run: bool,
    full: bool,
    jurisdiction_type: Vec<String>,
    field: Vec<String>,
    citing_ref_uid: Option<String>,
    text_query: Option<String>,
    workers: Option<usize>,
) -> Result<()> {
    let jts = (!jurisdiction_type.is_empty()).then_some(jurisdiction_type);
    let fields = (!field.is_empty()).then_some(field);
    pipeline::reextract_fields(
        fields.as_deref(),
        dry_run,
        full,
        jts.as_deref(),
        citing_ref_uid.as_deref(),
        text_query.as_deref(),
        workers,
    )
    .await
}

async fn cmd_refetch(ids: Vec<String>, with_embeddings: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let client = judilibre_client(&settings)?;
    pipeline::refetch_judilibre(&client, &ids, with_embeddings).await
}

/// Réparation des faux merges judilibre. Dry-run (défaut) : read-only,
/// aucun client requis. `--execute` : construit le client Judilibre (re-fetch
/// des provenances divergentes) puis re-split + re-embed ciblé.
async fn cmd_resplit_false_merges(
    execute: bool,
    audit_sample: usize,
    limit: Option<usize>,
) -> Result<()> {
    let dry_run = !execute;
    let settings = Settings::from_env()?;
    let client = if dry_run {
        None
    } else {
        Some(judilibre_client(&settings)?)
    };
    pipeline::resplit_false_merges(client.as_ref(), dry_run, audit_sample, limit).await
}

// ─────────────────────────────────────────────────────────────────────────────
// purge-reverses
// ─────────────────────────────────────────────────────────────────────────────

/// Hard-delete des décisions retirées (CSV `documents_reverses`).
async fn cmd_purge_reverses() -> Result<()> {
    let settings = Settings::from_env()?;
    let csv_dir = settings.paths().opendata().join("documents_reverses");
    if !csv_dir.exists() {
        return Err(anyhow!("Dossier introuvable : {}", csv_dir.display()));
    }
    let pool = pool_with_migrations(&settings).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let summaries = reverses::purge_all(&csv_dir, &repo).await?;
    for s in &summaries {
        println!(
            "{} : {} lignes, {} décisions marquées supprimées, {} absentes ou déjà purgées.",
            s.file, s.processed, s.newly_deleted, s.already_deleted_or_missing
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// generate-summaries
// ─────────────────────────────────────────────────────────────────────────────

/// Backfill des résumés Mistral. `--concurrency` borne les appels Mistral
/// concurrents (défaut dérivé du nombre de clés).
async fn summary_pipeline_generate(
    limit: Option<usize>,
    concurrency: Option<usize>,
    refresh_stale: bool,
) -> Result<()> {
    summary::backfill_summaries(
        lj_core::summary::SUMMARY_PROMPT_VERSION,
        concurrency,
        1000,
        limit.map(|l| l as i64),
        false,
        refresh_stale,
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// sitemap
// ─────────────────────────────────────────────────────────────────────────────

/// Fenêtre d'archive des fiches de consultation publiées au sitemap.
const ANNONCES_SITEMAP_ARCHIVE_JOURS: i32 = 180;

/// Source sitemap matérialisée : décisions `(public_id, lastmod)` et articles de
/// référentiel `(slug, num, lastmod)` pré-chargés depuis le repo (les méthodes
/// repo sont async, le trait `SitemapSource` est sync).
struct LoadedSitemapSource {
    decisions: Vec<(String, sitemap::SourceLastmod)>,
    referential: Vec<(String, String, sitemap::SourceLastmod)>,
    entities: Vec<(String, String, sitemap::SourceLastmod)>,
    texts: Vec<(String, sitemap::SourceLastmod)>,
    jurisdiction_hubs: sitemap::JurisdictionHubPages,
    norm_hubs: sitemap::NormHubPages,
    annonces: (Vec<(String, sitemap::SourceLastmod)>, Vec<String>),
}

impl sitemap::SitemapSource for LoadedSitemapSource {
    fn iter_decisions_for_sitemap(&self) -> Result<Vec<(String, sitemap::SourceLastmod)>> {
        Ok(self.decisions.clone())
    }
    fn iter_referential_for_sitemap(
        &self,
    ) -> Result<Vec<(String, String, sitemap::SourceLastmod)>> {
        Ok(self.referential.clone())
    }
    fn iter_entities_for_sitemap(&self) -> Result<Vec<(String, String, sitemap::SourceLastmod)>> {
        Ok(self.entities.clone())
    }
    fn iter_texts_for_sitemap(&self) -> Result<Vec<(String, sitemap::SourceLastmod)>> {
        Ok(self.texts.clone())
    }
    fn iter_jurisdiction_hubs_for_sitemap(&self) -> Result<sitemap::JurisdictionHubPages> {
        Ok(self.jurisdiction_hubs.clone())
    }
    fn iter_norm_hubs_for_sitemap(&self) -> Result<sitemap::NormHubPages> {
        Ok(self.norm_hubs.clone())
    }
    fn iter_annonces_for_sitemap(&self) -> Result<sitemap::AnnoncesSitemap> {
        Ok(self.annonces.clone())
    }
}

/// Génère les sitemaps et les publie en base.
///
/// Charge les décisions via le repo, construit l'ensemble en mémoire, puis
/// (sauf `--dry-run`) remplace la table `sitemaps` dans une transaction
/// unique — `lj-server` sert ces lignes sous `/sitemap.xml` +
/// `/sitemap-{n}.xml.gz`.
async fn cmd_sitemap(dry_run: bool) -> Result<()> {
    let settings = Settings::from_env()?;

    tracing::info!("=== build sitemaps ===");
    let hub_page_size = i64::from(lj_dtos::HUB_PAGE_SIZE);
    let pool = pool_with_migrations(&settings).await?;
    let (decisions, referential, entities, texts, jurisdiction_hubs, norm_hubs, annonces) = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        let decisions = repo
            .iter_decisions_for_sitemap(sitemap::MAX_URLS_PER_SITEMAP as i64)
            .await
            .map_err(|e| anyhow!("iter_decisions_for_sitemap: {e}"))?;
        let referential = repo
            .iter_referential_for_sitemap()
            .await
            .map_err(|e| anyhow!("iter_referential_for_sitemap: {e}"))?;
        let entities = repo
            .iter_entities_for_sitemap()
            .await
            .map_err(|e| anyhow!("iter_entities_for_sitemap: {e}"))?;
        let texts = repo
            .iter_texts_for_sitemap()
            .await
            .map_err(|e| anyhow!("iter_texts_for_sitemap: {e}"))?;
        let jurisdiction_hubs = repo
            .iter_jurisdiction_hubs_for_sitemap(hub_page_size)
            .await
            .map_err(|e| anyhow!("iter_jurisdiction_hubs_for_sitemap: {e}"))?;
        let norm_hubs = repo
            .iter_norm_hubs_for_sitemap(hub_page_size)
            .await
            .map_err(|e| anyhow!("iter_norm_hubs_for_sitemap: {e}"))?;
        // Annonces : la fenêtre ouverte + l'archive récente pour les fiches, et
        // les départements qui ont au moins une consultation ouverte pour les
        // hubs — au-delà, la page répond 404.
        let fond = lj_dtos::ANNONCE_FOND_MARCHE;
        let fiches = repo
            .annonces_sitemap(fond, ANNONCES_SITEMAP_ARCHIVE_JOURS)
            .await
            .map_err(|e| anyhow!("annonces_sitemap: {e}"))?
            .into_iter()
            .map(|(uid, date)| (uid, Some(date)))
            .collect();
        let departements = repo
            .annonces_facette(
                &lj_store::repository::AnnonceFiltre::nouveau(fond),
                lj_store::repository::AnnonceAxe::Departement,
            )
            .await
            .map_err(|e| anyhow!("annonces_facette(departement): {e}"))?
            .into_iter()
            .map(|(code, _, _)| code)
            .collect();
        (
            decisions,
            referential,
            entities,
            texts,
            jurisdiction_hubs,
            norm_hubs,
            (fiches, departements),
        )
    };
    let source = LoadedSitemapSource {
        decisions,
        referential,
        entities,
        texts,
        jurisdiction_hubs,
        norm_hubs,
        annonces,
    };
    let files = sitemap::build_sitemaps(&source, chrono::Utc::now().date_naive())?;

    let sub_count = files.len() - 1; // index exclu
    tracing::info!(
        sub_count,
        index = sitemap::SITEMAP_INDEX_NAME,
        "sitemaps générés"
    );

    if dry_run {
        tracing::info!("--dry-run : pas d'écriture en base.");
        return Ok(());
    }

    // Régénération complète atomique : DELETE + INSERT en une transaction.
    let rows: Vec<lj_store::repository::SitemapRow> = files
        .into_iter()
        .map(|f| lj_store::repository::SitemapRow {
            filename: f.filename,
            content_type: f.content_type.to_string(),
            body: f.body,
            lastmod: f.lastmod,
        })
        .collect();
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    conn.batch_execute("BEGIN").await?;
    match repo.replace_sitemaps(&rows).await {
        Ok(()) => conn.batch_execute("COMMIT").await?,
        Err(e) => {
            let _ = conn.batch_execute("ROLLBACK").await;
            return Err(anyhow!("replace_sitemaps: {e}"));
        }
    }
    tracing::info!(count = rows.len(), "sitemaps publiés en base");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// indexnow
// ─────────────────────────────────────────────────────────────────────────────

/// Pousse vers IndexNow les décisions récemment mises à jour.
///
/// Valide la clé (`LIBREJUSTICE_INDEXNOW_KEY`), collecte les `public_id`
/// dont `updated_at >= now - since_hours` et les consultations parues dans
/// la même fenêtre, puis soumet via `indexnow::submit` sous le cap dur
/// `max_urls` (troncature signalée).
async fn cmd_indexnow(since_hours: f64, max_urls: usize) -> Result<()> {
    use chrono::{Duration, Utc};

    let settings = Settings::from_env()?;
    let Some(key) = settings.indexnow_key.as_deref() else {
        return Err(anyhow!(
            "LIBREJUSTICE_INDEXNOW_KEY non posé — clé IndexNow requise."
        ));
    };
    let since = Utc::now() - Duration::milliseconds((since_hours * 3_600_000.0) as i64);

    let pool = pool_with_migrations(&settings).await?;
    let (urls, truncated) = {
        let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
        // `iter_public_ids_updated_since` pagine en keyset sur `id` mais filtre
        // `updated_at` (aucun index dédié, par choix). Sur un gros backlog un batch
        // balaie d'immenses plages d'`id` et dépasse le `statement_timeout` 30 s par
        // défaut → `query_canceled`. On le lève sur CETTE connexion : la commande
        // `indexnow` est un process CLI one-shot.
        conn.batch_execute("SET statement_timeout = 0").await?;
        let repo = lj_store::repository::DecisionRepository::new(&conn);
        let decisions = repo
            .iter_public_ids_updated_since(since, 10_000)
            .await
            .map_err(|e| anyhow!("iter_public_ids_updated_since: {e}"))?;
        let annonces = repo
            .annonces_vues_depuis(lj_dtos::ANNONCE_FOND_MARCHE, since, max_urls as i64)
            .await
            .map_err(|e| anyhow!("annonces_vues_depuis: {e}"))?;
        // Les consultations passent devant : leur fenêtre utile est l'échéance, à
        // trois ou quatre semaines, alors qu'une décision reste indexable des
        // années. Sous le cap, c'est donc la queue des décisions qui cède.
        let mut all: Vec<String> = annonces.iter().map(|u| indexnow::marche_url(u)).collect();
        all.extend(decisions.iter().map(|p| indexnow::decision_url(p)));
        let truncated = all.len() > max_urls;
        all.truncate(max_urls);
        (all, truncated)
    };

    if truncated {
        tracing::warn!(
            max_urls,
            since_hours,
            "plus d'URLs (re)publiées que la borne — soumission tronquée. \
             Relancer avec --max-urls plus haut, ou par fenêtres --since-hours plus courtes."
        );
    }
    let submitted = indexnow::submit(&urls, key).await?;
    tracing::info!(submitted, "IndexNow : URL(s) soumises.");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// db
// ─────────────────────────────────────────────────────────────────────────────

/// `statement_timeout` des commandes de maintenance lourdes (ANALYZE,
/// VACUUM FULL, REINDEX CONCURRENTLY, prewarm). `build_pool` arme 30 s sur
/// chaque connexion pour borner les requêtes API/search ; ces ops-là durent
/// légitimement de plusieurs minutes (REINDEX) à ~1 h (VACUUM FULL). Un
/// `SET` de session écrase l'option de démarrage `options`. Backstop fini
/// volontaire (pas `0`) : une op qui dérape finit par être tuée.
const MAINTENANCE_STATEMENT_TIMEOUT: &str = "2h";

/// Maintenance DB. Toutes ouvrent une connexion en `autocommit` et
/// exécutent du SQL de maintenance — pas de migrations.
async fn cmd_db(command: DbCommand) -> Result<()> {
    let settings = Settings::from_env()?;
    // 7 = shared connection below + the 5 parallel tables of `db vacuum-full`
    // and `db reindex-search` + the blocker sweep the latter holds throughout.
    let pool = pool_only(&settings, 7).await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Relève le statement_timeout pour les ops lourdes. `gc-mcp` (DELETE borné)
    // garde le garde-fou 30 s.
    if !matches!(command, DbCommand::GcMcp) {
        conn.batch_execute(&format!(
            "SET statement_timeout = '{MAINTENANCE_STATEMENT_TIMEOUT}'"
        ))
        .await?;
    }
    match command {
        DbCommand::Analyze => db_analyze(&conn).await,
        DbCommand::ReindexSearch => db_reindex_search(&pool).await,
        DbCommand::VacuumFull => db_vacuum_full(&pool).await,
        DbCommand::GcMcp => db_gc_mcp(&conn).await,
        DbCommand::Prewarm => db_prewarm(&conn).await,
        DbCommand::PurgeOrphanTombstones { apply } => {
            db_purge_orphan_tombstones(&conn, apply).await
        }
    }
}

/// Purge les tombstones orphelins (retraits totaux vidés sans provenance active,
/// cf. `DecisionRepository::purge_orphan_tombstones`). Ceinture-bretelle :
/// dry-run par défaut (compte + échantillon, aucune suppression) ; `--apply`
/// requis pour supprimer ; re-vérifie l'absence d'orphelin résiduel après coup.
async fn db_purge_orphan_tombstones(conn: &lj_store::db::Connection, apply: bool) -> Result<()> {
    let repo = lj_store::repository::DecisionRepository::new(conn);
    let n = repo.count_orphan_tombstones().await?;
    println!("Tombstones orphelins (retirés, vidés, sans provenance active) : {n}");
    let sample = repo.sample_orphan_tombstones(5).await?;
    if !sample.is_empty() {
        println!("Échantillon source_uid : {}", sample.join(", "));
    }
    if n == 0 {
        println!("Rien à purger.");
        return Ok(());
    }
    if !apply {
        println!("Dry-run : aucune suppression. Relancer avec --apply pour purger.");
        return Ok(());
    }
    let deleted = repo.purge_orphan_tombstones().await?;
    println!("{deleted} décisions supprimées (cascade provenances/chunks/full_text/citations).");
    let remaining = repo.count_orphan_tombstones().await?;
    if remaining != 0 {
        return Err(anyhow!(
            "purge incomplète : {remaining} orphelins résiduels"
        ));
    }
    println!("Vérifié : 0 orphelin résiduel.");
    Ok(())
}

async fn db_analyze(conn: &lj_store::db::Connection) -> Result<()> {
    // Sans transaction ouverte, chaque `execute` tokio-postgres s'auto-commit.
    // ANALYZE ne tourne pas dans un bloc.
    conn.execute("ANALYZE decisions", &[]).await?;
    conn.execute("ANALYZE decision_chunks", &[]).await?;
    conn.execute("ANALYZE decision_full_text", &[]).await?;
    // Stats fraîches pour l'overlay, les backlinks (GIN `lj_cit_terms`) et le
    // service `/texte/`.
    conn.execute("ANALYZE legal_citation", &[]).await?;
    conn.execute("ANALYZE legal_text", &[]).await?;
    conn.execute("ANALYZE legal_article", &[]).await?;
    // Le pot Annonces grossit de quelques milliers de lignes par jour sur 1,2 M :
    // l'autoanalyze, qui attend 10 % de la table, ne déclenche jamais.
    conn.execute("ANALYZE annonce", &[]).await?;
    conn.execute("ANALYZE annonce_source", &[]).await?;
    conn.execute("ANALYZE annonce_entity", &[]).await?;
    println!("ANALYZE terminé.");
    Ok(())
}

/// Supprime les index **invalides** d'une table. Un `REINDEX CONCURRENTLY`
/// interrompu (deadlock, kill) laisse derrière lui un `<index>_ccnew` marqué
/// `indisvalid = false` : inutilisable en lecture, mais `indisready = true`
/// donc **maintenu à chaque écriture** et occupant sa taille pleine.
/// Appelé avant ET après le reindex.
async fn drop_invalid_indexes(conn: &lj_store::db::Connection, table: &str) -> Result<()> {
    let rows = conn
        .query(
            "SELECT c.relname FROM pg_class c \
             JOIN pg_index i ON i.indexrelid = c.oid \
             JOIN pg_class t ON t.oid = i.indrelid \
             WHERE t.relname = $1 AND NOT i.indisvalid",
            &[&table],
        )
        .await?;
    for row in &rows {
        let index: String = row.get(0);
        println!("DROP INDEX CONCURRENTLY {index} (invalide, résidu d'un reindex interrompu) …");
        conn.batch_execute(&format!("DROP INDEX CONCURRENTLY {index}"))
            .await?;
    }
    Ok(())
}

/// Pooled connections carry `statement_timeout=30s`: an older open transaction
/// is a session out of pool, typically a `psql` whose client died unnoticed.
const MAINTENANCE_XACT_GRACE: &str = "5 minutes";

/// A blocker arriving mid-reindex costs as much as one already there.
const MAINTENANCE_SWEEP: std::time::Duration = std::time::Duration::from_secs(60);

/// Cancels transactions older than [`MAINTENANCE_XACT_GRACE`], ours excepted.
/// `REINDEX` and `DROP INDEX CONCURRENTLY` wait on every snapshot older than
/// their own, any table: 30/08/2026, an orphan `psql` burned the 2 h of all five.
async fn cancel_blocking_transactions(conn: &lj_store::db::Connection) -> Result<()> {
    let rows = conn
        .query(
            "SELECT pid, application_name, \
                    extract(epoch FROM now() - xact_start)::bigint \
               FROM pg_stat_activity \
              WHERE backend_type = 'client backend' \
                AND pid <> pg_backend_pid() \
                AND application_name IS DISTINCT FROM current_setting('application_name') \
                AND xact_start < now() - $1::text::interval",
            &[&MAINTENANCE_XACT_GRACE],
        )
        .await?;
    for row in &rows {
        let pid: i32 = row.get(0);
        let app: String = row.get(1);
        let age: i64 = row.get(2);
        conn.execute("SELECT pg_cancel_backend($1)", &[&pid])
            .await?;
        println!("maintenance : transaction de {age} s annulée (pid {pid}, application {app}).");
    }
    Ok(())
}

/// Maintenance hebdomadaire des tables de recherche :
/// `REINDEX TABLE CONCURRENTLY` puis `VACUUM`, table par table, en
/// parallèle. Zéro indisponibilité.
///
/// `REINDEX TABLE` et non `REINDEX INDEX` sur les seuls BM25 : les btrees
/// bloatent tout autant. Le rebuild purge aussi les tombstones tantivy des
/// index BM25, que la politique de merge par couches ne reprend jamais.
///
/// Le `VACUUM` simple qui suit rend l'espace mort du heap réutilisable sur
/// place et entretient la visibility map ; il ne rend rien à l'OS.
///
/// `CONCURRENTLY` ne peut pas tourner dans un bloc transactionnel →
/// `batch_execute` (protocole simple, hors transaction implicite des
/// requêtes paramétrées). Un échec ne coûte que sa semaine : les index en
/// place restent valides et servent, l'orphelin est nettoyé, et la chaîne
/// remonte `chain_failed`. Pas de retry.
async fn db_reindex_search(pool: &deadpool_postgres::Pool) -> Result<()> {
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    cancel_blocking_transactions(&conn).await?;
    let sweeper = tokio::spawn(async move {
        loop {
            tokio::time::sleep(MAINTENANCE_SWEEP).await;
            if let Err(e) = cancel_blocking_transactions(&conn).await {
                eprintln!("maintenance : filet anti-blocage en échec : {e}");
            }
        }
    });

    let mut handles = Vec::new();
    for table in SEARCH_TABLES {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
            conn.batch_execute(&format!(
                "SET statement_timeout = '{MAINTENANCE_STATEMENT_TIMEOUT}'"
            ))
            .await?;
            drop_invalid_indexes(&conn, table).await?;

            println!("REINDEX TABLE CONCURRENTLY {table} … [aucun lock exclusif]");
            let started = std::time::Instant::now();
            let outcome = conn
                .batch_execute(&format!("REINDEX TABLE CONCURRENTLY {table}"))
                .await;
            if let Err(e) = outcome {
                drop_invalid_indexes(&conn, table).await?;
                return Err(anyhow!("REINDEX {table} : {e}"));
            }
            println!(
                "REINDEX {table} terminé en {} s.",
                started.elapsed().as_secs()
            );

            let started = std::time::Instant::now();
            println!("VACUUM {table} …");
            conn.batch_execute(&format!("VACUUM {table}")).await?;
            println!(
                "VACUUM {table} terminé en {} s.",
                started.elapsed().as_secs()
            );
            Ok::<(), anyhow::Error>(())
        }));
    }
    let mut failures = Vec::new();
    for (table, handle) in SEARCH_TABLES.iter().zip(handles) {
        if let Err(e) = handle.await? {
            failures.push(format!("{table}: {e}"));
        }
    }
    sweeper.abort();
    if !failures.is_empty() {
        return Err(anyhow!(
            "reindex-search en échec — {}",
            failures.join(" ; ")
        ));
    }
    Ok(())
}

/// Tables porteuses d'index de recherche, couvertes par la maintenance
/// hebdomadaire `db reindex-search` et par le repli manuel `db vacuum-full`.
const SEARCH_TABLES: [&str; 5] = [
    "decision_chunks",
    "legal_article",
    "legal_text",
    "legal_article_usage",
    "decisions",
];

/// `VACUUM FULL` des tables de recherche : compacte le heap et
/// reconstruit **tous** leurs index — seul moyen de purger les tombstones
/// tantivy des gros segments, que les merges par couches ne reprennent
/// jamais. Les cinq tables passent en **parallèle** (une connexion chacune) :
/// le wall-clock tient à la plus grosse. Lock AccessExclusive par table
/// pendant sa réécriture ; un échec n'interrompt pas les autres (join
/// complet puis erreur agrégée).
///
/// Chaque table enchaîne sur un `VACUUM` simple qui reconstruit sa
/// visibility map, que le `VACUUM FULL` laisse vide.
async fn db_vacuum_full(pool: &deadpool_postgres::Pool) -> Result<()> {
    let maintenance_work_mem = "512MB";
    let mut handles = Vec::new();
    for table in SEARCH_TABLES {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
            // Timeout maintenance + work_mem par connexion (le SET de session
            // de `cmd_db` ne vaut que pour la connexion partagée).
            conn.batch_execute(&format!(
                "SET statement_timeout = '{MAINTENANCE_STATEMENT_TIMEOUT}'; \
                 SET maintenance_work_mem = '{maintenance_work_mem}'"
            ))
            .await?;
            println!("VACUUM FULL {table} (mwm={maintenance_work_mem}) … [lock AccessExclusive]");
            let started = std::time::Instant::now();
            // VACUUM FULL ne peut pas tourner dans un bloc transactionnel.
            conn.batch_execute(&format!("VACUUM FULL {table}")).await?;
            println!(
                "VACUUM FULL {table} terminé en {} s.",
                started.elapsed().as_secs()
            );
            // VACUUM FULL réécrit le heap et laisse la visibility map VIDE. Tant
            // qu'elle n'est pas reconstruite, tout Index Only Scan va chercher chaque
            // ligne dans le heap pour tester sa visibilité — l'« index only » ne l'est
            // plus. L'autovacuum ne rattrape pas : il se déclenche sur un ratio de
            // lignes mortes que ces tables, fraîchement compactées, mettent des
            // semaines à atteindre. Un VACUUM simple (lock ShareUpdateExclusive, ni
            // lecture ni écriture bloquée) la repose.
            let started = std::time::Instant::now();
            println!("VACUUM {table} (visibility map) …");
            conn.batch_execute(&format!("VACUUM {table}")).await?;
            println!(
                "VACUUM {table} terminé en {} s.",
                started.elapsed().as_secs()
            );
            Ok::<(), anyhow::Error>(())
        }));
    }
    let mut failures = Vec::new();
    for (table, handle) in SEARCH_TABLES.iter().zip(handles) {
        if let Err(e) = handle.await? {
            failures.push(format!("{table}: {e}"));
        }
    }
    if !failures.is_empty() {
        return Err(anyhow!("VACUUM FULL en échec — {}", failures.join(" ; ")));
    }
    Ok(())
}

/// Purge des tokens/codes MCP expirés — les trois tables.
///
/// `mcp_refresh_tokens` ne fait que croître, sur des lignes qu'aucun chemin
/// ne peut plus valider (`token_refresh` filtre sur `expires_at > now()`).
async fn db_gc_mcp(conn: &lj_store::db::Connection) -> Result<()> {
    let tokens = conn
        .execute("DELETE FROM mcp_tokens WHERE expires_at < now()", &[])
        .await?;
    let refresh = conn
        .execute(
            "DELETE FROM mcp_refresh_tokens WHERE expires_at < now()",
            &[],
        )
        .await?;
    let codes = conn
        .execute("DELETE FROM mcp_auth_codes WHERE expires_at < now()", &[])
        .await?;
    println!("GC MCP : {tokens} access + {refresh} refresh + {codes} code(s) expirés supprimés.");
    Ok(())
}

/// Prewarm des index de recherche après un redémarrage : sans lui, la
/// première requête paie l'ouverture des segments à froid.
///
/// La liste est **découverte dans le catalogue**, pas écrite ici : un index
/// neuf est couvert le jour où il existe.
///
/// Deux caches distincts, donc deux modes et aucune concurrence entre eux :
/// l'ANN VectorChord est épinglé en `shared_buffers`, le BM25 est tiré en
/// mode `read` (cache page OS) — les index dépassent ensemble
/// `shared_buffers`.
async fn db_prewarm(conn: &lj_store::db::Connection) -> Result<()> {
    // Le plus gros d'abord : si le cache manque, c'est lui qu'on veut dedans.
    const INDEXES: &str = "SELECT am.amname, c.relname, t.relname, pg_relation_size(c.oid) \
         FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_class t ON t.oid = i.indrelid \
         JOIN pg_am am ON am.oid = c.relam \
         WHERE am.amname IN ('vchordrq', 'bm25') \
         ORDER BY pg_relation_size(c.oid) DESC";

    let mut chauffes = 0usize;
    for row in conn.query(INDEXES, &[]).await? {
        let am: &str = row.get(0);
        let index: &str = row.get(1);
        let table: &str = row.get(2);
        let octets: i64 = row.get(3);

        // Les index du banc vivent dans la même base mais aucune requête servie ne
        // les lit.
        if table.starts_with("bench_") {
            println!("{index} : écarté (index du banc, hors chemin servi)");
            continue;
        }

        let go = octets as f64 / 1e9;
        if am == "vchordrq" {
            println!("vchordrq_prewarm('{index}') … [{go:.1} Go, épinglé en shared_buffers]");
            let r = conn
                .query_one("SELECT vchordrq_prewarm($1::text::regclass, 0)", &[&index])
                .await?;
            // Rend un RAPPORT texte multi-ligne (hauteur de l'arbre, nœuds, pages),
            // pas un compte de blocs.
            let rapport: String = r.get(0);
            for ligne in rapport.lines().filter(|l| !l.trim_matches('-').is_empty()) {
                println!("  {}", ligne.trim_end());
            }
        } else {
            println!("pg_prewarm('{index}', 'read') … [{go:.1} Go, cache page OS]");
            let r = conn
                .query_one("SELECT pg_prewarm($1::text::regclass, 'read')", &[&index])
                .await?;
            // `pg_prewarm` rend le nombre de blocs (int8) chargés.
            let blocs: i64 = r.get(0);
            println!("  {blocs} blocs");
        }
        chauffes += 1;
    }
    println!("{chauffes} index préchauffés");
    Ok(())
}
