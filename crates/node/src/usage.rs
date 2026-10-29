//! Per-app usage for billing, on `/metrics` (`calimero_app_*`).
//!
//! Each node counts only the WASM runs it made: an app's calls, `init`,
//! migrations and follow-ups on the leader that ran them, a strong view on the
//! leader, a stale view on the replica that answered it. A follower applies an
//! entry's diff and runs nothing, so it counts nothing for it, and a scraper
//! sums the nodes. A run whose result was thrown away (a leader race's entry
//! applied as a conflict, a deposed leader's entry, a component's timeout)
//! adds its CPU and memory but no call: the call is counted once, by the run
//! whose answer stood.
//!
//! - CPU is the run's thread CPU time (`CLOCK_THREAD_CPUTIME_ID` around it, on
//!   the one thread it runs on; [`THREAD_CPU`]), elsewhere its wall time. An
//!   outbound request waits on a thread of its own, so its wait is not CPU.
//! - Memory is byte-seconds: the run's linear memory times its wall time (memory
//!   is held while a run waits). The node cannot read how far an instance grew
//!   without the runtime saying (the runtime makes the instance and drops it), so
//!   the bytes are the linear memory the module declares at instantiation
//!   ([`declared_memory`]): a floor of the peak.
//! - State and blob bytes are gauges, sampled every [`SAMPLE_EVERY`] from the
//!   replica's own state ([`crate::app::AppMachine::storage_stats`]); every
//!   replica reports the same app, so a scraper takes one, never the sum.
//!
//! Labels are opaque ids only: `app`, its payer `org` ([`payer`]), and `kind`.
//! An app's series go once it leaves this node (deleted, or its replica moved).
//! Counters start at zero when the process starts.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::types::AppRecord;

/// How often a node samples the state and blob bytes of the apps it hosts.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(300);

/// Bytes in a WebAssembly page, unless a memory declares its own page size.
const WASM_PAGE_BYTES: u64 = 65_536;

/// Whether [`Run::cpu`] is the thread's CPU time on this platform (else wall time).
pub const THREAD_CPU: bool = cfg!(unix);

/// What a run was, for `calimero_app_calls_total{kind}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A logged run a request caused: a call, the app's `init`, a migration.
    Mutation,
    /// A view, strong or stale, and a declared view called through `/intents`.
    View,
    /// A run the node made for the app: an event handler, a timer, an xcall
    /// (counted on the app it called).
    FollowUp,
}

impl Kind {
    const ALL: [Self; 3] = [Self::Mutation, Self::View, Self::FollowUp];

    const fn label(self) -> &'static str {
        match self {
            Self::Mutation => "mutation",
            Self::View => "view",
            Self::FollowUp => "followup",
        }
    }
}

/// Who an app's usage is billed to: its organization's id, or for a personal
/// app `account:{owner}` (`:` is not base64url, so the two never collide).
#[must_use]
pub fn payer(app: &AppRecord) -> String {
    app.org
        .clone()
        .unwrap_or_else(|| format!("account:{}", app.owner))
}

/// What one run took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// Thread CPU time ([`THREAD_CPU`]), else wall time.
    pub cpu: Duration,
    pub wall: Duration,
}

/// Runs `f` and measures it on the thread it runs on.
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Run) {
    let (cpu_at, wall_at) = (thread_cpu(), Instant::now());
    let out = f();
    let wall = wall_at.elapsed();
    let cpu = match (cpu_at, thread_cpu()) {
        (Some(at), Some(now)) => now.saturating_sub(at),
        _ => wall,
    };
    (out, Run { cpu, wall })
}

#[cfg(unix)]
fn thread_cpu() -> Option<Duration> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` writes only the `timespec` it is handed, which
    // lives on this frame for the whole call.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts) };
    if rc != 0 {
        return None;
    }
    Some(Duration::new(
        u64::try_from(ts.tv_sec).ok()?,
        u32::try_from(ts.tv_nsec).ok()?,
    ))
}

#[cfg(not(unix))]
const fn thread_cpu() -> Option<Duration> {
    None
}

/// The linear memory `wasm` declares at instantiation, in bytes: every memory
/// of a core module, or of each core module a component nests. 0 for bytes
/// that do not parse.
#[must_use]
pub fn declared_memory(wasm: &[u8]) -> u64 {
    let mut bytes = 0_u64;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let reader = match payload {
            Ok(wasmparser::Payload::MemorySection(reader)) => reader,
            Ok(_) => continue,
            Err(_) => break,
        };
        for memory in reader.into_iter().flatten() {
            let page = memory
                .page_size_log2
                .map_or(WASM_PAGE_BYTES, |log2| 1_u64 << log2.min(16));
            bytes = bytes.saturating_add(memory.initial.saturating_mul(page));
        }
    }
    bytes
}

/// Where a run is billed: the app, its payer and the bundle it runs.
#[derive(Clone, Debug, Default)]
pub struct Meter {
    pub app: String,
    pub payer: String,
    pub bundle: String,
}

/// One app's counters on this node.
#[derive(Default)]
struct AppUsage {
    payer: String,
    calls: [u64; Kind::ALL.len()],
    cpu: Duration,
    /// Linear memory bytes × wall nanoseconds.
    memory: u128,
    /// Requests its runs handed the outbound transport.
    outbound: Arc<AtomicU64>,
    /// The last sample: (state bytes, blob bytes).
    stored: Option<(u64, u64)>,
}

/// Every app's usage on this node, in memory.
#[derive(Default)]
pub struct Usage {
    apps: Mutex<HashMap<String, AppUsage>>,
    /// The declared linear memory per bundle hash ([`declared_memory`]).
    memories: Mutex<HashMap<String, u64>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Usage {
    /// `app`'s counters under `payer`. A payer change (a transferred personal
    /// app) starts a new series from zero, so the new payer is billed only
    /// for what follows.
    fn entry<'a>(
        apps: &'a mut HashMap<String, AppUsage>,
        app: &str,
        payer: &str,
    ) -> &'a mut AppUsage {
        let usage = apps.entry(app.to_owned()).or_default();
        if usage.payer != payer {
            *usage = AppUsage {
                payer: payer.to_owned(),
                ..AppUsage::default()
            };
        }
        usage
    }

    /// Note the memory `bundle` declares, once per bundle.
    pub fn learn(&self, bundle: &str, wasm: &[u8]) {
        if lock(&self.memories).contains_key(bundle) {
            return;
        }
        let bytes = declared_memory(wasm);
        let _prev = lock(&self.memories).insert(bundle.to_owned(), bytes);
    }

    /// Add a run's CPU and memory.
    pub fn ran(&self, meter: &Meter, run: Run) {
        let memory = lock(&self.memories)
            .get(&meter.bundle)
            .copied()
            .unwrap_or(0);
        let mut apps = lock(&self.apps);
        let usage = Self::entry(&mut apps, &meter.app, &meter.payer);
        usage.cpu = usage.cpu.saturating_add(run.cpu);
        usage.memory = usage
            .memory
            .saturating_add(u128::from(memory).saturating_mul(run.wall.as_nanos()));
    }

    /// Count a call whose answer stood.
    pub fn called(&self, meter: &Meter, kind: Kind) {
        let mut apps = lock(&self.apps);
        let usage = Self::entry(&mut apps, &meter.app, &meter.payer);
        let n = &mut usage.calls[kind as usize];
        *n = n.saturating_add(1);
    }

    /// The counter of `app`'s outbound requests, for its run's client.
    #[must_use]
    pub fn outbound(&self, app: &str, payer: &str) -> Arc<AtomicU64> {
        let mut apps = lock(&self.apps);
        Arc::clone(&Self::entry(&mut apps, app, payer).outbound)
    }

    /// Record a sample of `app`'s state and blob bytes.
    pub fn stored(&self, app: &str, payer: &str, state: u64, blobs: u64) {
        let mut apps = lock(&self.apps);
        Self::entry(&mut apps, app, payer).stored = Some((state, blobs));
    }

    /// Drop the series of every app `hosted` does not name.
    pub fn retain(&self, hosted: impl Fn(&str) -> bool) {
        lock(&self.apps).retain(|app, _| hosted(app));
    }

    /// Append the `calimero_app_*` families. A counter's series appears once
    /// it counted something, and the blob gauge once the app holds a blob, so
    /// an app costs as few series as it uses.
    pub fn render(&self, out: &mut String) {
        let apps = lock(&self.apps);
        let mut rows: Vec<(&String, &AppUsage)> = apps.iter().collect();
        rows.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let labels = |app: &str, u: &AppUsage| {
            format!("app=\"{}\",org=\"{}\"", escape(app), escape(&u.payer))
        };
        let clock = if THREAD_CPU {
            "thread CPU time"
        } else {
            "wall time"
        };
        let mut family = |name: &str, kind: &str, help: &str, samples: Vec<(String, String)>| {
            let _r = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
            for (labels, value) in samples {
                let _r = writeln!(out, "{name}{{{labels}}} {value}");
            }
        };
        family(
            "calimero_app_calls_total",
            "counter",
            "WASM calls this node ran for an app whose answer stood, by kind (mutation, view, followup).",
            rows.iter()
                .flat_map(|(app, u)| {
                    Kind::ALL.iter().filter_map(move |k| {
                        let n = u.calls[*k as usize];
                        (n > 0).then(|| {
                            (
                                format!("{},kind=\"{}\"", labels(app, u), k.label()),
                                n.to_string(),
                            )
                        })
                    })
                })
                .collect(),
        );
        family(
            "calimero_app_wasm_cpu_seconds_total",
            "counter",
            &format!("Seconds of {clock} this node's WASM runs for an app took."),
            rows.iter()
                .filter(|(_, u)| !u.cpu.is_zero())
                .map(|(app, u)| (labels(app, u), u.cpu.as_secs_f64().to_string()))
                .collect(),
        );
        family(
            "calimero_app_wasm_memory_byte_seconds_total",
            "counter",
            "Linear memory bytes times wall seconds of this node's WASM runs for an app (the memory each module declares, a floor of its peak).",
            rows.iter()
                .filter(|(_, u)| u.memory > 0)
                .map(|(app, u)| (labels(app, u), (u.memory as f64 / 1e9).to_string()))
                .collect(),
        );
        family(
            "calimero_app_outbound_requests_total",
            "counter",
            "Outbound HTTPS requests an app's runs on this node sent.",
            rows.iter()
                .filter_map(|(app, u)| {
                    let n = u.outbound.load(Ordering::Relaxed);
                    (n > 0).then(|| (labels(app, u), n.to_string()))
                })
                .collect(),
        );
        family(
            "calimero_app_state_bytes",
            "gauge",
            "Key and value bytes of an app's state on this replica, sampled.",
            rows.iter()
                .filter_map(|(app, u)| u.stored.map(|(s, _)| (labels(app, u), s.to_string())))
                .collect(),
        );
        family(
            "calimero_app_blob_bytes",
            "gauge",
            "Bytes of the blobs an app holds, on this replica, sampled.",
            rows.iter()
                .filter_map(|(app, u)| {
                    u.stored
                        .filter(|(_, b)| *b > 0)
                        .map(|(_, b)| (labels(app, u), b.to_string()))
                })
                .collect(),
        );
    }
}

/// A label value as Prometheus text writes it. Ids need none of this; it keeps
/// a malformed one from breaking the exposition.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter(app: &str, payer: &str) -> Meter {
        Meter {
            app: app.to_owned(),
            payer: payer.to_owned(),
            bundle: "b".to_owned(),
        }
    }

    fn rendered(u: &Usage) -> String {
        let mut out = String::new();
        u.render(&mut out);
        out
    }

    /// A core module with one memory of `pages` initial pages, by hand.
    fn module(pages: u8) -> Vec<u8> {
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        // Memory section (5): 3 bytes, one memory, no maximum, `pages` initial.
        m.extend([5, 3, 1, 0, pages]);
        m
    }

    #[test]
    fn a_modules_declared_memory_is_its_initial_pages() {
        assert_eq!(declared_memory(&module(3)), 3 * 65_536);
        assert_eq!(declared_memory(&module(0)), 0);
        assert_eq!(declared_memory(b"not wasm"), 0);
    }

    #[test]
    fn thread_cpu_time_counts_work_not_sleep() {
        let ((), slept) = measure(|| std::thread::sleep(Duration::from_millis(50)));
        assert!(slept.wall >= Duration::from_millis(50));
        if THREAD_CPU {
            assert!(slept.cpu < Duration::from_millis(25), "{slept:?}");
        }
        // 300 ms of spinning, of which 15 ms must count: a loaded runner
        // can deschedule the thread for most of a short window (CI saw 8 ms
        // of CPU in a 30 ms spin).
        let (_sum, busy) = measure(|| {
            let started = Instant::now();
            let mut x = 0_u64;
            while started.elapsed() < Duration::from_millis(300) {
                x = std::hint::black_box(x.wrapping_add(1));
            }
            x
        });
        assert!(busy.cpu >= Duration::from_millis(15), "{busy:?}");
    }

    #[test]
    fn runs_and_calls_render_as_counters_labelled_by_app_and_payer() {
        let u = Usage::default();
        u.learn("b", &module(2));
        let m = meter("app1", "org1");
        u.ran(
            &m,
            Run {
                cpu: Duration::from_millis(250),
                wall: Duration::from_secs(2),
            },
        );
        u.called(&m, Kind::Mutation);
        u.called(&m, Kind::Mutation);
        u.called(&m, Kind::View);
        let text = rendered(&u);
        for line in [
            "calimero_app_calls_total{app=\"app1\",org=\"org1\",kind=\"mutation\"} 2",
            "calimero_app_calls_total{app=\"app1\",org=\"org1\",kind=\"view\"} 1",
            "calimero_app_wasm_cpu_seconds_total{app=\"app1\",org=\"org1\"} 0.25",
            // 2 pages × 2 s.
            "calimero_app_wasm_memory_byte_seconds_total{app=\"app1\",org=\"org1\"} 262144",
            "# TYPE calimero_app_state_bytes gauge",
        ] {
            assert!(text.lines().any(|l| l == line), "no `{line}` in\n{text}");
        }
        assert!(!text.contains("kind=\"followup\""), "{text}");
        assert!(
            !text.contains("calimero_app_outbound_requests_total{"),
            "{text}"
        );
        assert!(
            !text.contains("calimero_app_state_bytes{"),
            "not sampled yet"
        );
    }

    #[test]
    fn outbound_and_storage_series_appear_once_they_count() {
        let u = Usage::default();
        let sent = u.outbound("app1", "account:alice");
        let _prev = sent.fetch_add(3, Ordering::Relaxed);
        u.stored("app1", "account:alice", 120, 0);
        u.stored("app2", "org2", 10, 4096);
        let text = rendered(&u);
        for line in [
            "calimero_app_outbound_requests_total{app=\"app1\",org=\"account:alice\"} 3",
            "calimero_app_state_bytes{app=\"app1\",org=\"account:alice\"} 120",
            "calimero_app_state_bytes{app=\"app2\",org=\"org2\"} 10",
            "calimero_app_blob_bytes{app=\"app2\",org=\"org2\"} 4096",
        ] {
            assert!(text.lines().any(|l| l == line), "no `{line}` in\n{text}");
        }
        assert!(
            !text.contains("calimero_app_blob_bytes{app=\"app1\""),
            "{text}"
        );
    }

    #[test]
    fn an_app_that_left_the_node_loses_its_series() {
        let u = Usage::default();
        u.called(&meter("stays", "o"), Kind::View);
        u.called(&meter("gone", "o"), Kind::View);
        u.retain(|app| app == "stays");
        let text = rendered(&u);
        assert!(text.contains("app=\"stays\""), "{text}");
        assert!(!text.contains("app=\"gone\""), "{text}");
    }

    #[test]
    fn a_new_payer_starts_from_zero() {
        let u = Usage::default();
        u.called(&meter("app1", "account:alice"), Kind::Mutation);
        u.called(&meter("app1", "account:bob"), Kind::Mutation);
        let text = rendered(&u);
        assert!(!text.contains("account:alice"), "{text}");
        assert!(text.contains(
            "calimero_app_calls_total{app=\"app1\",org=\"account:bob\",kind=\"mutation\"} 1"
        ));
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
