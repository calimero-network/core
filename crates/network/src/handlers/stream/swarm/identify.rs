use libp2p::identify::{Event, Info};
use libp2p_metrics::Recorder;
use multiaddr::Protocol;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, NetworkManager};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        self.metrics.record(&event);
        debug!("{}: {:?}", "identify".yellow(), event);

        if let Event::Received {
            peer_id,
            info:
                Info {
                    listen_addrs,
                    observed_addr,
                    protocols,
                    ..
                },
            ..
        } = event
        {
            self.discovery
                .state
                .update_peer_protocols(&peer_id, &protocols);

            // Mirror the peer's declared listen addresses into our local
            // address book. Without this, we only ever learn an address
            // when a dial to it succeeds and ConnectionEstablished fires —
            // so a peer that advertises three addresses but only one
            // happens to work first would shrink to one entry in our
            // book, leaving us no fallback when that address dies.
            //
            // Filter out relayed addresses (`/p2p-circuit/`); they aren't
            // useful as direct-dial entries and the rendezvous-tick redial
            // loop would just waste attempts on them. Filter out our own
            // observed address (sometimes echoed back); it's about us, not
            // the peer.
            for addr in &listen_addrs {
                let is_relayed = addr.iter().any(|p| matches!(p, Protocol::P2pCircuit));
                if !is_relayed && addr != &observed_addr {
                    self.discovery.state.add_peer_addr(peer_id, addr);
                }
            }

            // External-address promotion is left entirely to libp2p:
            // `identify` reports each peer-observed address as a
            // `NewExternalAddrCandidate`, and the AutoNAT v2 client
            // probes those candidates and emits `ExternalAddrConfirmed`
            // only for addresses that are actually dial-back reachable
            // (handled in the swarm event handler). We deliberately do
            // NOT `add_external_address(observed_addr)` here: a single
            // peer's observation is unverified, and asserting it would
            // bypass AutoNAT — advertising an address we can't prove is
            // reachable. Operators who want a deterministic external
            // address set it via `discovery.external_address`, which is
            // seeded directly into the swarm at init.

            // Opportunistic relay-reservation request.
            //
            // If the just-identified peer offers `/libp2p/circuit/-
            // relay/0.2.0/hop`, kick off a reservation request against
            // it. This runs regardless of external-address state: a node
            // can still be behind a NAT until AutoNAT confirms a direct
            // address. Without an opportunistic reservation it would sit
            // unreachable until AutoNAT reports failure — and by then
            // there's no event-driven path that retries relay setup.
            //
            // Cost is bounded: `create_relay_reservation` short-
            // circuits via `is_relay_reservation_required(limit)` once
            // `relay_config.registrations_limit` accepted+pending
            // reservations are already in flight. So a publicly-
            // reachable node ends up with at most `registrations_limit`
            // (default: 3) idle relay slots — a fine price for the
            // fallback path being warm if the direct dial later
            // breaks.
            if self.discovery.state.is_peer_relay(&peer_id) {
                if let Err(err) = self.create_relay_reservation(&peer_id) {
                    error!(%err, "Failed to handle relay reservation");
                }
            }

            if self.discovery.state.is_peer_rendezvous(&peer_id) {
                if let Err(err) = self.rendezvous_discover(&peer_id, false) {
                    error!(%err, "Failed to perform rendezvous discovery");
                }

                if let Err(err) = self.rendezvous_register(&peer_id) {
                    error!(%err, "Failed to update registration discovery");
                }
            }
        }
    }
}
