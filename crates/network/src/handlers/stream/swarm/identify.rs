use libp2p::identify::{Event, Info};
use libp2p::Multiaddr;
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

            // Branch 1: observed-address → external-address promotion.
            //
            // Only relevant for operators who configured the node with
            // `advertise_address: true` AND have at least one listen
            // address — that's the only case `self.discovery.advertise`
            // is `Some(_)`. If the peer reports it observed us on an
            // address that matches our advertised IP+port pair, we
            // treat that as confirmation that we're publicly dialable
            // there and promote it to a swarm external address.
            if let Some(advertise_address) = &self.discovery.advertise {
                let is_external_addr = observed_addr
                    .iter()
                    .fold((false, false), |(found_ip, found_port), p| {
                        let found_ip = found_ip
                            || match p {
                                Protocol::Ip4(ipv4_addr) => ipv4_addr == advertise_address.ip,
                                _ => false,
                            };

                        let found_port = found_port
                            || match p {
                                Protocol::Tcp(port) | Protocol::Udp(port) => {
                                    advertise_address.ports.contains(&port)
                                }
                                _ => false,
                            };

                        (found_ip, found_port)
                    })
                    .eq(&(true, true));

                if !self.swarm.external_addresses().any(|a| *a == observed_addr) && is_external_addr
                {
                    debug!(
                        "Current external addresses: {:?}",
                        self.swarm.external_addresses().collect::<Vec<&Multiaddr>>()
                    );
                    debug!(
                        "Add observed address to external adresses: {:?}",
                        observed_addr
                    );
                    self.swarm.add_external_address(observed_addr);
                }
            }

            // Branch 2: opportunistic relay-reservation request.
            //
            // If the just-identified peer offers `/libp2p/circuit/-
            // relay/0.2.0/hop`, kick off a reservation request against
            // it. This MUST run independently of branch 1: a node
            // configured with `advertise_address: true` can still be
            // behind a NAT (the `advertise_address.ip` field reflects
            // what `api.ipify.org` returned, not what's actually
            // dialable). Without an opportunistic reservation it would
            // sit forever assuming reachability that AutoNAT later
            // disproves — and by the time autonat reports failure,
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
                if let Err(err) = self.rendezvous_discover(&peer_id) {
                    error!(%err, "Failed to perform rendezvous discovery");
                }

                if let Err(err) = self.rendezvous_register(&peer_id) {
                    error!(%err, "Failed to update registration discovery");
                }
            }
        }
    }
}
