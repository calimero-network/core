//! Request metrics for `/ops/metrics`: counts, errors, latency and bytes per
//! route, gathered by one layer on the API router ([`track`]). The sealed
//! envelope routes every opened request through that router again, so the
//! layer sees each request once, with the status the handler answered.
//!
//! Labels are route templates (`/apps/{app}/intents`), route groups, status
//! classes and error codes: never an id, a name or anything from a request's
//! body. Whoever scrapes the metrics is the operator, who must learn no
//! more than counts.
//!
//! Where v1's merod exported the same measure, the name and labels are v1's
//! (`http_requests_total`, `http_request_duration_seconds`, `merod_build_info`,
//! `process_*`, `storage_disk_usage_bytes`), so dashboards built on them keep
//! working; what v1 had no equivalent for is `calimero_*`.
//!
//! The registry is process-wide: in a test that runs several nodes in one
//! process, each node's `/ops/metrics` shows the sum of all of them.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::body::HttpBody as _;
use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;

/// Upper bounds, in seconds, of the request duration histogram's buckets: 1 ms
/// doubling to about 16 s, v1's `exponential_buckets(0.001, 2.0, 15)`.
const DURATION_BUCKETS: [f64; 15] = [
    0.001, 0.002, 0.004, 0.008, 0.016, 0.032, 0.064, 0.128, 0.256, 0.512, 1.024, 2.048, 4.096,
    8.192, 16.384,
];

/// `(method, route template, status class)`: v1's `HttpLabels`.
type HttpKey = (String, String, &'static str);

/// The error code an [`crate::error::ApiError`] answered with, carried on its
/// response for [`track`].
#[derive(Clone, Debug)]
pub struct ErrorCode(pub String);

#[derive(Default)]
struct Histogram {
    buckets: [u64; DURATION_BUCKETS.len()],
    count: u64,
    sum: f64,
}

impl Histogram {
    fn observe(&mut self, secs: f64) {
        for (bucket, le) in self.buckets.iter_mut().zip(DURATION_BUCKETS) {
            if secs <= le {
                *bucket += 1;
            }
        }
        self.count += 1;
        self.sum += secs;
    }
}

#[derive(Default)]
struct Registry {
    /// Requests answered.
    requests: BTreeMap<HttpKey, u64>,
    /// `(group, code)` → error answers.
    errors: BTreeMap<(&'static str, String), u64>,
    /// Time to the response's head.
    durations: BTreeMap<HttpKey, Histogram>,
    /// group → requests being handled now.
    in_flight: BTreeMap<&'static str, i64>,
    /// group → request body bytes, as each request declared them.
    request_bytes: BTreeMap<&'static str, u64>,
    /// group → response body bytes, where the length was known up front.
    response_bytes: BTreeMap<&'static str, u64>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

/// When this process started serving, for `process_start_time_seconds`.
static STARTED: LazyLock<f64> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
});

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Record the process's start; called once when the node starts serving.
pub fn mark_started() {
    let _ = *STARTED;
}

/// A route template's group: what a dashboard plots side by side.
#[must_use]
pub fn group(route: &str) -> &'static str {
    let in_app = |tail: &str| {
        route
            .strip_prefix("/apps/{app}")
            .is_some_and(|r| r.starts_with(tail))
    };
    match route {
        "/health" => "health",
        "/attestation" => "attestation",
        "/internal/raft" => "raft",
        "/apps/{app}/intents" => "calls",
        "/apps/{app}/views" => "views",
        _ if route.starts_with("/auth/passkey") || route == "/auth/refresh" => "sign_in",
        _ if route.starts_with("/auth") => "account",
        _ if in_app("/events") || in_app("/presence") => "streams",
        _ if in_app("/blobs") => "blobs",
        _ if route.starts_with("/apps") => "apps",
        _ if route.starts_with("/orgs") || route.starts_with("/invites") => "orgs",
        _ if route.starts_with("/bundles") || route.starts_with("/packages") => "packages",
        _ if route.starts_with("/ops") => "ops",
        _ if route.starts_with("/internal") => "internal",
        _ => "other",
    }
}

/// An error code fit to be a label: the node's own codes are upper-case
/// words; anything else is counted as `OTHER`.
fn code_label(code: &str) -> String {
    let fits = !code.is_empty()
        && code.len() <= 40
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if fits {
        code.to_owned()
    } else {
        "OTHER".to_owned()
    }
}

/// A status as v1 labelled it: its class (`2xx`, `4xx`, ...), not its code.
fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "unknown",
    }
}

/// The layer: count, time and size every request the router matched.
pub async fn track(route: Option<MatchedPath>, request: Request, next: Next) -> Response {
    let route = route.map_or_else(|| "unmatched".to_owned(), |r| r.as_str().to_owned());
    let group = group(&route);
    let method = request.method().as_str().to_owned();
    let declared = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    {
        let mut r = registry();
        *r.in_flight.entry(group).or_default() += 1;
        *r.request_bytes.entry(group).or_default() += declared;
    }
    let started = Instant::now();
    let response = next.run(request).await;
    let took = started.elapsed().as_secs_f64();
    let key = (method, route, status_class(response.status().as_u16()));
    let code = response
        .extensions()
        .get::<ErrorCode>()
        .map(|c| code_label(&c.0));
    let sent = response.body().size_hint().exact().unwrap_or(0);
    let mut r = registry();
    *r.in_flight.entry(group).or_default() -= 1;
    *r.requests.entry(key.clone()).or_default() += 1;
    if let Some(code) = code {
        *r.errors.entry((group, code)).or_default() += 1;
    }
    r.durations.entry(key).or_default().observe(took);
    *r.response_bytes.entry(group).or_default() += sent;
    response
}

/// The request metrics in Prometheus' text format, to follow the node's own.
pub fn render(out: &mut String) {
    let r = registry();
    let _w = writeln!(
        out,
        "# HELP process_start_time_seconds When this node's process started, in Unix seconds.\n# TYPE process_start_time_seconds gauge\nprocess_start_time_seconds {}",
        *STARTED
    );
    let _w = writeln!(
        out,
        "# HELP http_requests_total Requests answered, by method, route template and status class.\n# TYPE http_requests_total counter"
    );
    for ((method, path, status), n) in &r.requests {
        let _w = writeln!(
            out,
            "http_requests_total{{method=\"{method}\",path=\"{path}\",status=\"{status}\",group=\"{}\"}} {n}",
            group(path)
        );
    }
    let _w = writeln!(
        out,
        "# HELP calimero_http_errors_total Error answers, by route group and the node's error code.\n# TYPE calimero_http_errors_total counter"
    );
    for ((group, code), n) in &r.errors {
        let _w = writeln!(
            out,
            "calimero_http_errors_total{{group=\"{group}\",code=\"{code}\"}} {n}"
        );
    }
    let _w = writeln!(
        out,
        "# HELP http_request_duration_seconds Time to a response's head, by method, route template and status class.\n# TYPE http_request_duration_seconds histogram"
    );
    for ((method, path, status), h) in &r.durations {
        let labels = format!(
            "method=\"{method}\",path=\"{path}\",status=\"{status}\",group=\"{}\"",
            group(path)
        );
        for (le, n) in DURATION_BUCKETS.iter().zip(h.buckets) {
            let _w = writeln!(
                out,
                "http_request_duration_seconds_bucket{{{labels},le=\"{le}\"}} {n}"
            );
        }
        let _w = writeln!(
            out,
            "http_request_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}\nhttp_request_duration_seconds_sum{{{labels}}} {}\nhttp_request_duration_seconds_count{{{labels}}} {}",
            h.count, h.sum, h.count
        );
    }
    let _w = writeln!(
        out,
        "# HELP calimero_http_requests_in_flight Requests being handled now, by route group.\n# TYPE calimero_http_requests_in_flight gauge"
    );
    for (group, n) in &r.in_flight {
        let _w = writeln!(
            out,
            "calimero_http_requests_in_flight{{group=\"{group}\"}} {n}"
        );
    }
    let _w = writeln!(
        out,
        "# HELP calimero_http_request_bytes_total Request body bytes declared, by route group.\n# TYPE calimero_http_request_bytes_total counter"
    );
    for (group, n) in &r.request_bytes {
        let _w = writeln!(
            out,
            "calimero_http_request_bytes_total{{group=\"{group}\"}} {n}"
        );
    }
    let _w = writeln!(
        out,
        "# HELP calimero_http_response_bytes_total Response body bytes of a known length, by route group.\n# TYPE calimero_http_response_bytes_total counter"
    );
    for (group, n) in &r.response_bytes {
        let _w = writeln!(
            out,
            "calimero_http_response_bytes_total{{group=\"{group}\"}} {n}"
        );
    }
}

/// One Raft group as this node sees it, for the `calimero_group_*` gauges.
pub struct RaftSample {
    /// `system`, or the app group's number (never the app's id).
    pub group: String,
    pub term: u64,
    pub leader: bool,
    /// Entries in this node's log it has not applied yet.
    pub apply_lag: u64,
    /// Since a quorum last acknowledged this leader, in milliseconds.
    pub quorum_ack_ms: Option<u64>,
    /// On a leader: each follower's distance behind this node's log.
    pub follower_lag: Vec<(u64, u64)>,
    /// The index this node's latest snapshot covers.
    pub snapshot: Option<u64>,
}

/// Read `g`'s Raft metrics.
pub fn raft_sample<M: calimero_log::Machine>(
    group: String,
    g: &calimero_log::Group<M>,
) -> RaftSample {
    let status = g.status();
    let m = g.raft().metrics().borrow().clone();
    let last = m.last_log_index.unwrap_or(0);
    let applied = m.last_applied.map_or(0, |l| l.index);
    let follower_lag = m
        .replication
        .iter()
        .flatten()
        .filter(|(peer, _)| Some(**peer) != status.leader)
        .map(|(peer, matched)| (*peer, last.saturating_sub(matched.map_or(0, |l| l.index))))
        .collect();
    RaftSample {
        group,
        term: m.current_term,
        leader: status.is_leader,
        apply_lag: last.saturating_sub(applied),
        quorum_ack_ms: if status.is_leader {
            m.millis_since_quorum_ack
        } else {
            None
        },
        follower_lag,
        snapshot: m.snapshot.map(|l| l.index),
    }
}

/// The `calimero_group_*` Raft gauges for `samples`.
pub fn render_raft(out: &mut String, samples: &[RaftSample]) {
    let family = |out: &mut String, name: &str, help: &str| {
        let _w = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
    };
    family(
        out,
        "calimero_group_term",
        "The group's Raft term here; a change is an election.",
    );
    for s in samples {
        let _w = writeln!(
            out,
            "calimero_group_term{{group=\"{}\"}} {}",
            s.group, s.term
        );
    }
    family(
        out,
        "calimero_group_leader",
        "1 when this node leads the group.",
    );
    for s in samples {
        let _w = writeln!(
            out,
            "calimero_group_leader{{group=\"{}\"}} {}",
            s.group,
            u8::from(s.leader)
        );
    }
    family(
        out,
        "calimero_group_apply_lag",
        "Entries in this node's log of the group not applied yet.",
    );
    for s in samples {
        let _w = writeln!(
            out,
            "calimero_group_apply_lag{{group=\"{}\"}} {}",
            s.group, s.apply_lag
        );
    }
    family(
        out,
        "calimero_group_follower_lag",
        "On the group's leader: entries a follower is behind.",
    );
    for s in samples {
        for (peer, lag) in &s.follower_lag {
            let _w = writeln!(
                out,
                "calimero_group_follower_lag{{group=\"{}\",peer=\"{peer}\"}} {lag}",
                s.group
            );
        }
    }
    family(
        out,
        "calimero_group_quorum_ack_age_ms",
        "On the group's leader: since a quorum last acknowledged it.",
    );
    for s in samples {
        if let Some(ms) = s.quorum_ack_ms {
            let _w = writeln!(
                out,
                "calimero_group_quorum_ack_age_ms{{group=\"{}\"}} {ms}",
                s.group
            );
        }
    }
    family(
        out,
        "calimero_group_snapshot_index",
        "The index this node's latest snapshot of the group covers.",
    );
    for s in samples {
        if let Some(i) = s.snapshot {
            let _w = writeln!(
                out,
                "calimero_group_snapshot_index{{group=\"{}\"}} {i}",
                s.group
            );
        }
    }
}

/// Disk the files under `path` take, at most `depth` directories down: their
/// allocated blocks, as `du` counts (and v1's `storage_disk_usage_bytes`
/// did); 0 when it does not exist.
#[must_use]
pub fn disk_usage(path: &std::path::Path, depth: u8) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_file() => allocated(&m),
            Ok(m) if m.is_dir() && depth > 0 => disk_usage(&e.path(), depth - 1),
            _ => 0,
        })
        .sum()
}

#[cfg(unix)]
fn allocated(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    m.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated(m: &std::fs::Metadata) -> u64 {
    m.len()
}

/// v1's node gauges: `merod_build_info` (`peer_id` is the node id; release-v2
/// has no libp2p identity) and `storage_disk_usage_bytes` per store
/// (`datastore` is the sealed RocksDB store: log, state and blobs).
pub fn render_node(out: &mut String, node_id: u64, data_dir: &std::path::Path) {
    let _w = writeln!(
        out,
        "# HELP merod_build_info Always 1, labelled with this node's version and id.\n# TYPE merod_build_info gauge\nmerod_build_info{{version=\"{}\",peer_id=\"{node_id}\"}} 1",
        env!("CALIMERO_RELEASE_VERSION")
    );
    let _w = writeln!(
        out,
        "# HELP storage_disk_usage_bytes Disk this node's stores take (allocated blocks), by store.\n# TYPE storage_disk_usage_bytes gauge\nstorage_disk_usage_bytes{{store=\"datastore\"}} {}\nstorage_disk_usage_bytes{{store=\"snapshots\"}} {}",
        disk_usage(&data_dir.join("store"), 1),
        disk_usage(&data_dir.join("snapshots"), 2),
    );
}

/// Clock ticks per second in `/proc/self/stat` (`USER_HZ`, 100 on Linux).
#[cfg(target_os = "linux")]
const USER_HZ: f64 = 100.0;

/// User plus system CPU seconds from a `/proc/self/stat` line.
#[cfg(target_os = "linux")]
fn parse_cpu_seconds(stat: &str) -> Option<f64> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace().skip(11);
    let utime: u64 = fields.next()?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime.saturating_add(stime) as f64 / USER_HZ)
}

/// v1's `process_*` gauges, read from `/proc/self` at each scrape (Linux
/// only; elsewhere only `process_start_time_seconds`, from [`render`]).
pub fn render_process(out: &mut String) {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
        };
        let gauge = |out: &mut String, name: &str, help: &str, value: Option<u64>| {
            if let Some(value) = value {
                let _w = writeln!(
                    out,
                    "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}"
                );
            }
        };
        gauge(
            out,
            "process_resident_memory_bytes",
            "Resident memory, in bytes.",
            field("VmRSS:").map(|kb| kb.saturating_mul(1024)),
        );
        gauge(
            out,
            "process_virtual_memory_bytes",
            "Virtual memory, in bytes.",
            field("VmSize:").map(|kb| kb.saturating_mul(1024)),
        );
        gauge(
            out,
            "process_threads",
            "Threads of this process.",
            field("Threads:"),
        );
        // `read_dir` holds one descriptor of its own on the directory it lists.
        let fds = std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|d| (d.flatten().count() as u64).saturating_sub(1));
        gauge(out, "process_open_fds", "Open file descriptors.", fds);
        if let Some(cpu) = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .as_deref()
            .and_then(parse_cpu_seconds)
        {
            let _w = writeln!(
                out,
                "# HELP process_cpu_seconds_total User and system CPU time, in seconds.\n# TYPE process_cpu_seconds_total counter\nprocess_cpu_seconds_total {cpu}"
            );
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_template_has_a_group_and_ids_never_become_labels() {
        assert_eq!(group("/apps/{app}/intents"), "calls");
        assert_eq!(group("/apps/{app}/views"), "views");
        assert_eq!(group("/apps/{app}/presence/stream"), "streams");
        assert_eq!(group("/apps/{app}/blobs/{id}/info"), "blobs");
        assert_eq!(group("/auth/passkey/login/finish"), "sign_in");
        assert_eq!(group("/auth/apikeys/{key}"), "account");
        assert_eq!(group("/orgs/{org}/teams/{team}"), "orgs");
        assert_eq!(group("/invites/{token}/redeem"), "orgs");
        assert_eq!(group("/internal/raft"), "raft");
        assert_eq!(group("/internal/system/index"), "internal");
        assert_eq!(group("/ops/metrics"), "ops");
        assert_eq!(group("unmatched"), "other");
    }

    #[test]
    fn only_the_nodes_own_codes_become_labels() {
        assert_eq!(code_label("QUOTA_EXCEEDED"), "QUOTA_EXCEEDED");
        assert_eq!(code_label("TOO_LARGE"), "TOO_LARGE");
        assert_eq!(code_label("no such app"), "OTHER");
        assert_eq!(code_label(""), "OTHER");
        assert_eq!(code_label(&"A".repeat(41)), "OTHER");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cpu_seconds_sum_utime_and_stime_whatever_the_command_name() {
        let stat = "42 (merod) S 1 42 42 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 8 0";
        assert_eq!(parse_cpu_seconds(stat), Some(3.0));
        let odd = "42 (a) b (c) S 1 42 42 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 8 0";
        assert_eq!(parse_cpu_seconds(odd), Some(0.1));
        assert_eq!(parse_cpu_seconds("42 (merod) S 1 42"), None);
        let own = std::fs::read_to_string("/proc/self/stat").unwrap();
        assert!(parse_cpu_seconds(&own).is_some());
    }

    #[test]
    fn statuses_are_labelled_by_class() {
        assert_eq!(status_class(200), "2xx");
        assert_eq!(status_class(404), "4xx");
        assert_eq!(status_class(503), "5xx");
        assert_eq!(status_class(42), "unknown");
    }

    #[test]
    fn disk_usage_counts_nested_files_by_their_blocks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), vec![1_u8; 64 * 1024]).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), vec![1_u8; 64 * 1024]).unwrap();
        assert!(disk_usage(dir.path(), 1) >= 128 * 1024);
        assert!(disk_usage(dir.path(), 0) < 128 * 1024, "depth 0 skips sub/");
        assert_eq!(disk_usage(&dir.path().join("missing"), 1), 0);
    }

    #[test]
    fn a_histogram_counts_each_observation_in_every_bucket_at_or_above_it() {
        let mut h = Histogram::default();
        h.observe(0.003);
        h.observe(0.5);
        h.observe(60.0);
        assert_eq!(h.buckets.first(), Some(&0)); // ≤ 1 ms
        assert_eq!(h.buckets.get(2), Some(&1)); // ≤ 4 ms
        assert_eq!(h.buckets.get(9), Some(&2)); // ≤ 512 ms
        assert_eq!(h.buckets.last(), Some(&2)); // ≤ 16 s: the 60 s one is only in +Inf
        assert_eq!(h.count, 3);
    }
}
