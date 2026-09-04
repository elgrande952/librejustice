//! Scraper de métriques hôte → OTLP, sept séries.
//!
//! Elles distinguent un gel du runtime d'une stalle disque, ce qu'aucune
//! métrique applicative ne sait faire.
//!
//! Noms alignés sur le `hostmetricsreceiver` OTel là où il en a un, pour que la
//! conversion OTLP → Prometheus rende des noms prévisibles
//! (`system_cpu_utilization_ratio`, `system_cpu_load_average_1m_ratio`,
//! `system_memory_utilization_ratio`). Le PSI Linux n'est pas dans la semconv :
//! il sort en `system.linux.pressure`, une métrique portant un attribut
//! `resource` = `cpu` | `io` | `memory`, soit trois séries sous un nom.
//!
//! `/proc/{loadavg,stat,meminfo,pressure/*}` ne sont pas masqués par podman :
//! aucun privilège ni montage à ajouter. L'occupation du disque sort de
//! `statvfs(3)`, que `/proc` ne publie pas, et porte sur `/` — la racine overlay
//! rapporte le système de fichiers sous-jacent.

use opentelemetry::{global, KeyValue};
use std::sync::Mutex;
use std::time::Duration;
use tracing::{info, warn};

/// Période de scrape (1 DPM, free tier).
const SCRAPE_INTERVAL: Duration = Duration::from_secs(60);

/// Le PSI est lu sur sa fenêtre **avg60**, pas avg10 : à un scrape par minute,
/// avg10 ne couvre que le dernier sixième de l'intervalle et laisse passer un
/// gel de 30 s — exactement l'épisode qui a motivé ce module.
const PSI_WINDOW: &str = "avg60";

/// Snapshot lu par les callbacks OTel. `None` = jamais lu avec succès, la
/// métrique n'est alors pas observée du tout (mieux qu'un zéro qui mentirait).
struct HostSnapshot {
    cpu_utilization: Option<f64>,
    load1: Option<f64>,
    memory_utilization: Option<f64>,
    filesystem_utilization: Option<f64>,
    pressure_cpu: Option<f64>,
    pressure_io: Option<f64>,
    pressure_memory: Option<f64>,
}

impl HostSnapshot {
    const EMPTY: Self = Self {
        cpu_utilization: None,
        load1: None,
        memory_utilization: None,
        filesystem_utilization: None,
        pressure_cpu: None,
        pressure_io: None,
        pressure_memory: None,
    };
}

static SNAPSHOT: Mutex<HostSnapshot> = Mutex::new(HostSnapshot::EMPTY);

/// Compteurs cumulés de `/proc/stat` du tick précédent : l'utilisation CPU est
/// un delta, pas un instantané.
static CPU_PREV: Mutex<Option<(u64, u64)>> = Mutex::new(None);

/// Démarre le scraper : enregistre les gauges, puis rafraîchit le snapshot
/// toutes les 60 s. Conçu pour `tokio::spawn`.
pub async fn run() {
    register_gauges();
    info!(
        interval_secs = SCRAPE_INTERVAL.as_secs(),
        psi_window = PSI_WINDOW,
        "host_metrics : scraper démarré"
    );

    let mut ticker = tokio::time::interval(SCRAPE_INTERVAL);
    loop {
        ticker.tick().await;
        scrape();
    }
}

fn register_gauges() {
    let meter = global::meter("librejustice-host");

    let _cpu = meter
        .f64_observable_gauge("system.cpu.utilization")
        .with_description("Fraction du temps CPU non oisif, tous cœurs confondus.")
        .with_unit("1")
        .with_callback(|observer| {
            if let Some(v) = SNAPSHOT.lock().unwrap().cpu_utilization {
                observer.observe(v, &[KeyValue::new("state", "busy")]);
            }
        })
        .build();

    let _load = meter
        .f64_observable_gauge("system.cpu.load_average.1m")
        .with_description("Charge moyenne sur une minute.")
        .with_unit("1")
        .with_callback(|observer| {
            if let Some(v) = SNAPSHOT.lock().unwrap().load1 {
                observer.observe(v, &[]);
            }
        })
        .build();

    let _mem = meter
        .f64_observable_gauge("system.memory.utilization")
        .with_description("Fraction de la mémoire non disponible (MemTotal - MemAvailable).")
        .with_unit("1")
        .with_callback(|observer| {
            if let Some(v) = SNAPSHOT.lock().unwrap().memory_utilization {
                observer.observe(v, &[KeyValue::new("state", "used")]);
            }
        })
        .build();

    let _fs = meter
        .f64_observable_gauge("system.filesystem.utilization")
        .with_description("Fraction occupée du système de fichiers racine, au sens de `df`.")
        .with_unit("1")
        .with_callback(|observer| {
            if let Some(v) = SNAPSHOT.lock().unwrap().filesystem_utilization {
                observer.observe(v, &[KeyValue::new("mountpoint", "/")]);
            }
        })
        .build();

    let _psi = meter
        .f64_observable_gauge("system.linux.pressure")
        .with_description(
            "PSI Linux : fraction des 60 dernières secondes où au moins une tâche \
             était bloquée sur la ressource.",
        )
        .with_unit("1")
        .with_callback(|observer| {
            let snap = SNAPSHOT.lock().unwrap();
            for (resource, value) in [
                ("cpu", snap.pressure_cpu),
                ("io", snap.pressure_io),
                ("memory", snap.pressure_memory),
            ] {
                if let Some(v) = value {
                    observer.observe(v, &[KeyValue::new("resource", resource)]);
                }
            }
        })
        .build();
}

/// Lit `/proc` puis remplace le snapshot. Chaque famille garde sa valeur
/// précédente si sa lecture échoue. Les lectures se font **avant** le verrou :
/// un callback d'export n'attend pas un accès disque.
fn scrape() {
    let cpu = read_cpu_utilization();
    let load1 = read_load1();
    let memory = read_memory_utilization();
    let filesystem = read_filesystem_utilization();
    let pressure_cpu = read_pressure("cpu");
    let pressure_io = read_pressure("io");
    let pressure_memory = read_pressure("memory");

    let mut snap = SNAPSHOT.lock().unwrap();
    snap.cpu_utilization = cpu.or(snap.cpu_utilization);
    snap.load1 = load1.or(snap.load1);
    snap.memory_utilization = memory.or(snap.memory_utilization);
    snap.filesystem_utilization = filesystem.or(snap.filesystem_utilization);
    snap.pressure_cpu = pressure_cpu.or(snap.pressure_cpu);
    snap.pressure_io = pressure_io.or(snap.pressure_io);
    snap.pressure_memory = pressure_memory.or(snap.pressure_memory);
}

/// `/proc/loadavg` : premier champ.
fn read_load1() -> Option<f64> {
    let raw = read_proc("/proc/loadavg")?;
    raw.split_whitespace().next()?.parse().ok()
}

/// `/proc/stat`, ligne `cpu` : delta occupé / delta total depuis le tick
/// précédent. Rend `None` au premier passage, faute de point de comparaison.
fn read_cpu_utilization() -> Option<f64> {
    let raw = read_proc("/proc/stat")?;
    let line = raw.lines().find(|l| l.starts_with("cpu "))?;
    let f: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .filter_map(|x| x.parse().ok())
        .collect();
    if f.len() < 8 {
        return None;
    }
    let total: u64 = f.iter().sum();
    // idle + iowait : les deux formes d'inoccupation du receiver OTel.
    let busy = total.saturating_sub(f[3] + f[4]);

    let mut prev = CPU_PREV.lock().unwrap();
    let out = prev.and_then(|(pb, pt)| {
        let dt = total.checked_sub(pt)?;
        let db = busy.checked_sub(pb)?;
        (dt > 0).then(|| db as f64 / dt as f64)
    });
    *prev = Some((busy, total));
    out
}

/// `/proc/meminfo` : 1 - MemAvailable / MemTotal.
fn read_memory_utilization() -> Option<f64> {
    let raw = read_proc("/proc/meminfo")?;
    let kb = |key: &str| -> Option<f64> {
        raw.lines()
            .find(|l| l.starts_with(key))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    let total = kb("MemTotal:")?;
    let available = kb("MemAvailable:")?;
    (total > 0.0).then(|| 1.0 - available / total)
}

/// `statvfs("/")` : occupation au sens de `df`, soit `occupé / (occupé +
/// disponible)`. Les blocs réservés au superutilisateur (`f_bfree - f_bavail`,
/// 5 % par défaut en ext4) comptent donc comme occupés — les compter libres
/// annoncerait une place que le service, non privilégié, ne peut pas prendre.
fn read_filesystem_utilization() -> Option<f64> {
    let path = std::ffi::CString::new("/").ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `stat` est une valeur locale valide, `path` un CString vivant sur
    // toute la durée de l'appel.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        warn!(
            error = %std::io::Error::last_os_error(),
            "host_metrics : statvfs / échoué"
        );
        return None;
    }
    let occupe = (stat.f_blocks - stat.f_bfree) as f64;
    let utilisable = occupe + stat.f_bavail as f64;
    (utilisable > 0.0).then(|| occupe / utilisable)
}

/// `/proc/pressure/<resource>`, ligne `some`, champ [`PSI_WINDOW`]. Le PSI est
/// en pourcentage : on rend une fraction, comme les autres gauges.
fn read_pressure(resource: &str) -> Option<f64> {
    let raw = read_proc(&format!("/proc/pressure/{resource}"))?;
    let line = raw.lines().find(|l| l.starts_with("some "))?;
    let field = line.split_whitespace().find_map(|kv| {
        kv.strip_prefix(PSI_WINDOW)
            .and_then(|r| r.strip_prefix('='))
    })?;
    field.parse::<f64>().ok().map(|pct| pct / 100.0)
}

fn read_proc(path: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Some(raw),
        Err(err) => {
            warn!(path, error = %err, "host_metrics : lecture /proc échouée");
            None
        }
    }
}
