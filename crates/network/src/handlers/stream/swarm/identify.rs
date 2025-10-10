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

            // TODO: Revist AutoNAT protocol implementation
            // if self.discovery.state.is_peer_autonat(&peer_id) {
            //     if let Err(err) = self.add_autonat_server(&peer_id) {
            //         error!(%err, "Failed to add autonat server");
            //     };
            // }

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
            } else {
                if self.discovery.state.is_peer_relay(&peer_id) {
                    if let Err(err) = self.create_relay_reservation(&peer_id) {
                        error!(%err, "Failed to handle relay reservation");
                    };
                }
            }

            if self.discovery.state.is_peer_rendezvous(&peer_id) {
                if let Err(err) = self.rendezvous_discover(&peer_id) {
                    error!(%err, "Failed to perform rendezvous discovery");
                };

                if let Err(err) = self.rendezvous_register(&peer_id) {
                    error!(%err, "Failed to update registration discovery");
                };
            }
        }
    }
}
