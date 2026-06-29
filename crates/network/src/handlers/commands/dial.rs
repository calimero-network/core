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
                //
                // NB: this `Ok(())` means "a dial to this peer is already in
                // flight", not "the dial succeeded". The in-flight dial owns
                // the only sender, so we can't subscribe to its result here
                // without a broadcast/clone; until that's wired up, a caller
                // hitting this branch gets a spurious success even if the
                // real dial later fails.
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

        Response::fut(async move {
            // The sender lives in `pending_dial` and is normally either fired
            // by `ConnectionEstablished`/`OutgoingConnectionError` or carried
            // until then. If it is dropped without sending — e.g. the manager
            // is shutting down and tears down the swarm — `recv` errors out.
            // Surface that as a dial error rather than panicking the actor.
            match receiver.await {
                Ok(result) => result,
                Err(_) => Err(eyre!("dial cancelled before completion")),
            }
        })
    }
}
