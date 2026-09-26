//! Re-announce a TEE replica whose authority evidence never landed.
//!
//! The evidence an admitter publishes right after admitting a TEE carries the
//! TEE's raw quote, and only the TEE holds that quote. Fleet-join stops
//! announcing once admitted, so if that one publish failed, nothing would ever
//! produce the evidence again and the TEE could never author. This loop closes
//! that gap from the TEE's side: while it is owed evidence
//! ([`calimero_governance_store::tee_evidence_owed`]) in a namespace, it
//! announces itself again with a fresh quote. An admitter that hears an
//! already-admitted TEE with no evidence publishes the evidence, the same path
//! an admission takes, so no new wire message is involved.
//!
//! Every node runs it, and it is idle on any node that is not an admitted TEE.

use std::collections::HashMap;
use std::time::Duration;

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::NamespaceRepository;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// How often the loop looks for a namespace owed evidence.
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// The wait before the first re-announce in a namespace, doubled after each
/// one that does not settle it.
const FIRST_BACKOFF: Duration = Duration::from_secs(60);
/// The longest wait between re-announces. Each one costs every admitter that
/// hears it a quote verification and a collateral fetch.
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// When a namespace may be announced to next, and the wait after that.
struct Backoff {
    next_at: tokio::time::Instant,
    wait: Duration,
}

/// The wait after one that did not settle anything: doubled, up to the cap.
fn next_wait(wait: Duration) -> Duration {
    wait.saturating_mul(2).min(MAX_BACKOFF)
}

/// Run until `shutdown` fires.
pub async fn run(
    store: Store,
    node_client: NodeClient,
    #[cfg(feature = "mock-attestation")] mock_tee: bool,
    shutdown: CancellationToken,
) {
    let mut backoff: HashMap<ContextGroupId, Backoff> = HashMap::new();
    let mut ticker = tokio::time::interval(CHECK_INTERVAL);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let owed = match owed_namespaces(&store) {
            Ok(owed) => owed,
            Err(err) => {
                debug!(?err, "TEE evidence retry: could not read governance state");
                continue;
            }
        };
        // Settled namespaces start from the first wait if they are ever owed
        // evidence again.
        backoff.retain(|namespace, _| owed.contains(namespace));

        let now = tokio::time::Instant::now();
        for namespace in owed {
            // The first announce waits a full backoff: right after an admission
            // the admitter's own evidence publish is still in flight, and that
            // is the ordinary way the evidence arrives.
            let entry = backoff.entry(namespace).or_insert(Backoff {
                next_at: now + FIRST_BACKOFF,
                wait: FIRST_BACKOFF,
            });
            if now < entry.next_at {
                continue;
            }
            entry.next_at = now + entry.wait;
            entry.wait = next_wait(entry.wait);

            announce(
                &store,
                &node_client,
                &namespace,
                #[cfg(feature = "mock-attestation")]
                mock_tee,
            )
            .await;
        }
    }
}

/// The namespaces this node takes part in where its own account is owed
/// evidence.
fn owed_namespaces(store: &Store) -> eyre::Result<Vec<ContextGroupId>> {
    let namespaces = NamespaceRepository::new(store);
    let mut owed = Vec::new();
    for namespace in namespaces.participating_namespaces()? {
        let Some((public_key, _)) = namespaces.resolve_identity(&namespace)? else {
            continue;
        };
        let Some(account) =
            calimero_governance_store::member_account_in_namespace(store, &namespace, &public_key)?
        else {
            continue;
        };
        if calimero_governance_store::tee_evidence_owed(store, &namespace, &account)? {
            owed.push(namespace);
        }
    }
    Ok(owed)
}

async fn announce(
    store: &Store,
    node_client: &NodeClient,
    namespace: &ContextGroupId,
    #[cfg(feature = "mock-attestation")] mock_tee: bool,
) {
    let Ok(Some((public_key, _))) = NamespaceRepository::new(store).resolve_identity(namespace)
    else {
        return;
    };
    let announcement = match super::announce::build(
        store,
        namespace,
        public_key,
        #[cfg(feature = "mock-attestation")]
        mock_tee,
    ) {
        Ok(announcement) => announcement,
        Err(err) => {
            warn!(
                namespace = %hex::encode(namespace.to_bytes()),
                reason = err.message(),
                "TEE evidence retry: could not build an announcement"
            );
            return;
        }
    };
    match node_client
        .publish_on_namespace_now(namespace.to_bytes(), announcement.payload)
        .await
    {
        Ok(mesh_peers) => info!(
            namespace = %hex::encode(namespace.to_bytes()),
            mesh_peers,
            "TEE authority evidence is missing; re-announced so an admitter publishes it"
        ),
        Err(err) => debug!(
            namespace = %hex::encode(namespace.to_bytes()),
            ?err,
            "TEE evidence retry: re-announce publish failed; retrying later"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_up_to_the_cap() {
        assert_eq!(next_wait(FIRST_BACKOFF), FIRST_BACKOFF * 2);
        assert_eq!(next_wait(MAX_BACKOFF), MAX_BACKOFF);
        let mut wait = FIRST_BACKOFF;
        for _ in 0..64 {
            wait = next_wait(wait);
        }
        assert_eq!(wait, MAX_BACKOFF);
    }
}
