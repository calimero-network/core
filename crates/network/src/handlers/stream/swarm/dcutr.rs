use libp2p::dcutr::Event;
use libp2p_metrics::Recorder;
use owo_colors::OwoColorize;
use tracing::{debug, info, warn};

use super::{EventHandler, NetworkManager};
use crate::discovery::state::DcutrUpgradeStatus;

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        self.metrics.record(&event);
        // The libp2p DCUtR behaviour is wired up in `behaviour.rs`
        // and runs autonomously once two peers have a relay circuit
        // between them — there's no behaviour-level work to do
        // here. What we DO want is visibility into the outcome so
        // operators don't have to trawl `RUST_LOG=debug` to find
        // out whether the hole-punch upgraded a circuit-relayed
        // connection to a direct one.
        //
        // Log levels:
        //   * Initiated/RemoteInitiated → debug (infrastructural,
        //     fires for every relayed connection)
        //   * Succeeded → info (the key event ops cares about — we
        //     went direct, the relay can be released)
        //   * Failed → warn (peer is staying on the relay path —
        //     either symmetric-NAT'd or the predicted hole didn't
        //     open in time; ops needs to see this without --debug)
        //
        // In addition to the log lines we record the outcome on the
        // peer's DiscoveryState entry so `GET /admin-api/network/status`
        // can surface it as typed data instead of asking operators to
        // grep logs.
        let status = match &event.result {
            Ok(connection_id) => {
                info!(
                    "{}: direct-connection upgrade succeeded with peer {} (connection {:?})",
                    "dcutr".yellow(),
                    event.remote_peer_id,
                    connection_id
                );
                DcutrUpgradeStatus::Succeeded {
                    connection_id: *connection_id,
                }
            }
            Err(err) => {
                warn!(
                    "{}: direct-connection upgrade failed with peer {} — relay path will continue to carry traffic: {}",
                    "dcutr".yellow(),
                    event.remote_peer_id,
                    err
                );
                DcutrUpgradeStatus::Failed {
                    reason: err.to_string(),
                }
            }
        };
        self.discovery
            .state
            .record_dcutr_outcome(event.remote_peer_id, status);
        debug!("{}: {:?}", "dcutr".yellow(), event);
    }
}
