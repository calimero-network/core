#[cfg(test)]
#[path = "recipient_tests.rs"]
mod recipient_tests;

use std::ops::Deref;

use actix::dev::ToEnvelope;
use actix::{Actor, Addr, Handler, Message, Recipient};

#[derive(Debug, Clone, Default)]
pub struct LazyRecipient<M>(Option<Recipient<M>>)
where
    M: Message<Result: Send> + Send;

impl<M> LazyRecipient<M>
where
    M: Message<Result: Send> + Send,
{
    pub const fn new_uninit() -> Self {
        Self(None)
    }

    pub fn init<A>(&mut self, addr: Addr<A>)
    where
        A: Actor<Context: ToEnvelope<A, M>> + Handler<M>,
        M: 'static,
    {
        self.0 = Some(addr.recipient());
    }
}

impl<T> Deref for LazyRecipient<T>
where
    T: Message<Result: Send> + Send,
{
    type Target = Recipient<T>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect(&format!(
            "attempted illegal use of uninitialized `Recipient<{}>`",
            std::any::type_name::<T>()
        ))
    }
}
