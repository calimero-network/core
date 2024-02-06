use color_eyre::owo_colors::OwoColorize;
use libp2p::mdns;
use tokio::sync::oneshot;
use tracing::{debug, info};

use super::{Command, EventHandler, EventLoop};

impl EventHandler<mdns::Event> for EventLoop {
    async fn handle(&mut self, event: mdns::Event) {
        info!("{}: {:?}", "mdns".yellow(), event);

        match event {
            mdns::Event::Discovered(peers) => {
                for (peer_id, addr) in peers {
                    debug!("Discovered {} at {}", peer_id, addr);

                    let (sender, _receiver) = oneshot::channel();

                    self.handle_command(Command::Dial {
                        peer_addr: addr,
                        sender,
                    })
                    .await;
                }
            }
            _ => {}
        }
    }
}
