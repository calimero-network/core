use libp2p::identify::{Event, Info};
use libp2p_metrics::Recorder;
use multiaddr::{Multiaddr, Protocol};
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, NetworkManager};

/// Whether a peer-advertised (identify-pushed) address is worth keeping as a
/// direct-dial candidate.
///
/// Rejects addresses no remote peer can legitimately be reachable at —
/// loopback, unspecified (`0.0.0.0` / `::`), and link-local — since these are
/// unverified, peer-controlled input. Private (RFC-1918) and IPv6 unique-local
/// (`fc00::/7`) ranges are kept: they're valid for same-network / overlay
/// deployments. Addresses with no IP component (e.g. a DNS multiaddr) pass
/// through unchanged.
///
/// Unlike `is_seedable_external_address` in `lib.rs`, this filter stays silent
/// on private/unique-local ranges rather than warning. That counterpart vets
/// *our own* operator-configured external address, where a private range is a
/// likely misconfiguration worth flagging; here the addresses belong to other
/// peers, so a per-peer warning would be noise, not an actionable signal.
fn is_dialable_advertised_addr(addr: &Multiaddr) -> bool {
    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip) => {
                if ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() {
                    return false;
                }
            }
            Protocol::Ip6(ip) => {
                if ip.is_loopback() || ip.is_unspecified() {
                    return false;
                }
                // Link-local fe80::/10 — never routable off-link.
                if (ip.segments()[0] & 0xffc0) == 0xfe80 {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

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
            //
            // Also drop addresses a remote peer can never legitimately be
            // reachable at — loopback, unspecified, and link-local. These are
            // peer-controlled, unverified input: without the filter a
            // malicious or misconfigured peer could push entries that seed our
            // dial book with wasted attempts, or point dials at our own
            // loopback/link-local interfaces. Private (RFC-1918) and IPv6
            // unique-local ranges are kept — they're legitimate for
            // same-network / overlay deployments.
            for addr in &listen_addrs {
                let is_relayed = addr.iter().any(|p| matches!(p, Protocol::P2pCircuit));
                if !is_relayed && addr != &observed_addr && is_dialable_advertised_addr(addr) {
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

#[cfg(test)]
mod tests {
    use super::is_dialable_advertised_addr;

    fn dialable(s: &str) -> bool {
        is_dialable_advertised_addr(&s.parse().expect("valid multiaddr"))
    }

    #[test]
    fn keeps_routable_and_overlay_addresses() {
        assert!(dialable("/ip4/203.0.113.7/tcp/2428"));
        assert!(dialable("/ip6/2001:db8::1/tcp/2428"));
        // Private / unique-local are legitimate for overlays.
        assert!(dialable("/ip4/10.0.0.5/tcp/2428"));
        assert!(dialable("/ip4/192.168.1.20/udp/2428/quic-v1"));
        assert!(dialable("/ip6/fd00::1/tcp/2428"));
        // No IP component (DNS) passes through.
        assert!(dialable("/dns4/node.example.com/tcp/2428"));
    }

    #[test]
    fn drops_unreachable_advertised_addresses() {
        assert!(!dialable("/ip4/127.0.0.1/tcp/2428"));
        assert!(!dialable("/ip4/0.0.0.0/tcp/2428"));
        assert!(!dialable("/ip4/169.254.1.1/tcp/2428"));
        assert!(!dialable("/ip6/::1/tcp/2428"));
        assert!(!dialable("/ip6/::/tcp/2428"));
        assert!(!dialable("/ip6/fe80::1/tcp/2428"));
    }
}
