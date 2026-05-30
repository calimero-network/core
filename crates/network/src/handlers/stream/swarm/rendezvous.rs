use libp2p::rendezvous::client::Event;
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use owo_colors::OwoColorize;
use tracing::{debug, error, warn};

use super::{EventHandler, NetworkManager};
use crate::discovery::state::{PeerDiscoveryMechanism, RendezvousRegistrationStatus};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "rendezvous".yellow(), event);

        match event {
            Event::Discovered {
                rendezvous_node,
                registrations,
                cookie,
            } => {
                self.discovery
                    .state
                    .update_rendezvous_cookie(&rendezvous_node, &cookie);

                for registration in registrations {
                    let peer_id = registration.record.peer_id();

                    if peer_id == *self.swarm.local_peer_id() {
                        continue;
                    }

                    self.discovery
                        .state
                        .add_peer_discovery_mechanism(&peer_id, PeerDiscoveryMechanism::Rendezvous);

                    if self.swarm.is_connected(&peer_id)
                        || self
                            .discovery
                            .state
                            .is_peer_discovered_via(&peer_id, PeerDiscoveryMechanism::Mdns)
                    {
                        continue;
                    }

                    let addrs = registration.record.addresses().to_vec();
                    debug!(
                        %peer_id,
                        ?addrs,
                        "Discovered new unconnected peer via rendezvous, attempting to dial it"
                    );

                    // Dial the peer ONCE, deduped at the swarm level.
                    // `DisconnectedAndNotDialing` makes libp2p skip the
                    // attempt if we are already connected or a dial to
                    // this peer is already in flight. Without it, every
                    // rendezvous-discovery cycle re-dials the same peers
                    // — and while the node is peerless the tick discovers
                    // every interval (~15s), so on the global rendezvous
                    // namespace this fans relayed circuit dials past the
                    // relay client's in-flight cap
                    // (MAX_CONCURRENT_STREAMS_PER_CONNECTION = 10),
                    // producing the "Dropping in-flight connect request
                    // because we are at capacity" storm. Per-peer dedup
                    // bounds the fan-out to one in-flight dial per peer.
                    let opts = DialOpts::peer_id(peer_id)
                        .condition(PeerCondition::DisconnectedAndNotDialing)
                        .addresses(addrs)
                        .build();
                    if let Err(err) = self.swarm.dial(opts) {
                        // Benign when the condition isn't met (already
                        // connected / dialing) or the record carried no
                        // addresses — debug, not error.
                        debug!(
                            %peer_id,
                            ?err,
                            "Did not dial rendezvous-discovered peer (already connected/dialing or no usable address)"
                        );
                    }
                }
            }
            Event::Registered {
                rendezvous_node, ..
            } => {
                // Only accept the registration if we're still expecting it (status is Requested).
                // If status was changed to Expired (e.g., we became unreachable while the
                // registration was in-flight), ignore this late response to avoid registering
                // with stale addresses.
                let current_status = self
                    .discovery
                    .state
                    .get_peer_info(&rendezvous_node)
                    .and_then(|info| info.rendezvous())
                    .map(|info| info.registration_status());

                if current_status != Some(RendezvousRegistrationStatus::Requested) {
                    debug!(
                        %rendezvous_node,
                        ?current_status,
                        "Ignoring late registration response - status is no longer Requested"
                    );
                    return;
                }

                self.discovery.state.update_rendezvous_registration_status(
                    &rendezvous_node,
                    RendezvousRegistrationStatus::Registered,
                );

                if let Some(peer_info) = self.discovery.state.get_peer_info(&rendezvous_node) {
                    if peer_info
                        .rendezvous()
                        .and_then(|info| info.cookie())
                        .is_none()
                    {
                        debug!(%rendezvous_node, "Discovering peers via rendezvous after registration");
                        if let Err(err) = self.rendezvous_discover(&rendezvous_node, false) {
                            error!(%err, "Failed to run rendezvous discovery after registration");
                        }
                    }
                }
            }
            Event::DiscoverFailed {
                rendezvous_node,
                namespace,
                error,
            } => {
                warn!(?rendezvous_node, ?namespace, error_code=?error, "Rendezvous discovery failed");
            }
            Event::RegisterFailed {
                rendezvous_node,
                namespace,
                error,
            } => {
                error!(?rendezvous_node, ?namespace, error_code=?error, "Rendezvous registration failed");
            }
            Event::Expired { peer } => {
                self.discovery.state.update_rendezvous_registration_status(
                    &peer,
                    RendezvousRegistrationStatus::Expired,
                );

                if let Some(nominated_peer) = self.discovery.state.find_new_rendezvous_peer() {
                    if self.swarm.is_connected(&nominated_peer) {
                        if let Err(err) = self.rendezvous_register(&nominated_peer) {
                            error!(%err, "Failed to register with nominated rendezvous peer");
                        }
                    } else {
                        debug!(%nominated_peer, "Dialing nominated rendezvous peer");
                        if let Err(err) = self.swarm.dial(nominated_peer) {
                            error!(%err, "Failed to dial nominated rendezvous peer");
                        }
                    }
                } else {
                    debug!("Couldn't find new peer to nominate for rendezvous registration.");
                }
            }
        }
    }
}
