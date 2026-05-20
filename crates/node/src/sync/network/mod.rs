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
//! **`open_stream` mockability**: `Stream` is a concrete type wrapping
//! a real `libp2p::Stream` and currently has no synthetic constructor.
//! Mocks can return `Err(_)` from `open_stream` (sufficient for
//! testing retry/timeout/error paths in the namespace-join discovery
//! loop, the snapshot/delta-request open paths, etc.) but cannot
//! return a synthetic `Ok(Stream)` until a `Stream::test_pair()`
//! constructor lands — tracked as a follow-up.
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
    /// Return the current gossipsub mesh peers for `topic`.
    ///
    /// Used by sync to discover sync peers and namespace-join targets.
    async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId>;

    /// Open a new substream to `peer_id` over the calimero protocol.
    ///
    /// Used by every sync initiator path. Mock impls can return
    /// `Err(_)` to exercise the retry/timeout/error branches.
    async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<Stream>;
}

#[async_trait]
impl SyncNetwork for NetworkClient {
    // Fully-qualified syntax — `NetworkClient::mesh_peers(self, …)`
    // rather than `self.mesh_peers(…)` — is used here defensively.
    // Rust's method resolution prefers inherent methods over trait
    // methods today, so both forms dispatch to the inherent method
    // and a rename would fail to compile under either form. The
    // load-bearing difference is *removal*: if the inherent method
    // is ever removed in `calimero-network-primitives`, the
    // bare-self form silently starts dispatching to this trait
    // method, recursing forever; the fully-qualified form fails to
    // compile instead — the failure mode we want.
    async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId> {
        NetworkClient::mesh_peers(self, topic).await
    }

    async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<Stream> {
        NetworkClient::open_stream(self, peer_id).await
    }
}
