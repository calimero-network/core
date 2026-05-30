//! Narrow trait over the [`NetworkClient`] surface that the sync
//! subsystem actually uses.
//!
//! Sync only needs two methods (`mesh_peers`, `open_stream`); the real
//! `NetworkClient` has ~15. Defining a sync-specific trait here lets
//! tests substitute a mock without spinning up an Actix runtime + the
//! whole libp2p stack, and lets the post-extraction sync crate
//! (#2302) declare its network dependency in one place.
//!
//! The mock lives in the sibling `mock` submodule under `#[cfg(test)]`
//! so its types are never compiled into production binaries. See the
//! `manager/mod.rs` + `manager/tests.rs` precedent for the same
//! mod-dir layout.
//!
//! **`open_stream` mockability**: `Stream` wraps a real `libp2p::Stream`
//! in production, but `Stream::test_pair()` (behind the network-primitives
//! `test-utils` feature, which the node enables on its dev-dependency
//! edge) backs it with an in-memory duplex pipe. Mocks can therefore
//! script `Err(_)`/hang *and* a synthetic `Ok(Stream)`, covering both
//! the retry/timeout/error paths and the success/recovery paths of the
//! namespace-join discovery loop, snapshot/delta-request opens, etc.
//!
//! See #2406 + #2302 (sync extraction epic).
//!
//! [`NetworkClient`]: calimero_network_primitives::client::NetworkClient

use async_trait::async_trait;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;

#[cfg(test)]
pub(crate) mod mock;

/// The network surface the sync subsystem uses.
///
/// The blanket impl on `NetworkClient` is the production wiring; the
/// `#[cfg(test)]` mock in the sibling [`mock`] module is the unit-test
/// wiring.
#[async_trait]
pub trait SyncNetwork: Send + Sync + 'static {
    /// Return the connected peers SUBSCRIBED to `topic` — the full
    /// subscriber set, not just the grafted gossipsub mesh.
    ///
    /// Used by sync to discover sync peers and namespace-join targets.
    /// Deliberately the subscriber set rather than the mesh: a peer can
    /// be connected and subscribed yet not (yet/still) in our mesh
    /// (GRAFT lag, PRUNE, mesh churn under flaky relay links), and sync
    /// must be able to reconcile with any connected subscriber regardless
    /// of mesh health — otherwise a healthy 3-member context reports "no
    /// peers" whenever the mesh is momentarily thin, and a malicious peer
    /// occupying a mesh slot could starve sync discovery.
    async fn subscribed_peers(&self, topic: TopicHash) -> Vec<PeerId>;

    /// Open a new substream to `peer_id` over the calimero protocol.
    ///
    /// Used by every sync initiator path. Mock impls can return
    /// `Err(_)` to exercise the retry/timeout/error branches, or a
    /// `Stream::test_pair()` end to exercise the success branch.
    async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<Stream>;
}

#[async_trait]
impl SyncNetwork for NetworkClient {
    // Fully-qualified syntax — `NetworkClient::subscribed_peers(self, …)`
    // rather than `self.subscribed_peers(…)` — is used here defensively.
    // Rust's method resolution prefers inherent methods over trait
    // methods today, so both forms dispatch to the inherent method
    // and a rename would fail to compile under either form. The
    // load-bearing difference is *removal*: if the inherent method
    // is ever removed in `calimero-network-primitives`, the
    // bare-self form silently starts dispatching to this trait
    // method, recursing forever; the fully-qualified form fails to
    // compile instead — the failure mode we want.
    async fn subscribed_peers(&self, topic: TopicHash) -> Vec<PeerId> {
        NetworkClient::subscribed_peers(self, topic).await
    }

    async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<Stream> {
        NetworkClient::open_stream(self, peer_id).await
    }
}
