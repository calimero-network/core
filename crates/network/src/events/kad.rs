use std::collections::HashSet;

use color_eyre::owo_colors::OwoColorize;
use libp2p::kad;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<kad::Event> for EventLoop {
    async fn handle(&mut self, event: kad::Event) {
        debug!("{}: {:?}", "kad".yellow(), event);

        match event {
            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::Bootstrap(result),
                ..
            } => {
                if let Some(sender) = self.pending_bootstrap.remove(&id) {
                    let _ = sender.send(result.map(|_| None).map_err(Into::into));
                }
            }
            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::StartProviding(_),
                ..
            } => {
                let _ = self
                    .pending_start_providing
                    .remove(&id)
                    .expect("Completed query to be previously pending.")
                    .send(());
            }
            kad::Event::OutboundQueryProgressed {
                id,
                result:
                    kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                        providers,
                        ..
                    })),
                ..
            } => {
                if let Some(sender) = self.pending_get_providers.remove(&id) {
                    sender.send(providers).expect("Receiver not to be dropped");

                    self.swarm
                        .behaviour_mut()
                        .kad
                        .query_mut(&id)
                        .unwrap()
                        .finish();
                }
            }
            kad::Event::OutboundQueryProgressed {
                id,
                result:
                    kad::QueryResult::GetProviders(Ok(
                        kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. },
                    )),
                ..
            } => {
                if let Some(sender) = self.pending_get_providers.remove(&id) {
                    sender
                        .send(HashSet::new())
                        .expect("Receiver not to be dropped");

                    self.swarm
                        .behaviour_mut()
                        .kad
                        .query_mut(&id)
                        .unwrap()
                        .finish();
                }
            }
            _ => {}
        }
    }
}
