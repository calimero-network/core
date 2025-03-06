#[cfg(test)]
#[path = "addr_tests.rs"]
mod addr_tests;

use core::fmt;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use actix::dev::channel::{channel, AddressReceiver};
use actix::dev::EnvelopeProxy;
use actix::fut::wrap_stream;
use actix::{Actor, ActorFuture, ActorStreamExt, Addr, AsyncContext};
use futures_util::{stream, StreamExt};

struct LazyRecipientInner<A: Actor> {
    addr: Addr<A>,
    inner: ManuallyDrop<Option<AddressReceiver<A>>>,
}

pub struct LazyAddr<A: Actor> {
    inner: Arc<Mutex<LazyRecipientInner<A>>>,
}

impl<A: Actor> LazyAddr<A> {
    pub fn new() -> Self {
        let (tx, rx) = channel(16);

        let inner = LazyRecipientInner {
            addr: Addr::new(tx),
            inner: ManuallyDrop::new(Some(rx)),
        };

        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn init(&self, func: impl FnOnce(PendingMessages<'_, A>) -> Addr<A>) {
        let mut inner = self.inner.lock().expect("mutex poisoned?");

        let Some(mut rx) = inner.inner.take() else {
            panic!("attempted to initialize `LazyAddr<A>` more than once");
        };

        // we have to check if the channel is still connected, because actix's channel doesn't do that for us
        let rx = stream::poll_fn(move |cx| {
            rx.connected()
                .then(|| rx.poll_next_unpin(cx))
                .unwrap_or(Poll::Ready(None))
        });

        let pending = wrap_stream(rx)
            .map(|mut msg, act, ctx| msg.handle(act, ctx))
            .finish();

        #[expect(trivial_casts, reason = "false flag, doesn't compile without it")]
        let mut pending = Some(Box::pin(pending) as Pin<Box<_>>);

        let addr = func(PendingMessages {
            inner: &mut pending,
        });

        assert!(pending.is_none(), "pending messages were not processed");

        inner.addr = addr;
    }

    pub fn get(&self) -> Addr<A> {
        let inner = self.inner.lock().expect("mutex poisoned?");

        inner.addr.clone()
    }
}

#[must_use = "please call `finish`"]
pub struct PendingMessages<'a, A: Actor> {
    inner: &'a mut Option<Pin<Box<dyn ActorFuture<A, Output = ()>>>>,
}

impl<A: Actor> fmt::Debug for PendingMessages<'_, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingMessages").finish()
    }
}

impl<A: Actor<Context: AsyncContext<A>>> PendingMessages<'_, A> {
    pub fn process(self, ctx: &mut A::Context) {
        let inner = self.inner.take().expect("missing `PendingMessages` future");
        let _ignored = ctx.spawn(inner);
    }
}

// pub type LazyAddr<A> = Lazy<Addr<A>>;
// pub type LazyRecipient<M> = Lazy<Recipient<M>>;

// #[derive(Debug)]
// enum LazyInner<T> {
//     Uninit(Vec<T>),
//     Init(T),
// }

// pub struct Lazy<T> {
//     inner: Arc<LazyInner<T>>,
// }

// trait Receiver {
//     type Buffer;
// }

// pub trait FromReceiver<T>: Receiver {
//     fn from_receiver(receiver: T) -> Self;
// }

// impl<A> Receiver for Addr<A>
// where
//     A: Actor,
// {
//     type Buffer = Vec<Box<dyn Message<Result = ()>>>;
// }

// impl<A: Actor> FromReceiver<Addr<A>> for Addr<A> {
//     fn from_receiver(receiver: Addr<A>) -> Self {
//         receiver
//     }
// }

// impl<M> Receiver for Recipient<M>
// where
//     M: Message<Result: Send> + Send,
// {
//     type Buffer = Vec<M>;
// }

// impl<M> FromReceiver<Recipient<M>> for Recipient<M>
// where
//     M: Message<Result: Send> + Send,
// {
//     fn from_receiver(receiver: Recipient<M>) -> Self {
//         receiver
//     }
// }

// impl<A, M> FromReceiver<Addr<A>> for Recipient<M>
// where
//     A: Actor<Context: ToEnvelope<A, M>> + Handler<M>,
//     M: Message<Result: Send> + Send + 'static,
// {
//     fn from_receiver(receiver: Addr<A>) -> Self {
//         receiver.recipient()
//     }
// }

// impl<T> Lazy<T> {
//     pub fn new_uninit() -> Self {
//         Self {
//             inner: Arc::new(LazyInner {
//                 inner: OnceLock::new(),
//                 guard: Semaphore::const_new(1),
//             }),
//         }
//     }

//     pub async fn init<R>(&self, receiver: R)
//     where
//         T: FromReceiver<R>,
//     {
//         let receiver = T::from_receiver(receiver);

//         let _guard = self.inner.guard.acquire().await.expect("semaphore closed?");

//         self.inner
//             .inner
//             .set(receiver)
//             .ok()
//             .expect("attempted to initialize `Lazy<T>` more than once");

//         self.inner.guard.add_permits(1);
//     }

//     pub async fn get(&self) -> &T {
//         let _guard = self
//             .inner
//             .guard
//             .acquire_many(2)
//             .await
//             .expect("semaphore closed?");

//         self.inner.inner.get().expect("uninitialized `Lazy<T>`?")
//     }
// }

// impl<T> Deref for Lazy<T> {
//     type Target = T;

//     fn deref(&self) -> &Self::Target {
//         // tokio::task::spawn_blocking(|| self.get())
//     }
// }
