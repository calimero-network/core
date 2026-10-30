//! A job node (`merod run --job-node`): runs compute for a cluster's apps and
//! holds none of its keys, store, Raft or system group; serves only `/health`, `/attestation` and `/job/*`.

mod admission;
mod bundles;
mod exec;
mod runs;

use std::net::SocketAddr;
use std::path::PathBuf;
#[cfg(feature = "testing")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_sealed::{expire_sessions, intercept, SealedOptions, SealedTransport};
use serde::Deserialize;
use serde_json::{json, to_value, Value};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::attest::{job_key_binding, quote, AttestationDoc, Mode, Release};
use crate::error::ApiError;
use crate::node::attested_release;

pub const RUNS_PER_JOB_NODE: usize = 2; // How many compute runs one job node takes at once

const SCRATCH_DIR: &str = "scratch"; // Where run files live under the data directory

const STOP_WAIT: Duration = Duration::from_secs(2); // How long [`RunningJob::stop`] waits for the server to drain

const SWEEP_EVERY: Duration = Duration::from_secs(1); // How often finished and abandoned runs are looked for

pub use runs::{
    AWAITING_INPUTS_TTL, CANCEL_WAIT, JOB_RESULT_TTL, MAX_ASSIGNMENT_BYTES, MAX_FINISHED_BYTES,
    MAX_RUN_INPUTS, MAX_RUN_INPUT_BYTES, NEED_BUNDLE, TOKEN_HEADER, UNKNOWN_RUN,
};

/// What a job node is started with: nothing of a cluster's storage or keys.
#[derive(Clone, Debug)]
pub struct JobConfig {
    pub node_id: u64,
    pub listen: SocketAddr,
    /// Run files live under its `scratch` directory.
    pub data_dir: PathBuf,
    pub cluster: String,
    pub attestation: Mode,
    pub sealed_required: bool,
    pub release: String,
    pub tee_release: Option<String>,
    pub tee_profile: Option<String>,
    /// The cluster's identity key (`GET /ops/cluster`): work and components it did not sign are refused.
    pub cluster_identity: [u8; 32],
}

impl JobConfig {
    fn attested_release(&self) -> Release {
        attested_release(
            &self.release,
            self.tee_release.as_deref(),
            self.tee_profile.as_deref(),
            self.attestation,
        )
    }
}

struct JobNode {
    config: JobConfig,
    transport: Arc<SealedTransport>,
    /// Signs the reports of this node's runs. Made in memory at start, never stored.
    job_key: ed25519_dalek::SigningKey,
    /// Compiles and runs compute only; holds no key.
    engine: Arc<WasmtimeEngine>,
    bundles: bundles::Bundles,
    runs: runs::Runs,
    scratch: PathBuf,
    dirs: AtomicU64,
    /// While set, every run answers `running`, as a stuck node would (feature `testing`).
    #[cfg(feature = "testing")]
    stuck: AtomicBool,
    /// While set, run status and cancel never answer, as a frozen node (feature `testing`).
    #[cfg(feature = "testing")]
    frozen: AtomicBool,
    /// While set, every output route answers an error (feature `testing`).
    #[cfg(feature = "testing")]
    outputs_fail: AtomicBool,
}

impl JobNode {
    /// A directory name no other run of this process has.
    fn next_dir(&self) -> u64 {
        self.dirs.fetch_add(1, Ordering::Relaxed)
    }
}

/// `m`'s contents; a thread that panicked holding it leaves them usable.
fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running job node.
pub struct RunningJob {
    pub addr: SocketAddr,
    /// The job signing key's public half, as `purpose=jobs` attests it.
    pub job_key: [u8; 32],
    job: Arc<JobNode>,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl RunningJob {
    /// Stop serving, and stop the runs.
    pub async fn stop(self) {
        self.job.runs.cancel_all();
        self.shutdown.cancel();
        let mut server = self.server;
        if tokio::time::timeout(STOP_WAIT, &mut server).await.is_err() {
            server.abort();
        }
    }

    /// Makes every run's status and cancel answer `running` from now on. Tests only.
    #[cfg(feature = "testing")]
    pub fn answer_running_always(&self) {
        self.job.stuck.store(true, Ordering::Relaxed);
    }

    /// Makes run status and cancel requests hang from now on. Tests only.
    #[cfg(feature = "testing")]
    pub fn freeze(&self) {
        self.job.frozen.store(true, Ordering::Relaxed);
    }

    /// Makes every output and key route answer an error from now on. Tests only.
    #[cfg(feature = "testing")]
    pub fn fail_outputs(&self) {
        self.job.outputs_fail.store(true, Ordering::Relaxed);
    }

    /// Signs `report` as this node's job key would for the assignment with `digest`. Tests only.
    #[cfg(feature = "testing")]
    pub fn sign_report(
        &self,
        digest: &[u8; 32],
        report: &crate::job_wire::Report,
    ) -> eyre::Result<String> {
        crate::job_wire::sign_report(&self.job.job_key, digest, report)
    }
}

/// Refuses a cluster identity that is not a curve point, or is one of small order: a signature
/// by such a key can be made without holding it.
pub fn check_cluster_identity(identity: &[u8; 32]) -> eyre::Result<()> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(identity)
        .map_err(|_| eyre::eyre!("the cluster identity is not a point of the curve"))?;
    if key.is_weak() {
        eyre::bail!("the cluster identity is a weak key");
    }
    Ok(())
}

/// Start a job node and serve until stopped.
pub async fn run_job(config: JobConfig) -> eyre::Result<RunningJob> {
    check_cluster_identity(&config.cluster_identity)?;
    let scratch = config.data_dir.join(SCRATCH_DIR);
    // A restarted node knows no runs: what an earlier process left is no one's.
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch)?;
    }
    std::fs::create_dir_all(&scratch)?;
    let listener = TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    let transport = Arc::new(SealedTransport::generate(&SealedOptions {
        required: config.sealed_required,
        unsealed_paths: vec!["/attestation".into(), "/health".into()],
    }));
    let job_key = ed25519_dalek::SigningKey::from_bytes(&calimero_identity::random::<32>());
    let public = job_key.verifying_key().to_bytes();
    let node_id = config.node_id;
    let job = Arc::new(JobNode {
        config,
        transport: Arc::clone(&transport),
        job_key,
        engine: Arc::new(WasmtimeEngine::for_compute()?),
        bundles: bundles::Bundles::new(),
        runs: runs::Runs::default(),
        scratch,
        dirs: AtomicU64::new(0),
        #[cfg(feature = "testing")]
        stuck: AtomicBool::new(false),
        #[cfg(feature = "testing")]
        frozen: AtomicBool::new(false),
        #[cfg(feature = "testing")]
        outputs_fail: AtomicBool::new(false),
    });
    let router = Router::new()
        .route("/health", get(health))
        .route("/attestation", get(attestation))
        .route("/job/load", get(load))
        .route("/job/bundles/{hash}", put(bundles::put_bundle))
        .route(
            "/job/runs",
            post(runs::assign).layer(DefaultBodyLimit::max(runs::MAX_ASSIGNMENT_BYTES)),
        )
        .route(
            "/job/runs/{app}/{run}/{attempt}",
            get(runs::status).delete(runs::cancel),
        )
        .route(
            "/job/runs/{app}/{run}/{attempt}/inputs/{i}",
            put(runs::put_input),
        )
        .route(
            "/job/runs/{app}/{run}/{attempt}/outputs/{i}",
            get(runs::output),
        )
        .route(
            "/job/runs/{app}/{run}/{attempt}/outputs/{i}/key",
            get(runs::output_key),
        )
        // Components and inputs exceed axum's 2 MiB default; the sealed envelope caps requests.
        .layer(DefaultBodyLimit::max(
            calimero_sealed::server::MAX_SEALED_BYTES,
        ))
        .with_state(Arc::clone(&job));
    let app = tower::Layer::layer(
        &axum::middleware::from_fn_with_state(Arc::clone(&transport), intercept),
        router,
    );
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let server = tokio::spawn(async move {
        let svc = axum::ServiceExt::<axum::extract::Request>::into_make_service(app);
        if let Err(e) = axum::serve(listener, svc)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await
        {
            warn!(error = %e, "job node server stopped");
        }
    });
    drop(tokio::spawn(expire_sessions(
        Arc::downgrade(&transport),
        shutdown.clone(),
    )));
    drop(tokio::spawn(sweep_runs(Arc::clone(&job), shutdown.clone())));
    info!(node = node_id, %addr, "merod serving as a job node");
    Ok(RunningJob {
        addr,
        job_key: public,
        job,
        shutdown,
        server,
    })
}

/// Drops runs whose time is up, and their files.
async fn sweep_runs(job: Arc<JobNode>, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(SWEEP_EVERY) => {}
        }
        for run in job.runs.sweep(Instant::now()) {
            let _ = tokio::fs::remove_dir_all(&run.dir).await;
        }
    }
}

async fn health(State(job): State<Arc<JobNode>>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "node": job.config.node_id,
        "role": "job",
        "release": job.config.release,
    }))
}

#[derive(Deserialize)]
struct AttestQuery {
    nonce: String,
    /// Must be `jobs`.
    purpose: Option<String>,
}

async fn attestation(
    State(job): State<Arc<JobNode>>,
    Query(q): Query<AttestQuery>,
) -> Result<Json<Value>, ApiError> {
    let nonce: [u8; 32] = hex::decode(&q.nonce)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| ApiError::bad_request("BAD_NONCE", "nonce must be 32 bytes hex"))?;
    // A quote for the transport key alone would open every channel that
    // attests it the usual way (key delivery, peer posts) to this node.
    if q.purpose.as_deref() != Some("jobs") {
        return Err(ApiError::not_found("a job node attests only its job key"));
    }
    let transport_key = job.transport.public_key();
    let job_key = job.job_key.verifying_key().to_bytes();
    let mut report = [0; 64];
    report[..32].copy_from_slice(&nonce);
    let release = job.config.attested_release();
    report[32..].copy_from_slice(&job_key_binding(
        &transport_key,
        &job_key,
        &release.version,
        &job.config.cluster,
        job.config.node_id,
        Some(&job.config.cluster_identity),
    ));
    let quote =
        quote(job.config.attestation, report).map_err(|e| ApiError::internal(&e.to_string()))?;
    let doc = AttestationDoc {
        quote: STANDARD.encode(quote),
        transport_key: hex::encode(transport_key),
        app_hash: hex::encode([0; 32]),
        nonce: hex::encode(nonce),
        release,
        cluster: job.config.cluster.clone(),
        node: job.config.node_id,
        mock: job.config.attestation == Mode::Mock,
        receipt_key: None,
        governance: None,
        kms: None,
        job_key: Some(hex::encode(job_key)),
        cluster_identity: Some(hex::encode(job.config.cluster_identity)),
    };
    Ok(Json(to_value(doc).unwrap_or_default()))
}

/// `GET /job/load`: how many runs this node has and how many it takes.
async fn load(State(job): State<Arc<JobNode>>) -> Json<Value> {
    Json(json!({
        "running": job.runs.active(),
        "capacity": RUNS_PER_JOB_NODE,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cluster_identity_must_be_a_strong_curve_point() {
        let real = crate::backup::cluster_identity_key(&[1; 32]);
        assert!(check_cluster_identity(&real.verifying_key().to_bytes()).is_ok());
        // The neutral element and a point of order 4 sign for anyone.
        let mut neutral = [0u8; 32];
        neutral[0] = 1;
        for weak in [neutral, [0u8; 32]] {
            let e = check_cluster_identity(&weak).unwrap_err();
            assert!(e.to_string().contains("weak"), "{e}");
        }
        let off_curve = (2u8..=255)
            .map(|y| {
                let mut b = [0u8; 32];
                b[0] = y;
                b
            })
            .find(|b| ed25519_dalek::VerifyingKey::from_bytes(b).is_err())
            .unwrap();
        let e = check_cluster_identity(&off_curve).unwrap_err();
        assert!(e.to_string().contains("not a point"), "{e}");
    }
}
