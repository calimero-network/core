use std::collections::hash_map::Entry;

use actix::{Context, Handler, Message, Response};
use calimero_network_primitives::messages::Dial;
use eyre::eyre;
use multiaddr::Protocol;
use tokio::sync::oneshot;

use crate::NetworkManager;

impl Handler<Dial> for NetworkManager {
    type Result = Response<<Dial as Message>::Result>;

    fn handle(&mut self, Dial(mut peer_addr): Dial, _ctx: &mut Context<Self>) -> Self::Result {
        let Some(Protocol::P2p(peer_id)) = peer_addr.pop() else {
            return Response::reply(Err(eyre!("No peer ID in address: {}", peer_addr)));
        };

        let (sender, receiver) = oneshot::channel();

        match self.pending_dial.entry(peer_id) {
            Entry::Occupied(_) => {
                // todo! await the existing receiver
                return Response::reply(Ok(()));
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
                        return Response::reply(Err(eyre!(e)));
                    }
                }
            }
        }

        Response::fut(async { receiver.await.expect("Sender not to be dropped.") })
    }
}
