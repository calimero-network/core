//! Receiver-side handlers for the namespace-topic readiness variants.
//!
//! Phase 7.3 of the three-phase governance contract (#2237).
//!
//! - `handle_readiness_beacon` verifies the signature + namespace
//!   membership via [`verify_readiness_beacon`], inserts the beacon
//!   into the shared [`ReadinessCache`] (the cache is internally
//!   synchronised, so we bypass the `ReadinessManager` mailbox here
//!   to avoid a per-beacon hop), then notifies the manager via
//!   [`ApplyBeaconLocal`] so the FSM can re-evaluate against the new
//!   `peer_summary`.
//! - `handle_readiness_probe` forwards the probe to the manager which
//!   rate-limits the per-(peer, namespace) response at
//!   `BEACON_INTERVAL / 2` — see
//!   [`Handler<EmitOutOfCycleBeacon>`](crate::readiness::ReadinessManager).

use calimero_context::governance_broadcast::verify_readiness_beacon;
use calimero_context_client::local_governance::{ReadinessProbe, SignedReadinessBeacon};
use libp2p::PeerId;
use tracing::debug;

use crate::readiness::{ApplyBeaconLocal, EmitOutOfCycleBeacon};
use crate::NodeManager;

pub(super) fn handle_readiness_beacon(
    manager: &mut NodeManager,
    _ctx: &mut actix::Context<NodeManager>,
    _peer_id: PeerId,
    beacon: SignedReadinessBeacon,
) {
    if !verify_readiness_beacon(&manager.datastore, &beacon) {
        debug!(
            namespace_id = %hex::encode(beacon.namespace_id),
            "ReadinessBeacon failed verification; dropping"
        );
        return;
    }
    let namespace_id = beacon.namespace_id;
    manager.readiness_cache.insert(&beacon);
    // Wake any `await_first_fresh_beacon` waiters for this namespace
    // (Phase 8.1). Must run AFTER `cache.insert` so a waiter that
    // re-checks the cache on wakeup sees the new entry.
    manager.readiness_notify.notify(namespace_id);
    if let Some(addr) = &manager.readiness_addr {
        addr.do_send(ApplyBeaconLocal { namespace_id });
    }
}

pub(super) fn handle_readiness_probe(
    manager: &mut NodeManager,
    _ctx: &mut actix::Context<NodeManager>,
    peer_id: PeerId,
    probe: ReadinessProbe,
) {
    // Forward to the manager so it can rate-limit per-(peer, namespace).
    // No verification needed at this layer — `ReadinessProbe` is
    // unsigned (it carries a 16-byte nonce only), and the
    // `EmitOutOfCycleBeacon` handler is the choke point that prevents
    // probe-driven amplification regardless of probe content.
    if let Some(addr) = &manager.readiness_addr {
        addr.do_send(EmitOutOfCycleBeacon {
            namespace_id: probe.namespace_id,
            requesting_peer: peer_id,
        });
    }
}
