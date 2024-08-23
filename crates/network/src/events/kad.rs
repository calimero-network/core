use std::collections::HashSet;

use libp2p::kad::{Event, GetProvidersOk, QueryResult};
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    async fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "kad".yellow(), event);

        match event {
            Event::OutboundQueryProgressed {
                id,
                result: QueryResult::Bootstrap(result),
                ..
            } => {
                if let Some(sender) = self.pending_bootstrap.remove(&id) {
                    drop(sender.send(result.map(|_| None).map_err(Into::into)));
                }
            }
            Event::OutboundQueryProgressed {
                id,
                result: QueryResult::StartProviding(_),
                ..
            } => {
                let _ignore = self
                    .pending_start_providing
                    .remove(&id)
                    .expect("Completed query to be previously pending.")
                    .send(());
            }
            Event::OutboundQueryProgressed {
                id,
                result:
                    QueryResult::GetProviders(Ok(GetProvidersOk::FoundProviders { providers, .. })),
                ..
            } => {
                if let Some(sender) = self.pending_get_providers.remove(&id) {
                    sender.send(providers).expect("Receiver not to be dropped");

                    if let Some(mut query) = self.swarm.behaviour_mut().kad.query_mut(&id) {
                        query.finish();
                    }
                }
            }
            Event::OutboundQueryProgressed {
                id,
                result:
                    QueryResult::GetProviders(Ok(GetProvidersOk::FinishedWithNoAdditionalRecord {
                        ..
                    })),
                ..
            } => {
                if let Some(sender) = self.pending_get_providers.remove(&id) {
                    sender
                        .send(HashSet::new())
                        .expect("Receiver not to be dropped");

                    if let Some(mut query) = self.swarm.behaviour_mut().kad.query_mut(&id) {
                        query.finish();
                    }
                }
            }
            Event::InboundRequest { .. }
            | Event::ModeChanged { .. }
            | Event::OutboundQueryProgressed { .. }
            | Event::PendingRoutablePeer { .. }
            | Event::RoutablePeer { .. }
            | Event::RoutingUpdated { .. }
            | Event::UnroutablePeer { .. } => {}
        }
    }
}
