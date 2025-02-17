use actix::{Context, Handler, Message};
use eyre::{bail, Result as EyreResult};
use multiaddr::Multiaddr;

use crate::EventLoop;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<()>")]
pub struct ListenOn(Multiaddr);

impl Handler<ListenOn> for EventLoop {
    type Result = EyreResult<()>;

    fn handle(&mut self, ListenOn(addr): ListenOn, _ctx: &mut Context<Self>) -> EyreResult<()> {
        match self.swarm.listen_on(addr) {
            Ok(_) => Ok(()),
            Err(e) => bail!(e),
        }
    }
}
