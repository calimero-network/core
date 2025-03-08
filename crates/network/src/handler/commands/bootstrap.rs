use actix::{Context, Handler, Message, Response};
use eyre::{eyre, Result as EyreResult};
use tokio::sync::oneshot;

use crate::NetworkManager;

#[derive(Message, Clone, Copy, Debug)]
#[rtype("EyreResult<Option<()>>")]
pub struct Bootstrap;

impl Handler<Bootstrap> for NetworkManager {
    type Result = Response<EyreResult<Option<()>>>;

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
