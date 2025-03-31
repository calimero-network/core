use actix::{Context, Handler};
use calimero_network_primitives::messages::ListenOn;
use eyre::Result as EyreResult;

use crate::NetworkManager;

impl Handler<ListenOn> for NetworkManager {
    type Result = EyreResult<()>;

    fn handle(&mut self, ListenOn(addr): ListenOn, _ctx: &mut Context<Self>) -> EyreResult<()> {
        let _ignored = self.swarm.listen_on(addr)?;

        Ok(())
    }
}
