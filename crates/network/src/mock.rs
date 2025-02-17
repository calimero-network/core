use actix::{Actor, Context, Handler};

use crate::types::NetworkEvent;

#[derive(Default)]
pub struct NodeManagerMock;

impl Actor for NodeManagerMock {
    type Context = Context<Self>;
}

impl Handler<NetworkEvent> for NodeManagerMock {
    type Result = ();

    fn handle(&mut self, msg: NetworkEvent, _: &mut Self::Context) {
        println!("Handling network event with data: {:?}", msg);
    }
}
