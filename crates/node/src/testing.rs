//! Test hooks (feature `testing`, never in a release build).

use std::sync::Mutex;
use std::time::Duration;

/// Nodes whose `/attestation` answers late, by public URL.
static SLOW_ATTESTATION: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

/// Make the node at `public_url` answer `/attestation` only after `delay`, as
/// a TDX node does while it fetches Intel's collateral, so a test sees peers
/// attest each other more slowly than a Raft RPC may take.
pub fn slow_attestation(public_url: &str, delay: Duration) {
    if let Ok(mut s) = SLOW_ATTESTATION.lock() {
        s.retain(|(u, _)| u != public_url);
        s.push((public_url.to_owned(), delay));
    }
}

/// The delay [`slow_attestation`] set for the node at `public_url`.
pub(crate) fn attestation_delay(public_url: &str) -> Option<Duration> {
    SLOW_ATTESTATION
        .lock()
        .ok()?
        .iter()
        .find(|(u, _)| u == public_url)
        .map(|(_, d)| *d)
}

/// Nodes that advertise an older cluster version than this build's, by public URL.
static ADVERTISED_VERSION: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Make the node at `public_url` advertise `version` as the newest cluster version it
/// supports (its `/health`, and its own count when it raises the logged version), as a
/// node still on that release does, so a test holds the cluster below a feature this
/// build brings. `None` lifts it. Set it in `Cluster::start_with`'s `configure`, before
/// the node starts, so the cluster is never raised past it.
pub fn advertise_cluster_version(public_url: &str, version: Option<&str>) {
    if let Ok(mut a) = ADVERTISED_VERSION.lock() {
        a.retain(|(u, _)| u != public_url);
        if let Some(v) = version {
            a.push((public_url.to_owned(), v.to_owned()));
        }
    }
}

/// The version [`advertise_cluster_version`] set for the node at `public_url`.
pub(crate) fn advertised_cluster_version(public_url: &str) -> Option<String> {
    ADVERTISED_VERSION
        .lock()
        .ok()?
        .iter()
        .find(|(u, _)| u == public_url)
        .map(|(_, v)| v.clone())
}

/// Nodes whose quotes take long, by public URL.
static SLOW_QUOTES: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

/// Make each quote of the node at `public_url` take `delay` on the thread that
/// makes it (`std::thread::sleep`), as a TDX quote blocks in the kernel for
/// about a second, so a test sees what a quote holds while it runs. Unlike
/// [`slow_attestation`], which waits asynchronously before the quote.
pub fn slow_quotes(public_url: &str, delay: Duration) {
    if let Ok(mut s) = SLOW_QUOTES.lock() {
        s.retain(|(u, _)| u != public_url);
        s.push((public_url.to_owned(), delay));
    }
}

/// The delay [`slow_quotes`] set for the node at `public_url`.
pub(crate) fn quote_delay(public_url: &str) -> Option<Duration> {
    SLOW_QUOTES
        .lock()
        .ok()?
        .iter()
        .find(|(u, _)| u == public_url)
        .map(|(_, d)| *d)
}

/// Nodes whose component compiles take long, by public URL.
static SLOW_COMPILES: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

/// Make each compile a package install or bundle upload runs on the node at
/// `public_url` take `delay` more on the thread that compiles it
/// (`std::thread::sleep`), as cranelift holds its thread for seconds on a large
/// (componentize-js) component, so a test sees what a compile holds while it runs.
pub fn slow_compiles(public_url: &str, delay: Duration) {
    if let Ok(mut s) = SLOW_COMPILES.lock() {
        s.retain(|(u, _)| u != public_url);
        s.push((public_url.to_owned(), delay));
    }
}

/// The delay [`slow_compiles`] set for the node at `public_url`.
pub(crate) fn compile_delay(public_url: &str) -> Option<Duration> {
    SLOW_COMPILES
        .lock()
        .ok()?
        .iter()
        .find(|(u, _)| u == public_url)
        .map(|(_, d)| *d)
}

/// Nodes whose shared HTTP client (peers, job nodes, attestation) has a test's timeout
/// instead of [`crate::node::SHARED_HTTP_TIMEOUT`], by data directory: a test's own
/// temporary directory, where a public URL's port can be handed to a later test's node.
static SHARED_HTTP_TIMEOUT: Mutex<Vec<(std::path::PathBuf, Duration)>> = Mutex::new(Vec::new());

/// Make the node whose data directory is `data_dir` give its shared HTTP client `timeout`,
/// so a test can show a transfer outlives it without waiting out the 30 s a cluster uses.
/// The client is built when the node starts: set it in `Cluster::start_with`'s `configure`.
pub fn shared_http_timeout(data_dir: &std::path::Path, timeout: Duration) {
    if let Ok(mut t) = SHARED_HTTP_TIMEOUT.lock() {
        t.retain(|(d, _)| d != data_dir);
        t.push((data_dir.to_owned(), timeout));
    }
}

/// The timeout [`shared_http_timeout`] set for the node whose data directory is `data_dir`.
pub(crate) fn shared_http_timeout_override(data_dir: &std::path::Path) -> Option<Duration> {
    SHARED_HTTP_TIMEOUT
        .lock()
        .ok()?
        .iter()
        .find(|(d, _)| d == data_dir)
        .map(|(_, t)| *t)
}

/// Backup directories whose file writes take long, each with its delay, and
/// how many writes wait there now.
static SLOW_BACKUP_FILES: Mutex<Vec<(std::path::PathBuf, Duration, usize)>> =
    Mutex::new(Vec::new());

/// Make each file a backup writes into `dir` (a snapshot copy, kept code, the
/// kept cluster key) hold the thread that writes it for `delay`, or until
/// [`heal_backup_files`], as a slow or contended disk's copy and fsync would.
/// [`backup_file_waiting`] says whether one waits meanwhile.
pub fn slow_backup_files(dir: &std::path::Path, delay: Duration) {
    if let Ok(mut s) = SLOW_BACKUP_FILES.lock() {
        s.retain(|(d, ..)| d != dir);
        s.push((dir.to_owned(), delay, 0));
    }
}

/// Let backup files into `dir` be written at the disk's speed again, a write
/// held now included.
pub fn heal_backup_files(dir: &std::path::Path) {
    if let Ok(mut s) = SLOW_BACKUP_FILES.lock() {
        s.retain(|(d, ..)| d != dir);
    }
}

/// Whether a backup file write into `dir` waits now where [`slow_backup_files`] holds it.
#[must_use]
pub fn backup_file_waiting(dir: &std::path::Path) -> bool {
    SLOW_BACKUP_FILES
        .lock()
        .is_ok_and(|s| s.iter().any(|(d, _, n)| d == dir && *n > 0))
}

/// Where a backup writes a file into `dir`: holds the thread as
/// [`slow_backup_files`] says, nothing otherwise.
pub(crate) fn backup_file_write(dir: &std::path::Path) {
    let slowed = |n: isize| {
        let mut s = SLOW_BACKUP_FILES.lock().ok()?;
        let e = s.iter_mut().find(|(d, ..)| d == dir)?;
        e.2 = e.2.saturating_add_signed(n);
        Some(e.1)
    };
    let Some(delay) = slowed(1) else {
        return;
    };
    let started = std::time::Instant::now();
    while started.elapsed() < delay && slowed(0).is_some() {
        std::thread::sleep(Duration::from_millis(1));
    }
    let _left = slowed(-1);
}

/// Whether every node in this process fails to take a restore point ([`fail_restore_points`]).
static FAIL_RESTORE_POINTS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Make every node started in this process answer `503` when asked to take a restore
/// point (`Node::take_restore_point_here`), as a replica does while it is down or its
/// group has no snapshot yet, so a test sees what a passing failure before an upgrade
/// leaves behind. `false` puts it back.
pub fn fail_restore_points(fail: bool) {
    FAIL_RESTORE_POINTS.store(fail, std::sync::atomic::Ordering::SeqCst);
}

/// Whether [`fail_restore_points`] is on.
pub(crate) fn restore_points_fail() -> bool {
    FAIL_RESTORE_POINTS.load(std::sync::atomic::Ordering::SeqCst)
}

/// How many of the automatic-update passes in this process run now, and the most that
/// ever ran at once ([`most_auto_update_passes_at_once`]).
static AUTO_UPDATE_PASSES: Mutex<(usize, usize)> = Mutex::new((0, 0));

/// The most automatic-update passes (`Node::auto_update_once`) that ran at once in this
/// process, so a test sees whether a pass ever overlaps another.
#[must_use]
pub fn most_auto_update_passes_at_once() -> usize {
    AUTO_UPDATE_PASSES.lock().map_or(0, |p| p.1)
}

/// Counts an automatic-update pass from its start until it is dropped.
pub(crate) struct AutoUpdatePass;

impl AutoUpdatePass {
    pub(crate) fn start() -> Self {
        if let Ok(mut p) = AUTO_UPDATE_PASSES.lock() {
            p.0 += 1;
            p.1 = p.1.max(p.0);
        }
        Self
    }
}

impl Drop for AutoUpdatePass {
    fn drop(&mut self) {
        if let Ok(mut p) = AUTO_UPDATE_PASSES.lock() {
            p.0 = p.0.saturating_sub(1);
        }
    }
}

/// How far update decisions' times run ahead of the clock ([`shift_update_clock`]).
static UPDATE_CLOCK_SHIFT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Make every update decision logged from now on in this process (an upgrade's `at`, an
/// undo's, the automatic pass's: `crate::types::update_now`) carry a time `secs` ahead
/// of the clock, so a test sees a hold or the undo window pass.
/// `0` puts it back.
pub fn shift_update_clock(secs: u64) {
    UPDATE_CLOCK_SHIFT.store(secs, std::sync::atomic::Ordering::SeqCst);
}

/// The shift [`shift_update_clock`] set.
pub(crate) fn update_clock_shift() -> u64 {
    UPDATE_CLOCK_SHIFT.load(std::sync::atomic::Ordering::SeqCst)
}
