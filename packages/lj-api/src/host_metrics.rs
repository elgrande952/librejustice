use opentelemetry::{global, KeyValue};
use std::sync::Mutex;
use std::time::Duration;
use tracing::{info, warn};

const SCRAPE_INTERVAL: Duration = Duration::from_secs(60);

const PSI_WINDOW: &str = "avg60";

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

static CPU_PREV: Mutex<Option<(u64, u64)>> = Mutex::new(None);

pub async fn run() {
    register_gauges();
    info!(
        interval_secs = SCRAPE_INTERVAL.as_secs(),
        psi_window = PSI_WINDOW,
        "host_metrics: scraper started"
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

fn read_load1() -> Option<f64> {
    let raw = read_proc("/proc/loadavg")?;
    raw.split_whitespace().next()?.parse().ok()
}

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

fn read_filesystem_utilization() -> Option<f64> {
    let path = std::ffi::CString::new("/").ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `stat` est une valeur locale valide, `path` un CString vivant sur
    // toute la durée de l'appel.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        warn!(
            error = %std::io::Error::last_os_error(),
            "host_metrics: statvfs / failed"
        );
        return None;
    }
    let occupe = (stat.f_blocks - stat.f_bfree) as f64;
    let utilisable = occupe + stat.f_bavail as f64;
    (utilisable > 0.0).then(|| occupe / utilisable)
}

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
            warn!(path, error = %err, "host_metrics: /proc read failed");
            None
        }
    }
}
