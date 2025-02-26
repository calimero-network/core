use actix::{Context, Handler, Message, ResponseFuture};
use eyre::{eyre, Result as EyreResult};
use tokio::sync::oneshot;

use crate::NetworkManager;

#[derive(Message, Clone, Copy, Debug)]
#[rtype("EyreResult<Option<()>>")]
pub struct Bootstrap;

impl Handler<Bootstrap> for NetworkManager {
    type Result = ResponseFuture<EyreResult<Option<()>>>;

    fn handle(
        &mut self,
        _msg: Bootstrap,
        _ctx: &mut Context<Self>,
    ) -> ResponseFuture<EyreResult<Option<()>>> {
        let (sender, receiver) = oneshot::channel();

        match self.swarm.behaviour_mut().kad.bootstrap() {
            Ok(query_id) => {
                let _ignored = self.pending_bootstrap.insert(query_id, sender);
            }
            Err(err) => {
                return Box::pin(async { Err(eyre!(err)) });
            }
        }

        Box::pin(async { receiver.await.expect("Sender not to be dropped.") })
    }
}
