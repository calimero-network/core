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
                match result {
                    TestResult::Reachable { addr } => {
                        info!(
                            "✓ AutoNAT: Address {} confirmed reachable as {}",
                            tested_addr, addr
                        );
                        // Swarm will emit ExternalAddrConfirmed - let that handle it
                    }
                    TestResult::Failed { error } => {
                        warn!("✗ AutoNAT: Address {} test failed: {}", tested_addr, error);
                        // This address failed, but it might not have been in our
                        // confirmed set anyway. Let swarm events drive state.
                    }
                }
            }
            Event::Server {
                client,
                data_amount,
                ..
            } => {
                info!("Served AutoNAT test for {} ({} bytes)", client, data_amount);
            }
            Event::ModeChanged { old_mode, new_mode } => {
                info!("AutoNAT mode: {:?} → {:?}", old_mode, new_mode);
            }
            Event::PeerHasServerSupport { peer_id } => {
                info!("Peer {} has AutoNAT server support", peer_id);
                self.discovery.state.add_autonat_server(&peer_id);
            }
        }
    }
}
