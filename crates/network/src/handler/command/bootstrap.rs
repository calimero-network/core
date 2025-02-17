use actix::{Context, Handler, Message, ResponseFuture};
use eyre::{eyre, Result as EyreResult};
use tokio::sync::oneshot;

use crate::EventLoop;

#[derive(Message, Clone, Copy, Debug)]
#[rtype("EyreResult<Option<()>>")]
pub struct Bootstrap;

impl Handler<Bootstrap> for EventLoop {
    type Result = ResponseFuture<EyreResult<Option<()>>>;

    fn handle(
        &mut self,
        _msg: Bootstrap,
        _ctx: &mut Context<Self>,
    ) -> ResponseFuture<EyreResult<Option<()>>> {
        let (sender, receiver) = oneshot::channel();

        match self.swarm.behaviour_mut().kad.bootstrap() {
            Ok(query_id) => {
                drop(self.pending_bootstrap.insert(query_id, sender));
            }
            Err(err) => {
                return Box::pin(async move { Err(eyre!(err)) });
            }
        }

        Box::pin(async move { receiver.await.expect("Sender not to be dropped.") })
    }
}
