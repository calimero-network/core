use std::collections::hash_map::Entry;

use actix::{Context, Handler, Message, ResponseFuture};
use eyre::{eyre, Result as EyreResult};
use multiaddr::{Multiaddr, Protocol};
use tokio::sync::oneshot;

use crate::NetworkManager;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<Option<()>>")]
pub struct Dial(Multiaddr);

impl From<Multiaddr> for Dial {
    fn from(addr: Multiaddr) -> Self {
        Self(addr)
    }
}

impl Handler<Dial> for NetworkManager {
    type Result = ResponseFuture<EyreResult<Option<()>>>;

    fn handle(
        &mut self,
        Dial(mut peer_addr): Dial,
        _ctx: &mut Context<Self>,
    ) -> ResponseFuture<EyreResult<Option<()>>> {
        let Some(Protocol::P2p(peer_id)) = peer_addr.pop() else {
            return Box::pin(async move { Err(eyre!("No peer ID in address: {}", peer_addr)) });
        };

        let (sender, receiver) = oneshot::channel();

        match self.pending_dial.entry(peer_id) {
            Entry::Occupied(_) => {
                return Box::pin(async { Ok(None) });
            }
            Entry::Vacant(entry) => {
                let _ignored = self
                    .swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&peer_id, peer_addr.clone());

                match self.swarm.dial(peer_addr) {
                    Ok(()) => {
                        let _ignored = entry.insert(sender);
                    }
                    Err(e) => {
                        return Box::pin(async { Err(eyre!(e)) });
                    }
                }
            }
        }

        Box::pin(async { receiver.await.expect("Sender not to be dropped.") })
    }
}
