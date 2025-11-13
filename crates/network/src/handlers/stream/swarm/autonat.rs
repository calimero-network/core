use owo_colors::OwoColorize;
use tracing::{debug, error, info, warn};

use super::EventHandler;
use crate::autonat::{Event, TestResult};
use crate::NetworkManager;

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "autonat".yellow(), event);
        match event {
            Event::Client {
                tested_addr,
                result,
                ..
            } => match result {
                TestResult::Reachable { addr } => {
                    info!(
                        "✓ Address {} is reachable (confirmed as {})",
                        tested_addr, addr
                    );

                    // Mark OUR address as reachable (the server tested it for us)
                    self.discovery
                        .state
                        .add_confirmed_external_address(&tested_addr);

                    // Since public, we can now become an AutoNAT server
                    // and help other nodes probe their own external addresses
                    if let Err(err) = self.swarm.behaviour_mut().autonat.enable_server() {
                        error!(%err, "Failed to enable AutoNAT server");
                    }

                    // If we now have confirmed reachable addresses, we're likely public
                    // Trigger rendezvous registration if we have rendezvous peers
                    let rendezvous_peers: Vec<_> =
                        self.discovery.state.get_rendezvous_peer_ids().collect();

                    if !rendezvous_peers.is_empty() {
                        info!("We appear to be publicly reachable, registering with rendezvous");
                        for peer_id in rendezvous_peers {
                            if let Err(err) = self.rendezvous_register(&peer_id) {
                                error!(%err, %peer_id, "Failed to register with rendezvous");
                            }
                        }
                    }
                }
                TestResult::Failed { error } => {
                    warn!("✗ Address {} test failed: {}", tested_addr, error);

                    // Mark this address as not confirmed
                    self.discovery
                        .state
                        .remove_confirmed_external_address(&tested_addr);

                    // Check if we have ANY confirmed reachable addresses left
                    let has_any_reachable = self.discovery.state.has_confirmed_external_addresses();

                    if !has_any_reachable {
                        info!("No confirmed reachable addresses, likely behind NAT");

                        // We're no longer public, we can't fulfill our role as an AutoNAT server
                        if let Err(err) = self.swarm.behaviour_mut().autonat.disable_server() {
                            error!(%err, "Failed to disable AutoNAT server");
                        }

                        // We're now behind NAT and consider ourselves unreachable
                        let rendezvous_peers: Vec<_> =
                            self.discovery.state.get_rendezvous_peer_ids().collect();
                        // Need to unregister from rendezvous
                        if !rendezvous_peers.is_empty() {
                            info!("We appear to be publicly unreachable, unregistering from rendezvous");
                            for peer_id in rendezvous_peers {
                                if let Err(err) = self.rendezvous_unregister(&peer_id) {
                                    error!(%err, %peer_id, "Failed to unregister from rendezvous");
                                }
                            }
                        }

                        // We appear to be private - set up relay reservations
                        let relay_peers: Vec<_> =
                            self.discovery.state.get_relay_peer_ids().collect();

                        for peer_id in relay_peers {
                            if let Err(err) = self.create_relay_reservation(&peer_id) {
                                error!(%err, %peer_id, "Failed to create relay reservation");
                            }
                        }

                        // Still try to discover peers through rendezvous
                        let rendezvous_peers: Vec<_> =
                            self.discovery.state.get_rendezvous_peer_ids().collect();

                        for peer_id in rendezvous_peers {
                            if let Err(err) = self.rendezvous_discover(&peer_id) {
                                error!(%err, %peer_id, "Failed to perform rendezvous discovery");
                            }
                            // And register with new relay address
                            if let Err(err) = self.rendezvous_register(&peer_id) {
                                error!(%err, %peer_id, "Failed to register with rendezvous");
                            }
                        }
                    }
                }
            },
            Event::Server {
                client,
                data_amount,
                ..
            } => {
                info!("Served test request for {} ({} bytes)", client, data_amount);
            }
            Event::ModeChanged { old_mode, new_mode } => {
                info!("AutoNAT mode changed: {:?} → {:?}", old_mode, new_mode);
            }
            Event::PeerHasServerSupport { peer_id } => {
                info!("Discovered peer {} has AutoNAT server support", peer_id);

                let _ = self.discovery.state.add_autonat_server(&peer_id);
            }
        }
    }
}
