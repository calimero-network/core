use libp2p::rendezvous;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{discovery, EventHandler, EventLoop};

impl EventHandler<rendezvous::client::Event> for EventLoop {
    async fn handle(&mut self, event: rendezvous::client::Event) {
        debug!("{}: {:?}", "rendezvous".yellow(), event);

        match event {
            rendezvous::client::Event::Discovered {
                rendezvous_node,
                registrations,
                cookie,
            } => {
                self.discovery
                    .state
                    .update_rendezvous_cookie(&rendezvous_node, cookie);

                for registration in registrations {
                    let peer_id = registration.record.peer_id();

                    if peer_id == *self.swarm.local_peer_id() {
                        continue;
                    }

                    self.discovery.state.add_peer_discovery_mechanism(
                        &peer_id,
                        discovery::state::PeerDiscoveryMechanism::Rendezvous,
                    );

                    if self.swarm.is_connected(&peer_id)
                        || self.discovery.state.is_peer_discovered_via(
                            &peer_id,
                            discovery::state::PeerDiscoveryMechanism::Mdns,
                        )
                    {
                        continue;
                    }

                    debug!(
                        %peer_id,
                        addrs=?(registration.record.addresses()),
                        "Discovered new unconnected peer via rendezvous, attempting to dial it"
                    );

                    for address in registration.record.addresses() {
                        debug!(%peer_id, %address, "Dialing peer discovered via rendezvous");
                        if let Err(err) = self.swarm.dial(address.clone()) {
                            error!("Failed to dial peer: {:?}", err);
                        }
                    }
                }
            }
            rendezvous::client::Event::Registered {
                rendezvous_node, ..
            } => {
                self.discovery.state.update_rendezvous_registration_status(
                    &rendezvous_node,
                    discovery::state::RendezvousRegistrationStatus::Registered,
                );

                if let Some(peer_info) = self.discovery.state.get_peer_info(&rendezvous_node) {
                    if peer_info
                        .rendezvous()
                        .and_then(|info| info.cookie())
                        .is_none()
                    {
                        debug!(%rendezvous_node, "Discovering peers via rendezvous after registration");
                        if let Err(err) = self.rendezvous_discover(&rendezvous_node) {
                            error!(%err, "Failed to run rendezvous discovery after registration");
                        }
                    }
                }
            }
            rendezvous::client::Event::DiscoverFailed {
                rendezvous_node,
                namespace,
                error,
            } => {
                error!(?rendezvous_node, ?namespace, error_code=?error, "Rendezvous discovery failed");
            }
            rendezvous::client::Event::RegisterFailed {
                rendezvous_node,
                namespace,
                error,
            } => {
                error!(?rendezvous_node, ?namespace, error_code=?error, "Rendezvous registration failed");
            }
            rendezvous::client::Event::Expired { peer } => {
                self.discovery.state.update_rendezvous_registration_status(
                    &peer,
                    discovery::state::RendezvousRegistrationStatus::Expired,
                );
            }
        }
    }
}
