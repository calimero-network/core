//! A job node (`merod run --job-node`): runs compute for a cluster's apps and
//! holds none of its keys, store, Raft or system group; serves only `/health`, `/attestation` and `/job/*`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_sealed::{expire_sessions, intercept, SealedOptions, SealedTransport};
use serde::Deserialize;
use serde_json::{json, to_value, Value};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::attest::{job_key_binding, quote, AttestationDoc, Measurements, Mode, Policy, Release};
use crate::error::ApiError;
use crate::node::{attested_release, peer_policy};

/// How many compute runs one job node takes at once.
pub const RUNS_PER_JOB_NODE: usize = 2;

/// Where run files live under the data directory.
const SCRATCH_DIR: &str = "scratch";

/// How long [`RunningJob::stop`] waits for the server to drain.
const STOP_WAIT: Duration = Duration::from_secs(2);

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
    /// The cluster's nodes, by id: the leaders this node will take work from.
    pub peers: BTreeMap<u64, String>,
    /// How this node will judge a leader's quote, as a cluster node judges its peers'.
    pub allow_mock_peers: bool,
    pub peer_measurements: Measurements,
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

    /// How this node judges a leader's quote.
    #[must_use]
    pub fn peer_policy(&self) -> Policy {
        peer_policy(
            self.allow_mock_peers,
            &self.peer_measurements,
            self.tee_profile.as_deref(),
        )
    }
}

struct JobNode {
    config: JobConfig,
    transport: Arc<SealedTransport>,
    /// Signs the reports of this node's runs. Made in memory at start, never stored.
    job_key: ed25519_dalek::SigningKey,
    running: AtomicUsize,
}

/// A running job node.
pub struct RunningJob {
    pub addr: SocketAddr,
    /// The job signing key's public half, as `purpose=jobs` attests it.
    pub job_key: [u8; 32],
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl RunningJob {
    /// Stop serving.
    pub async fn stop(self) {
        self.shutdown.cancel();
        let mut server = self.server;
        if tokio::time::timeout(STOP_WAIT, &mut server).await.is_err() {
            server.abort();
        }
    }
}

/// Start a job node and serve until stopped.
pub async fn run_job(config: JobConfig) -> eyre::Result<RunningJob> {
    std::fs::create_dir_all(config.data_dir.join(SCRATCH_DIR))?;
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
        running: AtomicUsize::new(0),
    });
    let router = Router::new()
        .route("/health", get(health))
        .route("/attestation", get(attestation))
        .route("/job/load", get(load))
        .with_state(job);
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
    info!(node = node_id, %addr, "merod serving as a job node");
    Ok(RunningJob {
        addr,
        job_key: public,
        shutdown,
        server,
    })
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
    };
    Ok(Json(to_value(doc).unwrap_or_default()))
}

/// `GET /job/load`: how many runs this node has and how many it takes.
async fn load(State(job): State<Arc<JobNode>>) -> Json<Value> {
    Json(json!({
        "running": job.running.load(Ordering::SeqCst),
        "capacity": RUNS_PER_JOB_NODE,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(tee_profile: Option<&str>, allow_mock_peers: bool) -> JobConfig {
        JobConfig {
            node_id: 9,
            listen: ([127, 0, 0, 1], 0).into(),
            data_dir: PathBuf::new(),
            cluster: "c".into(),
            attestation: Mode::Mock,
            sealed_required: false,
            release: "r".into(),
            tee_release: None,
            tee_profile: tee_profile.map(str::to_owned),
            peers: BTreeMap::new(),
            allow_mock_peers,
            peer_measurements: Measurements::Release {
                allowed_profiles: vec!["p".into()],
                min_version: Some("2.4.0".into()),
            },
        }
    }

    #[test]
    fn a_job_node_judges_leaders_by_the_policy_a_cluster_node_judges_peers() {
        let policy = config(None, true).peer_policy();
        assert!(policy.allow_mock);
        assert!(!policy.allow_debug_profiles);
        assert!(matches!(policy.measurements, Measurements::Release { .. }));
        assert!(!config(None, false).peer_policy().allow_mock);
        assert!(
            config(Some("debug-read-only"), false)
                .peer_policy()
                .allow_debug_profiles
        );
    }
}
