use actix::StreamHandler;
use tokio::time::Instant;
use tracing::{debug, error};

use crate::NetworkManager;

#[derive(Copy, Clone, Debug)]
pub struct RendezvousTick;

impl From<Instant> for RendezvousTick {
    fn from(_: Instant) -> Self {
        Self
    }
}

impl StreamHandler<RendezvousTick> for NetworkManager {
    fn started(&mut self, _ctx: &mut Self::Context) {
        debug!("started rendezvous tick stream");
    }

    fn handle(&mut self, _tick: RendezvousTick, _ctx: &mut Self::Context) {
        // Post-restart / post-partition recovery: if we currently hold
        // zero connections to *regular* (non-relay, non-rendezvous)
        // peers, the application overlay is unreachable and the
        // `discovery_rpm` throttle floor (default 120s) is far longer
        // than the sync-recovery budget. Bypass the throttle until we
        // regain a regular peer so rediscovery runs every tick
        // (`discovery_interval`, ~15s) instead of once per floor.
        //
        // This covers the fresh-restart case the #2469
        // `on_regular_peer_disconnected` force-rediscovery misses: that
        // path is keyed on `SwarmEvent::ConnectionClosed`, which never
        // fires after a restart (no connection was ever open this
        // process), so without this the node parks behind the throttle
        // while the sync layer reports "No peers to sync with".
        let connected: Vec<libp2p::PeerId> = self.swarm.connected_peers().copied().collect();
        let peerless = !self
            .discovery
            .state
            .has_regular_connected_peer(connected.iter());

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

            if !peerless
                && peer_info.is_rendezvous_discover_throttled(
                    self.discovery.rendezvous_config.discovery_rpm,
                )
            {
                continue;
            }

            if !self.swarm.is_connected(&peer_id) {
                for addr in peer_info.addrs().cloned() {
                    if let Err(err) = self.swarm.dial(addr) {
                        error!(%err, "Failed to dial rendezvous peer");
                    }
                }
            } else if let Err(err) = self.rendezvous_discover(&peer_id, peerless) {
                error!(%err, "Failed to perform rendezvous discover");
            }
        }
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        debug!("finished rendezvous tick stream");
    }
}
