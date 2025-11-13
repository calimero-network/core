use super::EventHandler;
use crate::autonat::{Event, TestResult};
use crate::NetworkManager;
use owo_colors::OwoColorize;
use tracing::{debug, info, warn};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "autonat".yellow(), event);

        match event {
            Event::Client {
                tested_addr,
                result,
                ..
            } => {
                let actions = match result {
                    TestResult::Reachable { addr } => {
                        info!(
                            "✓ Address {} is reachable (confirmed as {})",
                            tested_addr, addr
                        );
                        self.discovery.state.on_address_reachable(&tested_addr)
                    }
                    TestResult::Failed { error } => {
                        warn!("✗ Address {} test failed: {}", tested_addr, error);
                        self.discovery.state.on_address_unreachable(&tested_addr)
                    }
                };

                self.execute_reachability_actions(actions);
            }
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
