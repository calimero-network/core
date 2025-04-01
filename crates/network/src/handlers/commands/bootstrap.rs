use actix::{Context, Handler, Message, Response};
use calimero_network_primitives::messages::Bootstrap;
use eyre::eyre;
use tokio::sync::oneshot;

use crate::NetworkManager;

impl Handler<Bootstrap> for NetworkManager {
    type Result = Response<<Bootstrap as Message>::Result>;

    fn handle(&mut self, _msg: Bootstrap, _ctx: &mut Context<Self>) -> Self::Result {
        let (sender, receiver) = oneshot::channel();

        match self.swarm.behaviour_mut().kad.bootstrap() {
            Ok(query_id) => {
                let _ignored = self.pending_bootstrap.insert(query_id, sender);
            }
            Err(err) => {
                return Response::reply(Err(eyre!(err)));
            }
        }

        Response::fut(async { receiver.await.expect("Sender not to be dropped.") })
    }
}
