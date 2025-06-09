use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::ListenOn;

use crate::NetworkManager;

impl Handler<ListenOn> for NetworkManager {
    type Result = <ListenOn as Message>::Result;

    fn handle(&mut self, ListenOn(addr): ListenOn, _ctx: &mut Context<Self>) -> Self::Result {
        let _ignored = self.swarm.listen_on(addr)?;

        Ok(())
    }
}
