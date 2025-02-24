use actix::{Message, StreamHandler};
use tracing::error;

use crate::NetworkManager;

#[derive(Message)]
#[rtype(result = "()")]
pub struct FromTick;

impl StreamHandler<FromTick> for NetworkManager {
    fn started(&mut self, _ctx: &mut Self::Context) {
        println!("started receiving swarm messages");
    }

    fn handle(&mut self, _tick: FromTick, _ctx: &mut Self::Context) {
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

            if peer_info
                .is_rendezvous_discover_throttled(self.discovery.rendezvous_config.discovery_rpm)
            {
                continue;
            }

            if !self.swarm.is_connected(&peer_id) {
                for addr in peer_info.addrs().cloned() {
                    if let Err(err) = self.swarm.dial(addr) {
                        error!(%err, "Failed to dial rendezvous peer");
                    }
                }
            } else if let Err(err) = self.rendezvous_discover(&peer_id) {
                error!(%err, "Failed to perform rendezvous discover");
            }
        }
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        println!("finished receiving swarm messages");
    }
}
