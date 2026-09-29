//! Offline backfill of `decisions.summary`.
//!
//! Keyset iteration over the decisions whose summary is missing or older than the
//! target generation, then two shared files of jobs which the doors pull from, one
//! per shape: a decision whose court published its own commentary is pasted, the
//! others are rewritten.
//!
//! Nothing here sets a rate. A pool takes a job only when its own bucket can pay for
//! it, so the calls in flight are a consequence of the quotas rather than a setting,
//! and a refusal hands the job straight back to the queue instead of being ridden
//! out on the decision's back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::signal::unix::{signal, SignalKind};

use anyhow::{anyhow, bail, Result};
use lj_llm::codex;
use lj_llm::gemini::{GeminiClient, GeminiError};
use lj_llm::luna::{LunaClient, LunaError, LunaSeat};
use lj_llm::mistral::{MistralClient, MistralError};
use lj_store::repository::{CollageRow, Credential, DecisionRepository, MissingSummaryRow};
use lj_summary::{
    assemble_collage, build_summary_input, clean_summary, SummaryEngine, Thinking,
    COLLAGE_PROSE_MIN,
};

use crate::config::Settings;

/// Passes a decision gets across the pools before a later run has it: one that
/// three pools could not write is a defect to look at, not a loop to run.
const MAX_ATTEMPTS: u32 = 3;

/// The engines one door serves, one per shape of work, `None` where it serves none.
///
/// Both engines of a door share model and reasoning level, which is what lets one
/// client and one key ledger carry the two: a second client on the same keys would
/// hold a second rate ledger and spend the quota twice.
#[derive(Clone, Copy)]
struct Engines {
    rewrite: Option<SummaryEngine>,
    collage: Option<SummaryEngine>,
}

impl Engines {
    fn for_shape(self, shape: Shape) -> Option<SummaryEngine> {
        match shape {
            Shape::Rewrite => self.rewrite,
            Shape::Collage => self.collage,
        }
    }

    /// The engine the client is built from, either one saying the same model and
    /// the same level.
    fn any(self) -> SummaryEngine {
        self.rewrite
            .or(self.collage)
            .expect("une porte sert au moins une forme")
    }
}

/// Thinking tokens are charged here too, so this is the bench's 16 000 and not the
/// 2 000 that sufficed while no Gemini engine reasoned: the summary is about 101.
const GEMINI_MAX_OUTPUT_TOKENS: u32 = 16_000;

/// A model a run may spend, each writing with the engines of the current
/// generation, until its quota refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Model {
    // No collage on gemini, and not for want of quality: a request is what this
    // door is rationed on, 500 a day and per key, and a collage spends a whole one
    // on two sentences the token metered door writes for almost nothing.
    #[value(name = "gemini-3.1-flash-lite")]
    Gemini31FlashLite,
    #[value(name = "gemini-3.5-flash-lite")]
    Gemini35FlashLite,
    #[value(name = "gpt-6-luna")]
    Gpt6Luna,
    // Out of the chain while the live search rerank shares these keys: on
    // 20/09/2026 the backfill held the twenty at zero tokens a minute and 81 % of
    // rankings came back degraded. It writes at 13 to 16 euros per thousand,
    // twelve times luna for the same substance.
    #[value(name = "mistral-medium-latest")]
    MistralMedium,
}

/// What the scheduled chain spends. The free gemini quota is held per model, so a
/// second model is capacity the first one does not have.
pub const CHAIN_MODELS: &[Model] = &[
    Model::Gemini31FlashLite,
    Model::Gemini35FlashLite,
    Model::Gpt6Luna,
];

impl Model {
    fn engines(self) -> Engines {
        let rewrite = |engine| Engines {
            rewrite: Some(engine),
            collage: None,
        };
        match self {
            Self::Gemini31FlashLite => rewrite(SummaryEngine::Gemini31MonoV10),
            Self::Gemini35FlashLite => rewrite(SummaryEngine::Gemini35MonoV10),
            Self::Gpt6Luna => Engines {
                rewrite: Some(SummaryEngine::Luna6MonoV10),
                collage: Some(SummaryEngine::Luna6CollageV10),
            },
            Self::MistralMedium => rewrite(SummaryEngine::MistralMediumMonoV10),
        }
    }
}

/// The same door, serving the catch-up alone, which has nothing to rewrite.
const LUNA_COLLAGE_ONLY: Engines = Engines {
    rewrite: None,
    collage: Some(SummaryEngine::Luna6CollageV10),
};

/// Sleep floor of a driver whose pool cannot pay: the quota says how long to wait,
/// and this only keeps a wait of zero from spinning.
const IDLE_FLOOR_S: f64 = 0.05;

/// Gemini's 503 is an overloaded model, not a quota, and arrives in bursts of
/// several minutes: a job is retried eight times over about seven minutes.
const MAX_UNAVAILABLE: u32 = 8;
const UNAVAILABLE_BACKOFF_MAX_S: f64 = 120.0;

/// Drivers held per call the demand asks for. The two sides are not symmetric: an
/// excess sleeps, a shortfall leaves quota unused, so the factor sits above one. Two
/// covers a doubling of the round trip without touching the admitted rate, which the
/// ledger holds whatever the driver count.
const DRIVER_MARGIN: f64 = 2.0;

/// Libellés humains du `jurisdiction_type`.
fn juridiction_label(jurisdiction_type: &str) -> Option<&'static str> {
    Some(match jurisdiction_type {
        "TA" => "Tribunal administratif",
        "CAA" => "Cour administrative d'appel",
        "CE" => "Conseil d'État",
        "CC" => "Cour de cassation",
        "CA" => "Cour d'appel",
        "TJ" => "Tribunal judiciaire",
        "TCOM" => "Tribunal de commerce",
        _ => return None,
    })
}

const FR_MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// « 2026-05-29 » → « 29 mai 2026 ». Passe-plat si pas une date ISO complète.
fn format_fr_date(date_lecture: &str) -> String {
    let parts: Vec<&str> = date_lecture.split('-').collect();
    if parts.len() != 3 {
        return date_lecture.to_string();
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        parts[0].parse::<i32>(),
        parts[1].parse::<usize>(),
        parts[2].parse::<i32>(),
    ) else {
        return date_lecture.to_string();
    };
    match FR_MONTHS.get(month.wrapping_sub(1)) {
        Some(m) => format!("{day} {m} {year}"),
        None => date_lecture.to_string(),
    }
}

/// Nom de juridiction affichable : `jurisdiction_name` assaini, sinon libellé.
fn decision_jurisdiction(jurisdiction_type: &str, jurisdiction_name: Option<&str>) -> String {
    if let Some(name) = jurisdiction_name {
        let cleaned = name.trim().replace(" ,", ",");
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            return cleaned.to_string();
        }
    }
    juridiction_label(jurisdiction_type)
        .unwrap_or(jurisdiction_type)
        .to_string()
}

/// Titre lisible « <juridiction>, <date FR>, <numéro> ». Date et numéro
/// optionnels.
pub fn decision_title(
    jurisdiction_type: &str,
    jurisdiction_name: Option<&str>,
    date_lecture: Option<&str>,
    docket_numbers: Option<&[String]>,
) -> String {
    let mut parts = vec![decision_jurisdiction(jurisdiction_type, jurisdiction_name)];
    if let Some(date) = date_lecture {
        parts.push(format_fr_date(date));
    }
    if let Some(dockets) = docket_numbers {
        if let Some(first) = dockets.first() {
            parts.push(first.clone());
        }
    }
    parts.join(", ")
}

/// En-tête « [Décision] <titre> » pour donner au modèle la juridiction exacte.
fn summary_input(body_text: &str, title: &str) -> String {
    build_summary_input(&format!("[Décision] {title}\n\n{body_text}"))
}

/// The shape a decision is written in: its court's own words pasted, a rewrite
/// when the court published none, and nothing when no door of the run can paste.
/// A rewrite trades 0,96 of F-measure for 0,73 against the court's words, and the
/// row would then be written twice.
fn work_for(
    row: &MissingSummaryRow,
    commentary: Option<String>,
    can_collage: bool,
) -> Option<Work> {
    match commentary {
        Some(commentary) => can_collage.then_some(Work::Collage { commentary }),
        None => Some(Work::Rewrite {
            title: row_title(row),
        }),
    }
}

/// A door, its quota and the engines it serves. The engine is a property of the
/// job and not of the door: that pairing is what the bench graded, and a prompt
/// served by another model is a summary nobody measured.
enum Pool {
    Gemini(Arc<GeminiClient>, Engines),
    Mistral(Arc<MistralClient>, Engines),
    Luna(Arc<LunaClient>, Engines),
}

/// What a pool did with a job.
enum Attempt {
    Written(String),
    /// Nothing was sent: the pool cannot pay for this many seconds.
    Busy(f64),
    /// The pool is out until the quota resets, so it takes no further job.
    Handover,
    /// The model is overloaded: the job waits, the key is not at fault.
    Unavailable(String),
    Failed(String),
}

impl Pool {
    fn engines(&self) -> Engines {
        match self {
            Pool::Gemini(_, e) | Pool::Mistral(_, e) | Pool::Luna(_, e) => *e,
        }
    }

    fn engine_for(&self, shape: Shape) -> Option<SummaryEngine> {
        self.engines().for_shape(shape)
    }

    /// The file a door empties first. A collage costs the token metered door two
    /// sentences where a rewrite costs it the whole summary, and costs a request
    /// metered door the same one request either way. So the subscription seat goes
    /// to the collages and the free keys to the rewrites, each helping on the other
    /// file once its own is empty.
    fn prefers(&self) -> Shape {
        match self {
            Pool::Luna(..) => Shape::Collage,
            Pool::Gemini(..) | Pool::Mistral(..) => Shape::Rewrite,
        }
    }

    /// Order the doors are served in, lowest first. A door waits while a lower
    /// rank still has a driver able to take a job.
    ///
    /// The day's decisions go to the subscription seat, whose capacity expires in
    /// hours and is spent or lost. The free keys are rationed by the day and keep
    /// theirs for the catch-up, which has its own process.
    fn rank(&self) -> u8 {
        match self {
            Pool::Luna(..) => 0,
            Pool::Gemini(..) | Pool::Mistral(..) => 1,
        }
    }

    /// Keys the pool holds, one call at a time each.
    fn keys(&self) -> usize {
        match self {
            Pool::Gemini(client, _) => client.key_count(),
            Pool::Mistral(client, _) => client.live_key_count().max(1),
            Pool::Luna(client, _) => client.key_count(),
        }
    }

    /// Drivers the pool gets for a batch of `jobs`. One per key is the floor, which
    /// is what a pool that has served nothing yet runs on. Above it the measured
    /// demand rules: a quota paying for more calls in flight than the pool has keys
    /// is exactly what one driver per key cannot serve, and a large RPM or TPM
    /// allowance is that case. A driver the ledger refuses puts its job back and
    /// sleeps, so an excess costs a parked task and never a job held from another
    /// pool; a shortfall costs quota bought and left unused. Hence the margin.
    fn drivers(&self, jobs: usize) -> usize {
        let keys = self.keys();
        let (measured, mute) = self.demand();
        let heard = keys.saturating_sub(mute);
        let demand = if heard == 0 {
            0.0
        } else {
            measured * keys as f64 / heard as f64
        };
        let wanted = (demand * DRIVER_MARGIN).ceil() as usize;
        wanted.max(keys).min(jobs.max(1))
    }

    /// Calls in flight the quota justifies holding, against the drivers the pool
    /// was given. A gap either way is what an adaptive count would close.
    fn demand(&self) -> (f64, usize) {
        match self {
            Pool::Gemini(client, _) => client.demand(),
            Pool::Mistral(client, _) => client.demand(),
            Pool::Luna(client, _) => client.demand(),
        }
    }

    /// A refusal is a state of the pool, not a failure of the job: the ledger has
    /// already cut its rate, so the job goes back to the queue and pays nothing.
    async fn summarise(&self, job: &Job, engine: SummaryEngine) -> Attempt {
        let input = job.work.input(&job.body);
        match self {
            Pool::Gemini(client, _) => match client
                .generate_now(engine.prompt(), &input, GEMINI_MAX_OUTPUT_TOKENS)
                .await
            {
                Ok(raw) => job.work.finish(&raw),
                Err(GeminiError::Busy(wait)) => Attempt::Busy(wait),
                Err(GeminiError::Status(429)) => Attempt::Busy(0.0),
                Err(GeminiError::Exhausted) => Attempt::Handover,
                Err(err @ GeminiError::Status(503)) => Attempt::Unavailable(err.to_string()),
                Err(err) => Attempt::Failed(err.to_string()),
            },
            Pool::Mistral(client, _) => {
                match client
                    .chat_now(engine.prompt(), &input, mistral_cap(engine))
                    .await
                {
                    Ok(raw) => job.work.finish(&raw),
                    Err(MistralError::Busy(wait)) => Attempt::Busy(wait),
                    Err(MistralError::Status(429)) => Attempt::Busy(0.0),
                    Err(MistralError::Status(code))
                        if (code == 401 || code == 402) && client.live_key_count() == 0 =>
                    {
                        Attempt::Handover
                    }
                    Err(err) => Attempt::Failed(err.to_string()),
                }
            }
            Pool::Luna(client, _) => match client
                .generate_now(engine.model(), luna_effort(engine), engine.prompt(), &input)
                .await
            {
                Ok(raw) => job.work.finish(&raw),
                Err(LunaError::Busy(wait)) => Attempt::Busy(wait),
                Err(LunaError::Exhausted) => Attempt::Handover,
                Err(err) => Attempt::Failed(err.to_string()),
            },
        }
    }
}

/// What the model is asked for on a decision, and what its answer becomes.
///
/// Two shapes, because a decision whose court published its own commentary does not
/// need rewriting: the model writes the two sentences that frame it and the code
/// pastes the commentary between them, word for word.
enum Work {
    Rewrite { title: String },
    Collage { commentary: String },
}

/// Which of the two shapes a job is, and which file it waits in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    Collage,
    Rewrite,
}

impl Shape {
    fn other(self) -> Self {
        match self {
            Self::Collage => Self::Rewrite,
            Self::Rewrite => Self::Collage,
        }
    }
}

impl Work {
    fn shape(&self) -> Shape {
        match self {
            Work::Rewrite { .. } => Shape::Rewrite,
            Work::Collage { .. } => Shape::Collage,
        }
    }

    fn input(&self, body: &str) -> String {
        match self {
            Work::Rewrite { title } => summary_input(body, title),
            Work::Collage { commentary } => format!(
                "=== DÉCISION ===\n{}\n\n=== SOMMAIRE OFFICIEL DE LA JURIDICTION ===\n{commentary}\n",
                build_summary_input(body)
            ),
        }
    }

    fn finish(&self, raw: &str) -> Attempt {
        let summary = match self {
            Work::Rewrite { .. } => clean_summary(raw),
            Work::Collage { commentary } => assemble_collage(raw, commentary)
                .map(|assembled| clean_summary(&assembled))
                .unwrap_or_default(),
        };
        if summary.is_empty() {
            return Attempt::Failed("résumé vide".to_string());
        }
        Attempt::Written(summary)
    }
}

/// A decision to summarise, and the passes it has already cost.
struct Job {
    decision_id: i64,
    body: String,
    work: Work,
    attempts: u32,
    unavailable: u32,
}

/// One file per shape, so a door can be served what it is cheapest on without
/// scanning past what it cannot serve at all.
#[derive(Default)]
struct Files {
    collages: std::collections::VecDeque<Job>,
    rewrites: std::collections::VecDeque<Job>,
}

type Queue = Arc<std::sync::Mutex<Files>>;

impl Files {
    fn push_back(&mut self, job: Job) {
        match job.work.shape() {
            Shape::Collage => self.collages.push_back(job),
            Shape::Rewrite => self.rewrites.push_back(job),
        }
    }

    fn push_front(&mut self, job: Job) {
        match job.work.shape() {
            Shape::Collage => self.collages.push_front(job),
            Shape::Rewrite => self.rewrites.push_front(job),
        }
    }

    /// The next job this door has an engine for, its preferred shape first. A door
    /// with no engine for a shape never sees that file.
    fn take(&mut self, pool: &Pool) -> Option<Job> {
        let first = pool.prefers();
        [first, first.other()]
            .into_iter()
            .filter(|shape| pool.engine_for(*shape).is_some())
            .find_map(|shape| match shape {
                Shape::Collage => self.collages.pop_front(),
                Shape::Rewrite => self.rewrites.pop_front(),
            })
    }

    fn len(&self) -> usize {
        self.collages.len() + self.rewrites.len()
    }
}

/// A summary written, on its way to the store.
struct Done {
    decision_id: i64,
    summary: String,
    engine: SummaryEngine,
}

/// Drivers of rank 0 still able to take a job, which a higher rank waits on.
type Ahead = Arc<std::sync::atomic::AtomicUsize>;

/// How often a waiting driver rereads [`Ahead`].
const AHEAD_POLL_S: f64 = 0.5;

/// Set by SIGTERM or SIGINT during a run: the drivers take no new job, and the
/// calls in flight, already paid for, are written before the run ends.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// True while a run drains on a signal. The chain runs its steps in one process, so
/// outside a run, or on a second signal, the signal ends the process as it would
/// without a handler.
static RUNNING: AtomicBool = AtomicBool::new(false);
static HANDLER: std::sync::Once = std::sync::Once::new();

struct Run;

impl Run {
    fn start() -> Self {
        HANDLER.call_once(|| {
            tokio::spawn(async {
                let mut term = signal(SignalKind::terminate()).expect("handler SIGTERM");
                loop {
                    tokio::select! {
                        _ = term.recv() => {}
                        _ = tokio::signal::ctrl_c() => {}
                    }
                    if !RUNNING.load(Ordering::Relaxed) || STOPPING.swap(true, Ordering::Relaxed) {
                        std::process::exit(143);
                    }
                    tracing::warn!("summary_stopping");
                }
            });
        });
        RUNNING.store(true, Ordering::Relaxed);
        Run
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Relaxed);
    }
}

/// Which doors a run lets pull at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doors {
    /// The lowest rank serves alone while it can, the others wait behind it. The
    /// day's few decisions then go to the window that expires in hours, and the
    /// free day is left whole for the drain.
    ByRank,
    /// All of them together. A drain queues more work than any door's day holds,
    /// so holding one back writes no summary anywhere and lets its day expire:
    /// gemini's 8 000 free requests are lost at Pacific midnight.
    All,
}

/// Pulls from the shared queue as fast as its own quota allows, and no faster, once
/// no better ranked door is left to take the job.
async fn drive(
    pool: Arc<Pool>,
    queue: Queue,
    tx: tokio::sync::mpsc::Sender<Done>,
    ahead: Ahead,
    doors: Doors,
) {
    use std::sync::atomic::Ordering::Relaxed;
    if doors == Doors::ByRank && pool.rank() > 0 {
        while ahead.load(Relaxed) > 0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(AHEAD_POLL_S)).await;
        }
    }
    pull(&pool, &queue, &tx).await;
    if pool.rank() == 0 {
        ahead.fetch_sub(1, Relaxed);
    }
}

/// The job is taken for the length of one call: a pool that cannot pay hands it
/// straight back, where a worker holding it would park it behind a bucket.
async fn pull(pool: &Arc<Pool>, queue: &Queue, tx: &tokio::sync::mpsc::Sender<Done>) {
    let take = || {
        if STOPPING.load(Ordering::Relaxed) {
            return None;
        }
        queue.lock().expect("file non empoisonnée").take(pool)
    };
    while let Some(mut job) = take() {
        let engine = pool
            .engine_for(job.work.shape())
            .expect("la file ne rend que les formes que la porte sert");
        match pool.summarise(&job, engine).await {
            Attempt::Written(summary) => {
                let done = Done {
                    decision_id: job.decision_id,
                    summary,
                    engine,
                };
                if tx.send(done).await.is_err() {
                    return;
                }
            }
            Attempt::Busy(wait) => {
                queue.lock().expect("file non empoisonnée").push_front(job);
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait.max(IDLE_FLOOR_S)))
                    .await;
            }
            Attempt::Handover => {
                queue.lock().expect("file non empoisonnée").push_front(job);
                return;
            }
            Attempt::Unavailable(err) => {
                job.unavailable += 1;
                if job.unavailable >= MAX_UNAVAILABLE {
                    tracing::error!(
                        decision_id = job.decision_id,
                        engine = engine.code(),
                        error = %err,
                        "summary_failed"
                    );
                    continue;
                }
                let wait = (2f64.powi(job.unavailable as i32) * 2.5).min(UNAVAILABLE_BACKOFF_MAX_S);
                queue.lock().expect("file non empoisonnée").push_back(job);
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
            }
            Attempt::Failed(err) => {
                job.attempts += 1;
                if job.attempts >= MAX_ATTEMPTS {
                    tracing::error!(
                        decision_id = job.decision_id,
                        engine = engine.code(),
                        error = %err,
                        "summary_failed"
                    );
                } else {
                    queue.lock().expect("file non empoisonnée").push_back(job);
                }
            }
        }
    }
}

/// Format ETA « 12h03m04s ».
pub fn format_eta(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    format!("{h}h{m:02}m{sec:02}s")
}

/// Corps de la décision pour le résumé, depuis `decisions.full_text` (= le
/// `texte_integral_clean` indexé).
///
/// On découpe `full_text` en lignes, on trim chacune, on jette les vides, et on
/// joint par une ligne blanche. `None` si pas de paragraphe non vide.
fn body_text_from_full_text(full_text: &str) -> Option<String> {
    let paragraphs: Vec<&str> = full_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if paragraphs.is_empty() {
        return None;
    }
    Some(paragraphs.join("\n\n"))
}

/// Tirage `rand01` ∈ [0, 1) non cryptographique (jitter / sampling). Dérivé de
/// l'horloge — suffisant pour désynchroniser des workers / varier l'ordre.
fn rand01() -> f64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64)
        / 1_000_000_000.0
}

/// Supreme and European courts, then appeal, then the authorities, then first
/// instance. A type left out comes last.
pub const JURISDICTION_TIERS: &[(&str, i32)] = &[
    ("CONSTIT", 0),
    ("TC", 0),
    ("CE", 0),
    ("CC", 0),
    ("CJUE", 0),
    ("CEDH", 0),
    ("CAA", 1),
    ("CA", 1),
    ("CNIL", 2),
    ("CNDA", 2),
    ("TA", 3),
    ("TJ", 3),
    ("TCOM", 3),
];

/// Which stale decisions a run takes, and in what order.
#[derive(Debug, Clone)]
pub enum Order {
    /// Every one of them, by id: the sweep that finishes the corpus.
    Sweep,
    /// Cited at least `min` times inside the corpus, most cited first. The backlog
    /// is 3,6 million rows where a day of every door is thousands, so a run that
    /// cannot finish spends its calls where a reader is.
    Cited { min: i32 },
    /// Every one of them by [`JURISDICTION_TIERS`], most recent first inside a tier:
    /// the sweep that spends a week where a reader is once the cited ones are done.
    Jurisdiction { reverse: bool },
    /// What engine `code` wrote, in one jurisdiction type or in all of them: the
    /// repass of a text that changed without its generation moving, which no
    /// version predicate can select. Measured 21/09/2026 on the 280 commented
    /// decisions, repassing the Conseil d'État repairs one faulty decision in
    /// sixteen and takes the outcome clause from 37 to 92 %, cassation gaining none.
    Engine {
        code: i16,
        jurisdiction: Option<String>,
    },
}

/// Fills `decisions.summary` up to `summary_prompt_version >= target_version`.
///
/// Reads and writes on two connections of the pool. No rate is set here: each quota
/// bucket admits what it can pay for, the drivers pull from one shared queue, and
/// the writes are serialised on a single connection.
#[allow(clippy::too_many_arguments)]
pub async fn backfill_summaries(
    target_version: i16,
    batch_size: i64,
    limit: Option<i64>,
    shuffle: bool,
    refresh_stale: bool,
    order: Order,
    doors: Doors,
    models: &[Model],
    accounts: &[String],
) -> Result<()> {
    let _run = Run::start();
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let fetch_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get fetch: {e}"))?;
    let write_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get write: {e}"))?;
    // Backfill de maintenance : on lève la borne API (`build_pool` pose
    // `statement_timeout=30s`). Le scan de frontière des décisions sans résumé est
    // un batch long, pas une requête interactive.
    fetch_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout fetch: {e}"))?;
    write_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout write: {e}"))?;
    let fetch_repo = DecisionRepository::new(&fetch_conn);
    let write_repo = DecisionRepository::new(&write_conn);

    let picked = |c: &Credential| {
        accounts.is_empty()
            || accounts
                .iter()
                .any(|a| *a == c.tenant || *a == c.fingerprint)
    };
    let api_keys = |creds: Vec<Credential>| -> Vec<String> {
        creds
            .iter()
            .filter(|c| picked(c))
            .filter_map(|c| c.api_key().map(str::to_string))
            .collect()
    };
    let live_keys = api_keys(fetch_repo.credentials("mistral").await?);
    let gemini_keys = api_keys(fetch_repo.credentials("gemini").await?);
    let mistral_keys = live_keys.len();
    let mut mistral = Vec::new();
    let mut gemini = Vec::new();
    let mut luna = None;
    for model in models {
        let engines = model.engines();
        let engine = engines.any();
        match model {
            Model::MistralMedium => {
                let client = MistralClient::with_effort(
                    live_keys.clone(),
                    engine.model().to_string(),
                    engine.thinking().wire().map(str::to_string),
                )?;
                mistral.push((Arc::new(client), engines));
            }
            Model::Gemini31FlashLite | Model::Gemini35FlashLite => {
                if gemini_keys.is_empty() {
                    continue;
                }
                let client = GeminiClient::new(
                    gemini_keys.clone(),
                    engine.model().to_string(),
                    engine.thinking().wire().map(str::to_string),
                )?;
                client.seed_day(&fetch_repo.gemini_day_spent(engine.model()).await?);
                gemini.push((Arc::new(client), engines));
            }
            Model::Gpt6Luna => {
                luna = luna_client(&fetch_repo, &picked)
                    .await?
                    .map(|client| (client, engines));
            }
        }
    }
    let gemini = Arc::new(gemini);
    let luna_seats = luna.as_ref().map_or(0, |(c, _)| c.key_count());
    // A backup that runs alongside starves what it backs: sharing the queue, Mistral
    // takes 59 jobs a minute against Gemini's 10, measured 20/09/2026, and Gemini's
    // day of 16 000 free requests expires at 07:00 UTC where Mistral's runs to month end.
    let gemini_day_left: u32 = gemini.iter().map(|(client, _)| client.day_left()).sum();
    let pools: Vec<Arc<Pool>> = gemini
        .iter()
        .map(|(client, engines)| Arc::new(Pool::Gemini(Arc::clone(client), *engines)))
        .chain(
            (gemini_day_left == 0)
                .then(|| {
                    mistral.iter().map(|(client, engines)| {
                        Arc::new(Pool::Mistral(Arc::clone(client), *engines))
                    })
                })
                .into_iter()
                .flatten(),
        )
        .chain(
            luna.iter()
                .map(|(client, engines)| Arc::new(Pool::Luna(Arc::clone(client), *engines))),
        )
        .collect();
    let can_collage = pools.iter().any(|p| p.engine_for(Shape::Collage).is_some());
    if !can_collage && matches!(order, Order::Engine { .. }) {
        bail!("repassage du collage demandé sans porte qui colle : il faut un siège luna");
    }
    let drivers: usize = pools.iter().map(|p| p.keys()).sum();
    tracing::info!(
        mistral_keys,
        gemini_keys = gemini_keys.len(),
        luna_seats,
        pools = pools.len(),
        drivers,
        "summary_backfill_pools"
    );

    // Sampling de review : démarre la pagination keyset à un id aléatoire (avec
    // wraparound côté repo).
    let start_id = if shuffle {
        let max = fetch_repo.max_decision_id().await?;
        if max > 0 {
            (rand01() * (max as f64)) as i64
        } else {
            0
        }
    } else {
        0
    };

    let start = std::time::Instant::now();
    let mut processed: usize = 0;
    let mut persisted: usize = 0;
    let mut marked: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut par_moteur: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut left_to_collage: usize = 0;

    let batches = match &order {
        Order::Sweep => {
            fetch_repo
                .iter_decisions_missing_summary(
                    target_version,
                    batch_size,
                    limit,
                    start_id,
                    shuffle,
                    refresh_stale,
                )
                .await?
        }
        Order::Cited { min } => {
            fetch_repo
                .iter_summaries_by_citations(target_version, batch_size, limit, refresh_stale, *min)
                .await?
        }
        Order::Jurisdiction { reverse } => {
            let (types, tiers): (Vec<&str>, Vec<i32>) = JURISDICTION_TIERS.iter().copied().unzip();
            fetch_repo
                .iter_summaries_by_jurisdiction(
                    target_version,
                    batch_size,
                    limit,
                    refresh_stale,
                    &types,
                    &tiers,
                    *reverse,
                )
                .await?
        }
        Order::Engine { code, jurisdiction } => {
            fetch_repo
                .iter_summaries_by_engine(*code, jurisdiction.as_deref(), batch_size, limit)
                .await?
        }
    };
    tracing::info!(
        order = ?order,
        decisions = batches.iter().map(Vec::len).sum::<usize>(),
        "summary_backfill_queue"
    );

    for batch in batches {
        let ids: Vec<i64> = batch.iter().map(|row| row.decision_id).collect();
        let mut full_texts: std::collections::HashMap<i64, String> = fetch_repo
            .fetch_full_texts(&ids)
            .await?
            .into_iter()
            .collect();
        // What each decision carries decides its shape, one indexed lookup on the
        // batch's ids: a court that published its own commentary is not rewritten,
        // its words are pasted, 0,96 of F-measure against 0,73.
        let mut commentaries: std::collections::HashMap<i64, String> = fetch_repo
            .fetch_collage_rows(&ids, COLLAGE_PROSE_MIN as i32)
            .await?
            .into_iter()
            .map(|row| (row.decision_id, row.commentary))
            .collect();

        let queue: Queue = Arc::new(std::sync::Mutex::new(Files::default()));
        for row in &batch {
            let body = full_texts
                .remove(&row.decision_id)
                .and_then(|ft| body_text_from_full_text(&ft));
            let Some(body) = body else {
                tracing::warn!(
                    public_id = %row.public_id,
                    decision_id = row.decision_id,
                    "summary_no_body"
                );
                continue;
            };
            let Some(work) = work_for(row, commentaries.remove(&row.decision_id), can_collage)
            else {
                left_to_collage += 1;
                continue;
            };
            queue.lock().expect("file non empoisonnée").push_back(Job {
                decision_id: row.decision_id,
                body,
                work,
                attempts: 0,
                unavailable: 0,
            });
        }

        let jobs = queue.lock().expect("file non empoisonnée").len();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Done>(64);
        let mut running = Vec::new();
        let ahead: Ahead = Arc::new(std::sync::atomic::AtomicUsize::new(
            pools
                .iter()
                .filter(|p| p.rank() == 0)
                .map(|p| p.drivers(jobs))
                .sum(),
        ));
        for pool in &pools {
            for _ in 0..pool.drivers(jobs) {
                running.push(tokio::spawn(drive(
                    Arc::clone(pool),
                    Arc::clone(&queue),
                    tx.clone(),
                    Arc::clone(&ahead),
                    doors,
                )));
            }
        }
        // The writer's only sender must be the drivers, or the loop below never ends.
        drop(tx);

        processed += batch.len();
        while let Some(done) = rx.recv().await {
            write_repo
                .set_summary(
                    done.decision_id,
                    &done.summary,
                    done.engine.prompt_version(),
                    done.engine.code(),
                )
                .await
                .map_err(|e| anyhow!("set_summary id={}: {e}", done.decision_id))?;
            persisted += 1;
            *par_moteur.entry(done.engine.model()).or_default() += 1;
        }
        for driver in running {
            driver.await.map_err(|e| anyhow!("join: {e}"))?;
        }

        // Every pool handed over: what is left in the queue belongs to a later run,
        // and pulling the next batch would only pile up the same refusals.
        let left = queue.lock().expect("file non empoisonnée").len();
        processed -= left;
        for (fp, status) in mistral
            .iter()
            .flat_map(|(client, _)| client.drain_spent_fingerprints())
        {
            if marked.insert(fp.clone()) {
                write_repo
                    .disable_credential(&fp, None, status as i16, "ingest")
                    .await?;
                tracing::warn!(fingerprint = %fp, status, "mistral_key_disabled_marked");
            }
        }
        flush_gemini_day(&write_repo, &gemini).await?;
        flush_luna_rotations(
            &write_repo,
            luna.as_ref().map(|(client, _)| client.as_ref()),
        )
        .await?;
        let elapsed = start.elapsed().as_secs_f64();
        let rate = processed as f64 / elapsed.max(1e-6);
        tracing::info!(
            processed,
            persisted,
            rate,
            par_moteur = %format_par_moteur(&par_moteur),
            elapsed = %format_eta(elapsed),
            "summary_progress"
        );
        log_gemini_quota(&gemini);
        log_demand(&pools);
        log_luna_seats(luna.as_ref().map(|(client, _)| client));
        if STOPPING.load(Ordering::Relaxed) {
            tracing::warn!(left, "summary_stopped");
            break;
        }
        if left > 0 {
            tracing::warn!(left, "summary_pools_handed_over");
            break;
        }
    }

    flush_gemini_day(&write_repo, &gemini).await?;
    flush_luna_rotations(
        &write_repo,
        luna.as_ref().map(|(client, _)| client.as_ref()),
    )
    .await?;
    log_gemini_quota(&gemini);
    log_demand(&pools);
    tracing::info!(
        processed,
        persisted,
        par_moteur = %format_par_moteur(&par_moteur),
        left_to_collage,
        elapsed = %format_eta(start.elapsed().as_secs_f64()),
        "summary_done"
    );
    Ok(())
}

/// Seats in the order the door serves them: the shortest window first, since it is
/// the first to expire, then the least spent. A 5-hour window goes before a week
/// and a week before the 30 days of a free plan, each seat reading its own.
///
/// This is a priority and not a rotation: `Ledger::reserve` takes the first seat
/// that can pay, so the head absorbs everything its bucket admits. A seat the
/// endpoint will not answer for goes last rather than stopping the run.
async fn by_expiry(seats: Vec<LunaSeat>) -> Vec<LunaSeat> {
    let http = match reqwest::Client::builder()
        .user_agent("librejustice-ingest")
        .build()
    {
        Ok(http) => http,
        Err(_) => return seats,
    };
    let mut keyed: Vec<(i64, i64, LunaSeat)> = Vec::with_capacity(seats.len());
    for seat in seats {
        let key = match codex::usage(&http, &seat.access_token, &seat.account_id).await {
            Ok(usage) => usage
                .windows()
                .first()
                .map(|w| (w.limit_window_seconds, w.used_percent as i64))
                .unwrap_or((i64::MAX, i64::MAX)),
            Err(_) => (i64::MAX, i64::MAX),
        };
        keyed.push((key.0, key.1, seat));
    }
    keyed.sort_by_key(|(window, used, _)| (*window, *used));
    for (window, used, seat) in &keyed {
        tracing::info!(
            seat = %seat.fingerprint,
            fenetre_s = window,
            utilise_pct = used,
            "luna_ordre_des_sieges"
        );
    }
    keyed.into_iter().map(|(_, _, seat)| seat).collect()
}

/// The seats of the subscription door, or `None` when the base holds no live one.
async fn luna_client(
    repo: &DecisionRepository<'_>,
    picked: &impl Fn(&Credential) -> bool,
) -> Result<Option<Arc<LunaClient>>> {
    let seats: Vec<LunaSeat> = repo
        .credentials("openai")
        .await?
        .iter()
        .filter(|c| picked(c))
        .filter_map(|c| {
            Some(LunaSeat {
                fingerprint: c.fingerprint.clone(),
                account_id: c.account.clone()?,
                access_token: c.access_token()?.to_string(),
                refresh_token: c.refresh_token()?.to_string(),
            })
        })
        .collect();
    if seats.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(LunaClient::new(by_expiry(seats).await)?)))
}

/// Room for the summary alone, or none when the model reasons first: its
/// reasoning counts against the same cap and emptied one summary in three at 6 000.
fn mistral_cap(engine: SummaryEngine) -> Option<u32> {
    (engine.thinking() != Thinking::High).then_some(400)
}

/// The door refuses a call without an effort, which every luna engine declares.
fn luna_effort(engine: SummaryEngine) -> &'static str {
    engine
        .thinking()
        .wire()
        .expect("un moteur luna déclare son niveau de réflexion")
}

/// Identifiers per pass of the re-cleaning. The whole stock is read, about 3,8
/// million rows, and only what the cleaning changes is written back.
const RECLEAN_BATCH: i64 = 50_000;

/// Replays [`clean_summary`] over the stored summaries and writes back the ones it
/// changes, without touching the engine that wrote them.
///
/// The cleaning is deterministic and it gains rules: the header echo, the
/// anonymisation placeholders, then on 21/09/2026 the glyphs outside the
/// repertoire, which the token metered door was gluing to 1,9 % of its summaries.
/// A rule added upstream leaves the stock behind until this runs.
pub async fn reclean_summaries() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let read = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get read: {e}"))?;
    let write = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get write: {e}"))?;
    read.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let read_repo = DecisionRepository::new(&read);
    let write_repo = DecisionRepository::new(&write);
    let Some((min_id, max_id)) = read_repo.decision_id_bounds().await? else {
        return Ok(());
    };
    let (mut seen, mut repaired) = (0u64, 0u64);
    let mut from = min_id;
    while from <= max_id {
        let to = from.saturating_add(RECLEAN_BATCH);
        for (id, summary) in read_repo.summaries_in_range(from, to).await? {
            seen += 1;
            let cleaned = clean_summary(&summary);
            if cleaned != summary {
                write_repo.rewrite_summary_text(id, &cleaned).await?;
                repaired += 1;
            }
        }
        from = to;
    }
    tracing::warn!(lues = seen, reparees = repaired, "resumes_renettoyes");
    Ok(())
}

/// Overwrites the summary of the decisions whose juridiction published its own
/// commentary, by pasting that commentary rather than passing it through a model.
///
/// One door serves it, the token metered one, because the call is two sentences and
/// a request metered door would spend a whole request on it. Measured 20/09/2026 on
/// 120 decisions graded by opus, the collage scores 0,96 of F-measure and 95 % of
/// precision where rewriting the same decisions scores 0,73.
pub async fn upgrade_summaries_commentaries(batch_size: i64, limit: Option<i64>) -> Result<()> {
    let doors = Doors::All;
    let _run = Run::start();
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let fetch_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get fetch: {e}"))?;
    let write_conn = pool
        .get()
        .await
        .map_err(|e| anyhow!("pool.get write: {e}"))?;
    fetch_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout fetch: {e}"))?;
    write_conn
        .batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout write: {e}"))?;
    let fetch_repo = DecisionRepository::new(&fetch_conn);
    let write_repo = DecisionRepository::new(&write_conn);

    let luna = luna_client(&fetch_repo, &|_: &Credential| true)
        .await?
        .ok_or_else(|| anyhow!("aucun siège luna dans llm_credential : porte inutilisable"))?;
    let pools: Vec<Arc<Pool>> = vec![Arc::new(Pool::Luna(Arc::clone(&luna), LUNA_COLLAGE_ONLY))];
    tracing::info!(
        seats = luna.key_count(),
        engine = LUNA_COLLAGE_ONLY.any().code(),
        "collage_backfill_pools"
    );

    let start = std::time::Instant::now();
    let mut todo = fetch_repo
        .collage_queue(
            &SummaryEngine::COLLAGES.map(SummaryEngine::code),
            COLLAGE_PROSE_MIN as i32,
        )
        .await?;
    if let Some(max) = limit {
        todo.truncate(max as usize);
    }
    tracing::info!(
        queued = todo.len(),
        elapsed = %format_eta(start.elapsed().as_secs_f64()),
        "collage_queue_built"
    );

    let mut processed: usize = 0;
    let mut persisted: usize = 0;
    for chunk in todo.chunks(batch_size as usize) {
        let page: Vec<CollageRow> = fetch_repo
            .fetch_collage_rows(chunk, COLLAGE_PROSE_MIN as i32)
            .await?;

        let queue: Queue = Arc::new(std::sync::Mutex::new(Files::default()));
        for row in &page {
            match body_text_from_full_text(&row.full_text) {
                Some(body) => queue.lock().expect("file non empoisonnée").push_back(Job {
                    decision_id: row.decision_id,
                    body,
                    work: Work::Collage {
                        commentary: row.commentary.clone(),
                    },
                    attempts: 0,
                    unavailable: 0,
                }),
                None => tracing::warn!(decision_id = row.decision_id, "collage_no_body"),
            }
        }

        let jobs = queue.lock().expect("file non empoisonnée").len();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Done>(64);
        let mut running = Vec::new();
        let ahead: Ahead = Arc::new(std::sync::atomic::AtomicUsize::new(
            pools
                .iter()
                .filter(|p| p.rank() == 0)
                .map(|p| p.drivers(jobs))
                .sum(),
        ));
        for pool in &pools {
            for _ in 0..pool.drivers(jobs) {
                running.push(tokio::spawn(drive(
                    Arc::clone(pool),
                    Arc::clone(&queue),
                    tx.clone(),
                    Arc::clone(&ahead),
                    doors,
                )));
            }
        }
        drop(tx);

        processed += page.len();
        while let Some(done) = rx.recv().await {
            write_repo
                .set_summary(
                    done.decision_id,
                    &done.summary,
                    done.engine.prompt_version(),
                    done.engine.code(),
                )
                .await
                .map_err(|e| anyhow!("set_summary id={}: {e}", done.decision_id))?;
            persisted += 1;
        }
        for driver in running {
            driver.await.map_err(|e| anyhow!("join: {e}"))?;
        }

        let remaining = queue.lock().expect("file non empoisonnée").len();
        flush_luna_rotations(&write_repo, Some(&luna)).await?;
        let elapsed = start.elapsed().as_secs_f64();
        tracing::info!(
            processed,
            persisted,
            queued = todo.len(),
            rate = processed as f64 / elapsed.max(1e-6),
            elapsed = %format_eta(elapsed),
            "collage_progress"
        );
        log_demand(&pools);
        if STOPPING.load(Ordering::Relaxed) {
            tracing::warn!(remaining, "collage_stopped");
            break;
        }
        if remaining > 0 {
            tracing::warn!(remaining, "collage_pool_handed_over");
            break;
        }
    }

    flush_luna_rotations(&write_repo, Some(&luna)).await?;
    tracing::info!(
        processed,
        persisted,
        elapsed = %format_eta(start.elapsed().as_secs_f64()),
        "collage_done"
    );
    Ok(())
}

/// What the quota justifies holding against what the pool was given, which is the
/// reading an adaptive driver count would act on.
fn log_demand(pools: &[Arc<Pool>]) {
    for pool in pools {
        let (demande, muettes) = pool.demand();
        tracing::info!(
            model = pool.engines().any().model(),
            cles = pool.keys(),
            demande,
            muettes,
            "summary_pool_demand"
        );
    }
}

/// Emitted per batch: the daily chain caps this command with a watchdog, so a report kept for the clean exit is one nobody reads.
fn log_gemini_quota(gemini: &[(Arc<GeminiClient>, Engines)]) {
    for (client, engines) in gemini {
        let quota = client.refused_quota().unwrap_or_default();
        tracing::info!(
            model = engines.any().model(),
            refus = client.refusals(),
            refus_nus = client.bare_refusals(),
            quota = %quota,
            rate_par_min = client.rate_per_min(),
            budget_restant = client.day_left(),
            "summary_gemini_quota"
        );
    }
}

/// What a seat cost: the door answers only a percentage of a window, so a seat
/// emptied in one run otherwise leaves no trace of the value it held.
fn log_luna_seats(luna: Option<&Arc<LunaClient>>) {
    let Some(client) = luna else {
        return;
    };
    for seat in client.spent_per_seat() {
        tracing::info!(
            seat = %seat.fingerprint,
            calls = seat.calls,
            input = seat.usage.input,
            cached = seat.usage.cached,
            cache_write = seat.usage.cache_write,
            output = seat.usage.output,
            "luna_seat_spent"
        );
    }
}

/// `mistral-medium-latest=2669 gemini-3.1-flash-lite=184`, which pool served what.
fn format_par_moteur(counts: &std::collections::BTreeMap<&'static str, usize>) -> String {
    counts
        .iter()
        .map(|(model, n)| format!("{model}={n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Per batch and never per generation: one round trip for a thousand summaries.
async fn flush_gemini_day(
    repo: &DecisionRepository<'_>,
    gemini: &[(Arc<GeminiClient>, Engines)],
) -> Result<()> {
    for (client, engines) in gemini {
        let pending = client.day_pending();
        if pending.is_empty() {
            continue;
        }
        let totals = repo
            .bump_gemini_day_spent(engines.any().model(), &pending)
            .await?;
        client.day_flushed(&pending, &totals);
    }
    Ok(())
}

/// Writes back the seats luna refreshed. The refresh token is single use, so one
/// kept in memory dies with the process and the seat needs a browser again.
async fn flush_luna_rotations(
    repo: &DecisionRepository<'_>,
    luna: Option<&LunaClient>,
) -> Result<()> {
    let Some(client) = luna else {
        return Ok(());
    };
    for rotation in client.take_rotations() {
        let secret = serde_json::json!({
            "access_token": rotation.access_token,
            "refresh_token": rotation.refresh_token,
        });
        repo.merge_credential_secret(&rotation.fingerprint, &secret)
            .await?;
        tracing::info!(fingerprint = %rotation.fingerprint, "luna_seat_rotated");
    }
    Ok(())
}

/// Titre lisible d'une ligne de backfill, injecté dans le prompt.
/// `docket_numbers` → premier numéro éventuel.
fn row_title(row: &MissingSummaryRow) -> String {
    decision_title(
        &row.jurisdiction_type,
        row.jurisdiction_name.as_deref(),
        row.date_lecture.as_deref(),
        row.docket_numbers.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_date() {
        assert_eq!(format_fr_date("2026-05-29"), "29 mai 2026");
        assert_eq!(format_fr_date("2026-01-01"), "1 janvier 2026");
        assert_eq!(format_fr_date("2026-12-31"), "31 décembre 2026");
        assert_eq!(format_fr_date("pas-une-date-x"), "pas-une-date-x");
        assert_eq!(format_fr_date("2026/05/29"), "2026/05/29");
    }

    #[test]
    fn title_full() {
        let t = decision_title(
            "CC",
            Some("Cour de cassation"),
            Some("2026-05-29"),
            Some(&["24-17.384".to_string()]),
        );
        assert_eq!(t, "Cour de cassation, 29 mai 2026, 24-17.384");
    }

    #[test]
    fn title_falls_back_to_label() {
        let t = decision_title("TA", None, Some("2026-05-29"), None);
        assert_eq!(t, "Tribunal administratif, 29 mai 2026");
    }

    #[test]
    fn jurisdiction_sanitised() {
        assert_eq!(
            decision_jurisdiction("TA", Some("  Tribunal administratif de Paris ,  ")),
            "Tribunal administratif de Paris,"
        );
        assert_eq!(decision_jurisdiction("CE", Some("   ")), "Conseil d'État");
    }

    #[test]
    fn eta_format() {
        assert_eq!(format_eta(0.0), "0h00m00s");
        assert_eq!(format_eta(3661.0), "1h01m01s");
        assert_eq!(format_eta(-5.0), "0h00m00s");
    }

    /// A gemini client is built for one model and one level, so a door carrying two
    /// engines would send one of them settings nobody graded. Luna names both per call.
    #[test]
    fn une_porte_gemini_ne_porte_qu_un_reglage_et_luna_nomme_le_sien() {
        for model in [Model::Gemini31FlashLite, Model::Gemini35FlashLite] {
            let engines = model.engines();
            if let (Some(rewrite), Some(collage)) = (engines.rewrite, engines.collage) {
                assert_eq!(rewrite.model(), collage.model(), "{rewrite:?}/{collage:?}");
                assert_eq!(rewrite.thinking(), collage.thinking());
            }
        }
        for engines in [Model::Gpt6Luna.engines(), LUNA_COLLAGE_ONLY] {
            for engine in [engines.rewrite, engines.collage].into_iter().flatten() {
                luna_effort(engine);
            }
        }
    }

    /// A door never sees a file it has no engine for, and takes its own shape first.
    #[test]
    fn la_file_sert_la_forme_que_la_porte_prefere_puis_l_autre() {
        let body = || "corps".to_string();
        let collage = || Job {
            decision_id: 1,
            body: body(),
            work: Work::Collage {
                commentary: "sommaire".into(),
            },
            attempts: 0,
            unavailable: 0,
        };
        let rewrite = || Job {
            decision_id: 2,
            body: body(),
            work: Work::Rewrite {
                title: "titre".into(),
            },
            attempts: 0,
            unavailable: 0,
        };

        let mut files = Files::default();
        files.push_back(collage());
        files.push_back(rewrite());
        let mistral = Pool::Mistral(
            Arc::new(MistralClient::new(vec!["k".into()], "m".into()).unwrap()),
            Engines {
                rewrite: Some(SummaryEngine::MistralSmallV9),
                collage: None,
            },
        );
        assert_eq!(files.take(&mistral).map(|j| j.decision_id), Some(2));
        assert_eq!(files.take(&mistral).map(|j| j.decision_id), None);
        assert_eq!(files.len(), 1);
    }
}
