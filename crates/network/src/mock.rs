use actix::{Actor, Context, Handler};

use crate::types::NetworkEvent;

#[derive(Default)]
pub struct EventReceiverMock;

impl Actor for EventReceiverMock {
    type Context = Context<Self>;
}

impl Handler<NetworkEvent> for EventReceiverMock {
    type Result = ();

    fn handle(&mut self, msg: NetworkEvent, _: &mut Self::Context) {
        println!("Handling network event with data: {msg:?}");
    }
}
