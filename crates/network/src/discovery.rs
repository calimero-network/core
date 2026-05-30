use core::net::Ipv4Addr;
use core::time::Duration;
use std::collections::{BTreeSet, HashMap, HashSet};

use calimero_network_primitives::config::{AutonatConfig, RelayConfig, RendezvousConfig};
use eyre::{bail, ContextCompat, Result as EyreResult, WrapErr as _};
use std::time::{SystemTime, UNIX_EPOCH};

use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use libp2p::rendezvous::client::RegisterError;
use libp2p::rendezvous::Namespace;
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::PeerId;
use multiaddr::{Multiaddr, Protocol};
use tracing::{debug, error, info, trace};

use super::NetworkManager;
use crate::discovery::peer_cache::{PeerAddrCache, PersistedPeer};
use crate::discovery::state::{
    rendezvous_key_for_topic, under_connected_rendezvous_keys, DiscoveryState, ReachabilityActions,
    RelayReservationStatus, RendezvousRegistrationStatus,
};

pub mod state;

/// How long a cached peer address stays dial-worthy without being seen
/// again. A day comfortably covers laptop-sleep / overnight-restart while
/// aging out peers that have genuinely left.
const PEER_CACHE_TTL_SECS: u64 = 24 * 60 * 60;

/// Fixed node-local datastore key for the single peer-cache blob. The
/// whole relevant-peer set is stored as one value under this key in the
/// `Generic` column (raw-bytes codec).
fn peer_cache_store_key() -> GenericKey {
    GenericKey::new(*b"calimero-peercch", [0u8; 32])
}

// Persistent relevant-peer address cache: recorded on connect, loaded +
// dialed on startup, and re-persisted on the rendezvous tick (see the
// `peer_cache_*` methods below).
pub(crate) mod peer_cache;

/// Max rendezvous `discover` requests issued in a single
/// `rendezvous_discover` call. Bounds the per-tick discovery fan-out so a
/// node that belongs to many under-connected overlays doesn't flood the
/// rendezvous server in one pass; the rotating
/// [`Discovery::rendezvous_discover_cursor`] ensures every under-connected
/// key is eventually covered across successive ticks.
const RENDEZVOUS_DISCOVER_BUDGET: usize = 8;

#[derive(Debug)]
pub struct Discovery {
    pub(crate) state: DiscoveryState,
    pub(crate) rendezvous_config: RendezvousConfig,
    pub(crate) relay_config: RelayConfig,
    pub(crate) advertise: Option<AdvertiseState>,
    pub(crate) _autonat_config: AutonatConfig,
    /// Rotating offset into the under-connected rendezvous-key list, so
    /// successive `rendezvous_discover` calls cover different keys when
    /// there are more under-connected overlays than the per-call budget
    /// (`RENDEZVOUS_DISCOVER_BUDGET`). Without rotation, keys past the
    /// budget would be starved of discovery forever on a node that
    /// belongs to many namespaces/groups.
    pub(crate) rendezvous_discover_cursor: usize,
    /// Subscribed gossipsub topics that must NOT be mapped to a
    /// per-overlay rendezvous key — currently the specialized-node
    /// invite topic, which every node subscribes to. Registering all
    /// nodes under one invite-topic key would recreate the global
    /// fan-out we're eliminating. Every other subscribed topic
    /// (`ns/`, `group/`, or a bare context id) is treated as an overlay.
    pub(crate) reserved_topics: BTreeSet<String>,
}

#[derive(Debug)]
pub struct AdvertiseState {
    pub(crate) ip: Ipv4Addr,
    pub(crate) ports: HashSet<u16>,
}

impl Discovery {
    pub(crate) async fn new(
        rendezvous_config: &RendezvousConfig,
        relay_config: &RelayConfig,
        autonat_config: &AutonatConfig,
        listening_on: &[Multiaddr],
        reserved_topics: BTreeSet<String>,
    ) -> EyreResult<Self> {
        let advertise = if listening_on.is_empty() {
            None
        } else {
            let ports = listening_on
                .iter()
                .filter_map(|addr| {
                    addr.iter().find_map(|p| match p {
                        Protocol::Tcp(port) | Protocol::Udp(port) => Some(port),
                        _ => None,
                    })
                })
                .collect();

            Some(AdvertiseState {
                ip: Self::get_public_ip().await?,
                ports,
            })
        };

        let this = Self {
            state: DiscoveryState::default(),
            rendezvous_config: rendezvous_config.clone(),
            relay_config: relay_config.clone(),
            advertise,
            _autonat_config: autonat_config.clone(),
            rendezvous_discover_cursor: 0,
            reserved_topics,
        };

        Ok(this)
    }

    async fn get_public_ip() -> EyreResult<Ipv4Addr> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()?;

        let ip_addr = client
            .get("https://api.ipify.org")
            .send()
            .await?
            .text()
            .await?
            .parse()?;

        Ok(ip_addr)
    }
}

impl NetworkManager {
    /// Rendezvous keys for every overlay topic this node currently
    /// follows — `ns/<hex>`, `group/<hex>`, and bare context ids — minus
    /// the reserved (non-overlay) topics. Used for registration so
    /// co-members can find us under the exact namespace/group/context key.
    fn overlay_rendezvous_keys(&self) -> Vec<Namespace> {
        let reserved = &self.discovery.reserved_topics;
        self.swarm
            .behaviour()
            .gossipsub
            .topics()
            .filter(|topic| !reserved.contains(topic.as_str()))
            .filter_map(|topic| rendezvous_key_for_topic(topic.as_str()))
            .collect()
    }

    /// Rendezvous keys for overlay topics where we currently have **no
    /// connected mesh peer** — the demand-driven discovery set. A topic
    /// that already has a co-member in its mesh can sync through it, so
    /// re-discovering it would only add rendezvous load. This keeps
    /// discovery cost proportional to how many overlays are starved
    /// (≈0 in steady state; a burst only right after restart/partition).
    fn under_connected_overlay_keys(&self) -> Vec<Namespace> {
        let reserved = &self.discovery.reserved_topics;
        let gossipsub = &self.swarm.behaviour().gossipsub;
        // Count connected SUBSCRIBERS per topic (the full peer-topics
        // set), not grafted mesh peers. A topic with a connected
        // subscriber can already sync through it even if the mesh is
        // momentarily thin (GRAFT lag / churn), so it isn't
        // under-connected — counting the mesh instead would re-trigger
        // discovery on healthy-but-unmeshed topics.
        let mut subscriber_counts: HashMap<String, usize> = HashMap::new();
        for (_peer, topics) in gossipsub.all_peers() {
            for topic in topics {
                *subscriber_counts
                    .entry(topic.as_str().to_owned())
                    .or_default() += 1;
            }
        }
        let topics: Vec<(String, usize)> = gossipsub
            .topics()
            .filter(|topic| !reserved.contains(topic.as_str()))
            .map(|topic| {
                let count = subscriber_counts.get(topic.as_str()).copied().unwrap_or(0);
                (topic.as_str().to_owned(), count)
            })
            .collect();
        under_connected_rendezvous_keys(topics.iter().map(|(t, c)| (t.as_str(), *c)))
    }

    /// Current wall-clock unix seconds, used for peer-cache freshness so
    /// the cache survives process restarts (a monotonic `Instant`
    /// wouldn't). Saturates to 0 if the clock is before the epoch.
    fn now_unix_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Record the address we just connected to `peer_id` on into the
    /// peer cache. Records BOTH direct and relayed-circuit addresses
    /// (unlike the discovery book, which keeps direct only) — a relayed
    /// circuit address is re-dialable after a restart if the peer
    /// re-reserves on the same relay, so it's worth caching for NAT'd
    /// co-members.
    pub(crate) fn record_connected_addr(&mut self, peer_id: PeerId, addr: Multiaddr) {
        let now = self.now_unix_secs();
        self.peer_cache.record(peer_id, addr, now);
    }

    /// Connected peers subscribed to at least one of our own
    /// (non-reserved) overlay topics — the relevant set to persist/dial.
    fn current_overlay_subscribers(&self) -> std::collections::BTreeSet<PeerId> {
        let reserved = &self.discovery.reserved_topics;
        let gossipsub = &self.swarm.behaviour().gossipsub;
        let our_overlays: std::collections::BTreeSet<String> = gossipsub
            .topics()
            .filter(|t| !reserved.contains(t.as_str()))
            .map(|t| t.as_str().to_owned())
            .collect();
        let mut out = std::collections::BTreeSet::new();
        for (peer, topics) in gossipsub.all_peers() {
            if topics.iter().any(|t| our_overlays.contains(t.as_str())) {
                let _ = out.insert(*peer);
            }
        }
        out
    }

    /// Persist the relevant, still-fresh peer cache to the datastore
    /// (best-effort), under a fixed node-local `Generic` key — the
    /// datastore-backed peerstore pattern. Called from the rendezvous
    /// tick. A failed write just means a slower reconnect next restart,
    /// so failures are debug-logged, not propagated. Skips writing when
    /// there are no relevant peers, to avoid churning the blob while the
    /// node is idle/peerless.
    pub(crate) fn persist_peer_cache(&self) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let relevant = self.current_overlay_subscribers();
        if relevant.is_empty() {
            return;
        }
        let records =
            self.peer_cache
                .to_persisted(&relevant, self.now_unix_secs(), PEER_CACHE_TTL_SECS);
        let bytes = match serde_json::to_vec(&records) {
            Ok(bytes) => bytes,
            Err(err) => {
                debug!(?err, "failed to serialize peer cache");
                return;
            }
        };
        let key = peer_cache_store_key();
        let data = GenericData::from(Slice::from(bytes));
        let mut handle = store.handle();
        if let Err(err) = handle.put(&key, &data) {
            debug!(?err, "failed to persist peer cache to store");
        }
    }

    /// Load the persisted peer cache from the datastore and dial the
    /// still-fresh relevant peers, so a restarted node reconnects to its
    /// collaborators immediately instead of waiting a rendezvous
    /// round-trip. Dials are deduped at the swarm level
    /// (`DisconnectedAndNotDialing`); stale cached addresses that fail are
    /// evicted by the discovery book's failure threshold, and rendezvous
    /// supplies fresh ones. Best-effort: a missing or corrupt blob is
    /// ignored.
    pub(crate) fn load_peer_cache_and_dial(&mut self) {
        let now = self.now_unix_secs();
        let records: Vec<PersistedPeer> = {
            let Some(store) = self.store.as_ref() else {
                return;
            };
            let key = peer_cache_store_key();
            match store.handle().get(&key) {
                Ok(Some(data)) => match serde_json::from_slice(data.as_ref()) {
                    Ok(records) => records,
                    Err(err) => {
                        debug!(?err, "ignoring corrupt peer cache blob in store");
                        return;
                    }
                },
                Ok(None) => return, // nothing cached yet
                Err(err) => {
                    debug!(?err, "failed to read peer cache from store");
                    return;
                }
            }
        };
        self.peer_cache = PeerAddrCache::from_persisted(records, now, PEER_CACHE_TTL_SECS);

        let candidates = self.peer_cache.dial_candidates(now, PEER_CACHE_TTL_SECS);
        let count = candidates.len();
        for candidate in candidates {
            let opts = DialOpts::peer_id(candidate.peer_id)
                .condition(PeerCondition::DisconnectedAndNotDialing)
                .addresses(candidate.addrs)
                .build();
            if let Err(err) = self.swarm.dial(opts) {
                debug!(peer_id = %candidate.peer_id, ?err, "peer-cache startup dial skipped");
            }
        }
        if count > 0 {
            info!(count, "dialing cached peers on startup for fast reconnect");
        }
    }

    // Sends rendezvous discovery requests to the rendezvous peer, one per
    // overlay key we're under-connected on, so `discover` returns only
    // co-members of our namespaces/groups rather than the whole network.
    //
    // Throttled via `discovery_rpm` unless `force == true` — see the
    // `rendezvous_discover_force` field on `ReachabilityActions` for
    // when callers should bypass the throttle (event-driven recovery
    // paths where the throttle floor exceeds the recovery budget).
    // Bounded per call by `RENDEZVOUS_DISCOVER_BUDGET` with a rotating
    // cursor so a node in many namespaces covers every under-connected
    // key across ticks without flooding the server in one pass.
    //
    // This function expects that the rendezvous peer is already
    // connected.
    pub(crate) fn rendezvous_discover(
        &mut self,
        rendezvous_peer: &PeerId,
        force: bool,
    ) -> EyreResult<()> {
        let throttled = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err_with(|| format!("Failed to get peer info for {rendezvous_peer}"))?
            .is_rendezvous_discover_throttled(self.discovery.rendezvous_config.discovery_rpm);

        if !force && throttled {
            return Ok(());
        }

        // Global discovery (always): finds peers regardless of which
        // overlays they share. This is the bootstrap / namespace-join
        // path — a node joining a namespace it doesn't belong to yet must
        // still be able to find that namespace's members, and that can't
        // come from per-overlay discovery (it isn't a member). Uses the
        // per-peer cookie for incremental results.
        let global_cookie = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .and_then(|info| info.rendezvous())
            .and_then(|rz| rz.cookie())
            .cloned();
        self.swarm.behaviour_mut().rendezvous.discover(
            Some(self.discovery.rendezvous_config.namespace.clone()),
            global_cookie,
            None,
            *rendezvous_peer,
        );

        // Per-overlay discovery (additive): also pull co-members under the
        // keys we're under-connected on, so steady-state discovery
        // prioritises relevant peers. Paced with a rotating cursor so a
        // node in many namespaces covers every key across ticks without
        // flooding the server. Fresh cookie (`None`) per key — overlay
        // sets are small, so full polls are cheap; the `Discovered`
        // handler dedups before dialing.
        let mut keys = self.under_connected_overlay_keys();
        if !keys.is_empty() {
            let cursor = self.discovery.rendezvous_discover_cursor % keys.len();
            keys.rotate_left(cursor);
            let budget = RENDEZVOUS_DISCOVER_BUDGET.min(keys.len());
            // Advance the cursor by the budget, kept bounded modulo the
            // current key count so it never grows unboundedly and the
            // start index stays well-defined even as the key set changes
            // size between ticks.
            self.discovery.rendezvous_discover_cursor = (cursor + budget) % keys.len();

            for key in keys.into_iter().take(budget) {
                self.swarm.behaviour_mut().rendezvous.discover(
                    Some(key.clone()),
                    None,
                    None,
                    *rendezvous_peer,
                );
                debug!(
                    %rendezvous_peer,
                    rendezvous_namespace = %key,
                    force,
                    "Sent per-overlay discover request to rendezvous node"
                );
            }
        }

        Ok(())
    }

    // Sends rendezvous registrations request to all rendezvous peers which require registration.
    // If rendezvous peer is not connected, it will be dialed which will trigger the registration during identify exchange.
    pub(crate) fn broadcast_rendezvous_registrations(&mut self) {
        #[expect(clippy::needless_collect, reason = "Necessary here; false positive")]
        for peer_id in self
            .discovery
            .state
            .get_rendezvous_peer_ids()
            .collect::<Vec<_>>()
        {
            let Some(peer_info) = self.discovery.state.get_peer_info(&peer_id) else {
                error!(%peer_id, "Failed to lookup peer info");
                continue;
            };

            if !self.discovery.state.is_rendezvous_registration_required(
                self.discovery.rendezvous_config.registrations_limit,
            ) {
                continue;
            }

            if !self.swarm.is_connected(&peer_id) {
                for addr in peer_info.addrs().cloned() {
                    if let Err(err) = self.swarm.dial(addr) {
                        error!(%err, "Failed to dial relay peer");
                    }
                }
            } else if let Err(err) = self.rendezvous_register(&peer_id) {
                error!(%err, "Failed to update rendezvous registration");
            }
        }
    }

    // Registers our external addresses with `rendezvous_peer` under the
    // global namespace AND one key per overlay we belong to
    // (`/calimero/ns/<hex>`, `/calimero/grp/<hex>`, `/calimero/ctx/<id>`).
    //
    // Global registration keeps us findable for the bootstrap /
    // namespace-join path (a peer joining a namespace it doesn't share
    // yet finds members via global discovery). The per-overlay keys let
    // co-members find us under the exact namespace/group/context key, so
    // steady-state discovery is relevant rather than network-wide.
    //
    // If there are no external addresses for the node yet, the rendezvous
    // peer is marked `Pending` (queued, not registered) and the call returns
    // Ok; the next `ExternalAddrConfirmed` re-attempts. Expects the rendezvous
    // peer to be connected.
    pub(crate) fn rendezvous_register(&mut self, rendezvous_peer: &PeerId) -> EyreResult<()> {
        // `registrations_limit` gates how many rendezvous *peers* we
        // register with (infra fan-out), independent of how many keys we
        // register under with each.
        if !self.discovery.state.is_rendezvous_registration_required(
            self.discovery.rendezvous_config.registrations_limit,
        ) {
            return Ok(());
        }

        let mut keys = vec![self.discovery.rendezvous_config.namespace.clone()];
        keys.extend(self.overlay_rendezvous_keys());

        let mut registered_any = false;
        for key in keys {
            match self.swarm.behaviour_mut().rendezvous.register(
                key.clone(),
                *rendezvous_peer,
                None,
            ) {
                Ok(()) => {
                    registered_any = true;
                    debug!(
                        %rendezvous_peer,
                        rendezvous_namespace = %key,
                        "Sent register request to rendezvous node"
                    );
                }
                Err(RegisterError::NoExternalAddresses) => {
                    // `NoExternalAddresses` is a swarm-level condition: the
                    // swarm has no confirmed external address to advertise,
                    // which is independent of the rendezvous namespace. So
                    // every remaining key in this loop would fail
                    // identically — breaking early is safe, and it
                    // necessarily fires on the *first* key, so
                    // `registered_any` is always false here (no key went out
                    // this round).
                    //
                    // Mark the peer Pending ("tried, waiting on an external
                    // address") rather than the misleading Requested; the
                    // next ExternalAddrConfirmed fires
                    // broadcast_rendezvous_registrations, which re-attempts.
                    // But only if the peer is idle: `mark_..._if_idle` leaves
                    // an already Requested/Registered peer untouched. A
                    // re-broadcast can land here while a register is in
                    // flight (status Requested); clobbering it to Pending
                    // would make the Registered handler drop the incoming
                    // confirmation (it requires status == Requested) and
                    // would free a live slot, risking over-fan-out. Such
                    // peers transition out via their own Expired event.
                    if self
                        .discovery
                        .state
                        .mark_rendezvous_pending_if_idle(rendezvous_peer)
                    {
                        trace!(%rendezvous_peer, "No external addresses to register at rendezvous; marked Pending");
                    } else {
                        trace!(%rendezvous_peer, "No external addresses to register at rendezvous; keeping in-flight registration status");
                    }
                    return Ok(());
                }
                Err(err @ RegisterError::FailedToMakeRecord(_)) => bail!(err),
            }
        }

        if registered_any {
            self.discovery.state.update_rendezvous_registration_status(
                rendezvous_peer,
                RendezvousRegistrationStatus::Requested,
            );
        }

        Ok(())
    }

    // We unregister from a rendezvous peer if we were previously registered.
    // This function expectes that the rendezvous peer is already connected.
    pub(crate) fn rendezvous_unregister(&mut self, rendezvous_peer: &PeerId) -> EyreResult<()> {
        let status = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info")?
            .rendezvous()
            .wrap_err("Peer isn't rendezvous")?
            .registration_status();

        match status {
            RendezvousRegistrationStatus::Registered => {
                // Actively unregister from the rendezvous server under the
                // global namespace AND every overlay key we registered
                // under (mirrors `rendezvous_register`). Unregistering a
                // key we aren't registered under is a harmless no-op on
                // the server, so using our current overlay set is safe
                // even if our membership shifted since registration; any
                // stale server-side record TTLs out on its own.
                let mut keys = vec![self.discovery.rendezvous_config.namespace.clone()];
                keys.extend(self.overlay_rendezvous_keys());
                for key in keys {
                    self.swarm
                        .behaviour_mut()
                        .rendezvous
                        .unregister(key, *rendezvous_peer);
                }

                self.discovery.state.update_rendezvous_registration_status(
                    rendezvous_peer,
                    RendezvousRegistrationStatus::Expired,
                );
            }
            RendezvousRegistrationStatus::Requested => {
                // Can't cancel in-flight registration, but mark as expired so we don't
                // consider ourselves registered when the response arrives. The handler
                // for the registration response should check current status and re-register
                // with updated addresses if needed.
                self.discovery.state.update_rendezvous_registration_status(
                    rendezvous_peer,
                    RendezvousRegistrationStatus::Expired,
                );
            }
            RendezvousRegistrationStatus::Discovered
            | RendezvousRegistrationStatus::Pending
            | RendezvousRegistrationStatus::Expired => {
                // Nothing to unregister (Pending never actually sent a record)
            }
        }

        Ok(())
    }

    // Requests relay reservation on relay peer if one is required.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn create_relay_reservation(&mut self, relay_peer: &PeerId) -> EyreResult<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(relay_peer)
            .wrap_err("Failed to get peer info")?;

        if !self
            .discovery
            .state
            .is_relay_reservation_required(self.discovery.relay_config.registrations_limit)
        {
            return Ok(());
        }

        debug!(
            %relay_peer,
            ?peer_info,
            "Attempting to register with rendezvous node"
        );

        let preferred_addr = peer_info
            .get_preferred_addr()
            .wrap_err("Failed to get preferred addr for relay peer")?;

        // libp2p's relay-client transport requires the relay address to
        // be the FULL form:
        //   /<transport>/p2p/<relay-peer>/p2p-circuit/p2p/<self-peer>
        //
        // The `/p2p/<relay-peer>` segment between the transport and
        // `/p2p-circuit` is how the relay-client knows which peer is
        // serving the circuit. Without it, `listen_on` rejects the
        // multiaddr with a near-empty transport-level error and the
        // reservation flow silently fails.
        //
        // Why this would land on an incomplete addr: `peer_info.addrs`
        // is populated from two sources — `update_peer_protocols` (the
        // Identify protocols block, which carries no addresses) and the
        // listen_addrs loop in identify.rs which adds the peer's raw
        // listen_addrs. The peer's listen_addrs are bare network
        // endpoints (`/ip4/.../udp/.../quic-v1`) with no `/p2p/`
        // suffix. `get_preferred_addr` prefers UDP, which means it
        // returns the bare endpoint and the constructed relayed addr
        // ends up missing the relay-peer segment.
        //
        // Fix: append `/p2p/<relay-peer>` to the base only if it isn't
        // already present (some entries in `peer_info.addrs` are
        // populated from connection-established events and DO carry
        // the `/p2p/`).
        let mut base_addr = preferred_addr.clone();
        if !base_addr.iter().any(|p| matches!(p, Protocol::P2p(_))) {
            base_addr.push(Protocol::P2p(*relay_peer));
        }

        let relayed_addr = match base_addr
            .with(Protocol::P2pCircuit)
            .with_p2p(*self.swarm.local_peer_id())
        {
            Ok(addr) => addr,
            Err(err) => {
                bail!("Failed to construct relayed addr for relay peer: {:?}", err)
            }
        };

        // Wrap `listen_on` with context — its underlying error type
        // (libp2p's swarm::ListenError → TransportError) renders to
        // an unhelpfully terse string when propagated raw via `?`, and
        // the identify handler's `error!(%err, ...)` printout above
        // this call site reduces to `err=` with no Display content.
        // That made the previous incarnation of this bug
        // (relayed_addr missing /p2p/<relay-peer>) invisible in logs.
        let listener_id = self
            .swarm
            .listen_on(relayed_addr.clone())
            .wrap_err_with(|| format!("Failed to listen on relayed addr {}", relayed_addr))?;

        // Record the listener id so the ListenerClosed handler can route
        // recovery back to this relay peer even if the close event comes
        // with an empty `addresses` list (the quota-denied-before-address-
        // allocation case).
        self.discovery
            .state
            .record_relay_listener(listener_id, *relay_peer);

        self.discovery
            .state
            .update_relay_reservation_status(relay_peer, RelayReservationStatus::Requested);

        Ok(())
    }

    /// Execute actions determined by DiscoveryState
    pub(crate) fn execute_reachability_actions(&mut self, actions: ReachabilityActions) {
        if !actions.has_actions() {
            return;
        }

        // AutoNAT server control
        if actions.enable_autonat_server {
            if let Err(err) = self.swarm.behaviour_mut().autonat.enable_server() {
                error!(%err, "Failed to enable AutoNAT server");
            }
        }

        if actions.disable_autonat_server {
            if let Err(err) = self.swarm.behaviour_mut().autonat.disable_server() {
                error!(%err, "Failed to disable AutoNAT server");
            }
        }

        // Unregister from rendezvous (do this first when going private)
        for peer_id in actions.rendezvous_unregister {
            if let Err(err) = self.rendezvous_unregister(&peer_id) {
                error!(%err, %peer_id, "Failed to unregister from rendezvous");
            }
        }

        // Create relay reservations
        for peer_id in actions.relay_reservations {
            if let Err(err) = self.create_relay_reservation(&peer_id) {
                error!(%err, %peer_id, "Failed to create relay reservation");
            }
        }

        // Discover peers — throttled path
        for peer_id in &actions.rendezvous_discover {
            if let Err(err) = self.rendezvous_discover(peer_id, false) {
                error!(%err, %peer_id, "Failed to discover via rendezvous");
            }
        }

        // Discover peers — force path (bypasses throttle, for
        // event-driven recovery from a lost peer connection where
        // we can't afford to wait the discovery_rpm floor).
        for peer_id in &actions.rendezvous_discover_force {
            if let Err(err) = self.rendezvous_discover(peer_id, true) {
                error!(%err, %peer_id, "Failed to force-discover via rendezvous");
            }
        }

        // Register with rendezvous (do this last, after relay setup)
        for peer_id in actions.rendezvous_register {
            if let Err(err) = self.rendezvous_register(&peer_id) {
                error!(%err, %peer_id, "Failed to register with rendezvous");
            }
        }
    }
}
