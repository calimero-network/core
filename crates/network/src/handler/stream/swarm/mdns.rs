use libp2p::mdns::Event;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, EventLoop, RelayedMultiaddr};
use crate::discovery::state::PeerDiscoveryMechanism;

impl EventHandler<Event> for EventLoop {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "mdns".yellow(), event);

        if let Event::Discovered(peers) = event {
            for (peer_id, addr) in peers {
                if RelayedMultiaddr::try_from(&addr).is_ok() {
                    // Skip "fake" relayed addresses to avoid OutgoingConnectionError e.g.:
                    // /ip4/192.168.1.4/udp/4001/quic-v1/p2p/12D3KooWRnt7EmBwrNALhAXAgM151MdH7Ka9tvYS91ZUqnqwpjVg/p2p-circuit/p2p/12D3KooWSUpChB4mHmZNwVV26at6ZsRo25hNBHJRmPa8zfCeT41Y
                    continue;
                }

                self.discovery
                    .state
                    .add_peer_discovery_mechanism(&peer_id, PeerDiscoveryMechanism::Mdns);

                debug!(%peer_id, %addr, "Attempting to dial discovered peer via mdns");

                if let Err(err) = self.swarm.dial(addr) {
                    error!("Failed to dial peer: {:?}", err);
                }
            }
        }
    }
}
