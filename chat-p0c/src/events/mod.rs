use super::*;

mod identify;
mod kad;
mod mdns;
mod ping;
mod relay;

pub trait EventHandler<E> {
    async fn handle(&mut self, event: E);
}
