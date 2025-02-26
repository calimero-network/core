use actix::{Context, Handler, Message};
use eyre::Result as EyreResult;
use multiaddr::Multiaddr;

use crate::NetworkManager;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<()>")]
pub struct ListenOn(Multiaddr);

impl From<Multiaddr> for ListenOn {
    fn from(addr: Multiaddr) -> Self {
        Self(addr)
    }
}
impl Handler<ListenOn> for NetworkManager {
    type Result = EyreResult<()>;

    fn handle(&mut self, ListenOn(addr): ListenOn, _ctx: &mut Context<Self>) -> EyreResult<()> {
        let _ignored = self.swarm.listen_on(addr)?;

        Ok(())
    }
}
