//! The Calimero cloud node (release-v2).
//!
//! One process inside a TDX VM. It hosts the cluster's system group and the
//! app groups placed on it, runs app calls on each group's leader, and serves
//! clients only through the sealed channel bound into its attestation.
//! See `docs/` (Calimero Confidential Cloud architecture) for the design.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub mod api;
pub mod app;
pub mod attest;
pub mod backup;
pub mod error;
pub mod kms;
pub mod node;
pub mod system;
pub mod types;

pub use node::{Config, Node};

/// A running node.
pub struct Running {
    pub node: Arc<Node>,
    pub addr: std::net::SocketAddr,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl Running {
    /// Stop serving and stop every group.
    /// Open streams (event subscriptions) get two seconds to drain.
    pub async fn stop(self) {
        self.shutdown.cancel();
        let mut server = self.server;
        if tokio::time::timeout(Duration::from_secs(2), &mut server)
            .await
            .is_err()
        {
            server.abort();
        }
        self.node.engine.shutdown().await;
    }
}

/// Start a node and serve its API until stopped.
pub async fn run(config: Config) -> eyre::Result<Running> {
    let listener = TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    let node = Node::start(config).await?;
    let shutdown = CancellationToken::new();

    // The sealed envelope wraps the router from outside, so an opened request
    // is routed afresh and meets the same handlers as a direct one.
    let app = tower::Layer::layer(
        &axum::middleware::from_fn_with_state(
            Arc::clone(&node.transport),
            calimero_sealed::intercept,
        ),
        api::router(Arc::clone(&node)),
    );
    let stop = shutdown.clone();
    let server = tokio::spawn(async move {
        let svc = axum::ServiceExt::<axum::extract::Request>::into_make_service(app);
        if let Err(e) = axum::serve(listener, svc)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await
        {
            warn!(error = %e, "server stopped");
        }
    });

    drop(tokio::spawn(calimero_sealed::expire_sessions(
        Arc::downgrade(&node.transport),
        shutdown.clone(),
    )));

    // Keep hosted groups in line with the system state.
    let weak = Arc::downgrade(&node);
    let stop = shutdown.clone();
    let mut applied = node.system.subscribe_applied();
    drop(tokio::spawn(async move {
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                _ = applied.changed() => {}
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            let Some(node) = weak.upgrade() else { return };
            node.reconcile().await;
        }
    }));

    // Encrypted snapshots to object storage.
    if node.config.backup_dir.is_some() {
        let weak = Arc::downgrade(&node);
        let stop = shutdown.clone();
        drop(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = stop.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(60)) => {}
                }
                let Some(node) = weak.upgrade() else { return };
                if let Err(e) = node.backup().await {
                    warn!(error = %e, "backup failed");
                }
            }
        }));
    }

    info!(node = node.config.node_id, %addr, "merod-v2 serving");
    Ok(Running {
        node,
        addr,
        shutdown,
        server,
    })
}
