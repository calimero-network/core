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
    /// group → request body bytes, as each body's length says (capped).
    request_bytes: BTreeMap<&'static str, u64>,
    /// group → response body bytes, where the length was known up front.
    response_bytes: BTreeMap<&'static str, u64>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

/// When this process started, for `process_start_time_seconds`: from the
/// kernel on Linux, else when the node started serving.
static STARTED: LazyLock<f64> = LazyLock::new(|| {
    #[cfg(target_os = "linux")]
    if let Some(at) = linux_start_time() {
        return at;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
});

/// The process's start in Unix seconds: boot time (`btime` in `/proc/stat`)
/// plus its `starttime` ticks after boot (`/proc/self/stat`, field 22).
#[cfg(target_os = "linux")]
fn linux_start_time() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let boot = std::fs::read_to_string("/proc/stat").ok()?;
    start_time(&stat, &boot)
}

#[cfg(target_os = "linux")]
fn start_time(stat: &str, boot: &str) -> Option<f64> {
    let (_, rest) = stat.rsplit_once(')')?;
    let ticks: u64 = rest.split_whitespace().nth(19)?.parse().ok()?;
    let btime: u64 = boot
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(btime as f64 + ticks as f64 / USER_HZ)
}

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read the process's start time; called once when the node starts serving.
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
        _ if route.starts_with("/auth/passkey/") || route == "/auth/refresh" => "sign_in",
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

/// A body length fit to count: what the body says it holds, never more than
/// the sealed cap, which no request or answer of the node's passes. A caller's
/// `Content-Length` on an unsealed route is its own claim.
fn body_bytes(exact: Option<u64>) -> u64 {
    let cap = calimero_sealed::server::MAX_SEALED_BYTES as u64;
    exact.unwrap_or(0).min(cap)
}

/// One request in flight: counted as `cancelled` if its future is dropped
/// before the handler answers (a client gone, a stream reset, the sealed
/// connection closed), so the gauge never leaks and the request is counted.
struct InFlight {
    group: &'static str,
    key: Option<(String, String)>,
    started: Instant,
}

impl InFlight {
    fn start(group: &'static str, method: String, route: String) -> Self {
        *registry().in_flight.entry(group).or_default() += 1;
        Self {
            group,
            key: Some((method, route)),
            started: Instant::now(),
        }
    }

    /// The handler answered `response`.
    fn answered(mut self, response: &Response) {
        let Some((method, route)) = self.key.take() else {
            return;
        };
        let status = response.status().as_u16();
        // A refusal no `ApiError` built (an extractor's, the body limit's)
        // carries no code.
        let code = response
            .extensions()
            .get::<ErrorCode>()
            .map(|c| code_label(&c.0))
            .or_else(|| (status >= 400).then(|| "OTHER".to_owned()));
        let sent = body_bytes(response.body().size_hint().exact());
        let mut r = registry();
        r.finish((method, route, status_class(status)), self.started);
        if let Some(code) = code {
            let n = r.errors.entry((self.group, code)).or_default();
            *n = n.saturating_add(1);
        }
        let n = r.response_bytes.entry(self.group).or_default();
        *n = n.saturating_add(sent);
        // Release the registry before `self` drops, which takes it again.
        drop(r);
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut r = registry();
        if let Some((method, route)) = self.key.take() {
            r.finish((method, route, "cancelled"), self.started);
        }
        *r.in_flight.entry(self.group).or_default() -= 1;
    }
}

impl Registry {
    fn finish(&mut self, key: HttpKey, started: Instant) {
        let n = self.requests.entry(key.clone()).or_default();
        *n = n.saturating_add(1);
        self.durations
            .entry(key)
            .or_default()
            .observe(started.elapsed().as_secs_f64());
    }
}

/// The layer: count, time and size every request the router matched. It is a
/// `route_layer`, so a request no route matched (404, 405) is not counted, nor
/// is what is answered before the router: the sealed layer's own refusals and
/// handshakes, and app frontends (`app_host`).
pub async fn track(route: MatchedPath, request: Request, next: Next) -> Response {
    let route = route.as_str().to_owned();
    let group = group(&route);
    let declared = body_bytes(request.body().size_hint().exact());
    {
        let mut r = registry();
        let n = r.request_bytes.entry(group).or_default();
        *n = n.saturating_add(declared);
    }
    let flight = InFlight::start(group, request.method().as_str().to_owned(), route);
    let response = next.run(request).await;
    flight.answered(&response);
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
        "calimero_group_quorum_ack_age_seconds",
        "On the group's leader: seconds since a quorum last acknowledged it.",
    );
    for s in samples {
        if let Some(ms) = s.quorum_ack_ms {
            let _w = writeln!(
                out,
                "calimero_group_quorum_ack_age_seconds{{group=\"{}\"}} {}",
                s.group,
                ms as f64 / 1000.0
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

/// How long a disk sample serves scrapes: v1 sampled every 30 s too.
const DISK_SAMPLE_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Per data directory: the last `(datastore, snapshots)` disk sample and when
/// it was taken.
type DiskSamples = BTreeMap<std::path::PathBuf, (Instant, (u64, u64))>;
static DISK: Mutex<DiskSamples> = Mutex::new(BTreeMap::new());

/// `(datastore, snapshots)` disk use under `data_dir`, walked on a blocking
/// thread at most once per [`DISK_SAMPLE_EVERY`], never on an async worker.
pub async fn disk_sample(data_dir: &std::path::Path) -> (u64, u64) {
    let cached = DISK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(data_dir)
        .copied();
    if let Some((at, sample)) = cached {
        if at.elapsed() < DISK_SAMPLE_EVERY {
            return sample;
        }
    }
    let dir = data_dir.to_owned();
    let sample = tokio::task::spawn_blocking(move || {
        (
            disk_usage(&dir.join("store"), 1),
            disk_usage(&dir.join("snapshots"), 2),
        )
    })
    .await
    .unwrap_or_default();
    let _old = DISK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(data_dir.to_owned(), (Instant::now(), sample));
    sample
}

/// v1's node gauges: `merod_build_info` (`peer_id` is the node id; the node has
/// no libp2p identity) and `storage_disk_usage_bytes` per store
/// (`datastore` is the sealed RocksDB store: log, state and blobs), from
/// [`disk_sample`].
pub fn render_node(out: &mut String, node_id: u64, (datastore, snapshots): (u64, u64)) {
    let _w = writeln!(
        out,
        "# HELP merod_build_info Always 1, labelled with this node's version and id.\n# TYPE merod_build_info gauge\nmerod_build_info{{version=\"{}\",peer_id=\"{node_id}\"}} 1",
        env!("CALIMERO_RELEASE_VERSION")
    );
    let _w = writeln!(
        out,
        "# HELP storage_disk_usage_bytes Disk this node's stores take (allocated blocks), by store.\n# TYPE storage_disk_usage_bytes gauge\nstorage_disk_usage_bytes{{store=\"datastore\"}} {datastore}\nstorage_disk_usage_bytes{{store=\"snapshots\"}} {snapshots}",
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

    /// The router's route templates, read from its source.
    fn router_templates() -> Vec<&'static str> {
        let source = include_str!("api.rs");
        let router = &source[source.find("pub fn router(").unwrap()..];
        router
            .split(".route(")
            .skip(1)
            .filter_map(|r| r.trim_start().strip_prefix('"')?.split('"').next())
            .collect()
    }

    #[test]
    fn every_route_the_router_serves_has_a_group() {
        let templates = router_templates();
        assert!(templates.len() > 50, "{templates:?}");
        let other: Vec<_> = templates.iter().filter(|t| group(t) == "other").collect();
        assert!(other.is_empty(), "routes without a group: {other:?}");
    }

    #[test]
    fn a_body_counts_at_most_the_sealed_cap() {
        let cap = calimero_sealed::server::MAX_SEALED_BYTES as u64;
        assert_eq!(body_bytes(Some(u64::MAX)), cap);
        assert_eq!(body_bytes(Some(12)), 12);
        assert_eq!(body_bytes(None), 0);
    }

    #[test]
    fn every_route_template_has_a_group_and_ids_never_become_labels() {
        assert_eq!(group("/apps/{app}/intents"), "calls");
        assert_eq!(group("/apps/{app}/views"), "views");
        assert_eq!(group("/apps/{app}/presence/stream"), "streams");
        assert_eq!(group("/apps/{app}/blobs/{id}/info"), "blobs");
        assert_eq!(group("/auth/passkey/login/finish"), "sign_in");
        assert_eq!(group("/auth/passkeys/add/begin"), "account");
        assert_eq!(group("/auth/passkeys/{cred}"), "account");
        assert_eq!(group("/auth/apikeys/{key}"), "account");
        assert_eq!(group("/orgs/{org}/teams/{team}"), "orgs");
        assert_eq!(group("/invites/{token}/redeem"), "orgs");
        assert_eq!(group("/internal/raft"), "raft");
        assert_eq!(group("/internal/system/index"), "internal");
        assert_eq!(group("/ops/metrics"), "ops");
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

    #[cfg(target_os = "linux")]
    #[test]
    fn the_start_time_is_boot_time_plus_the_processs_start_ticks() {
        let stat = "42 (a) b) S 1 42 42 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 8 0 250 0";
        assert_eq!(start_time(stat, "cpu 1 2\nbtime 1000\n"), Some(1002.5));
        assert_eq!(start_time(stat, "cpu 1 2\n"), None);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let own = linux_start_time().unwrap();
        assert!(own <= now + 1.0 && own > now - 86_400.0 * 365.0, "{own}");
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

    fn tracked(route: &str, handler: axum::routing::MethodRouter) -> axum::Router {
        axum::Router::new()
            .route(route, handler)
            .route_layer(axum::middleware::from_fn(track))
    }

    #[tokio::test]
    async fn a_declared_length_can_neither_overflow_nor_wrap_the_byte_counters() {
        use tower::ServiceExt as _;
        let app = tracked("/health", axum::routing::get(|| async { "ok" }));
        for _ in 0..3 {
            let request = Request::builder()
                .uri("/health")
                .header(axum::http::header::CONTENT_LENGTH, i64::MAX.to_string())
                .body(axum::body::Body::empty())
                .unwrap();
            let answer = app.clone().oneshot(request).await.unwrap();
            assert_eq!(answer.status(), 200);
        }
        let r = registry();
        let bytes = r.request_bytes.get("health").copied().unwrap_or(0);
        assert!(
            bytes <= 3 * calimero_sealed::server::MAX_SEALED_BYTES as u64,
            "{bytes}"
        );
        assert_eq!(r.in_flight.get("health").copied(), Some(0));
    }

    #[tokio::test]
    async fn a_dropped_request_leaves_nothing_in_flight_and_counts_as_cancelled() {
        use tower::ServiceExt as _;
        let app = tracked(
            "/attestation",
            axum::routing::get(std::future::pending::<&'static str>),
        );
        let request = Request::builder()
            .uri("/attestation")
            .body(axum::body::Body::empty())
            .unwrap();
        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(20), app.oneshot(request)).await;
        assert!(waited.is_err(), "the handler never answers");
        let r = registry();
        assert_eq!(r.in_flight.get("attestation").copied(), Some(0));
        let key = ("GET".to_owned(), "/attestation".to_owned(), "cancelled");
        assert_eq!(r.requests.get(&key).copied(), Some(1));
        assert_eq!(r.durations.get(&key).map(|h| h.count), Some(1));
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
