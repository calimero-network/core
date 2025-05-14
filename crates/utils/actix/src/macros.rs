#[cfg(test)]
#[path = "macros_tests.rs"]
mod macros_tests;

#[doc(hidden)]
pub mod __private {
    pub use core::marker::Send;
    pub use core::ops::{DerefMut, FnOnce};
    pub use core::pin::pin;
    pub use core::ptr;
    pub use core::task::Poll;
    pub use std::boxed::Box;

    pub use actix::dev::channel;
    use actix::dev::ToEnvelope;
    pub use actix::{
        Actor, Addr, ArbiterHandle, AsyncContext, Context, Handler, Message, StreamHandler,
    };
    pub use futures_util::future::poll_fn;
    pub use futures_util::{FutureExt, Stream, StreamExt};
    pub use paste::paste;
    pub use tokio::task;

    pub use crate::actor;

    pub trait ActorSpawn: Actor {
        fn spawn(self, ctx: Self::Context);
    }

    #[derive(Debug, Message)]
    #[rtype("()")]
    pub enum FromStreamInner<T> {
        Started,
        Finished,
        Value(T),
    }

    impl<T: Send> FromStreamInner<T> {
        pub fn send<A>(self, addr: &Addr<A>)
        where
            A: Actor<Context: ToEnvelope<A, Self>> + Handler<Self> + StreamHandler<T>,
        {
            addr.do_send(self);
        }
    }

    impl FromStreamInner<()> {
        pub const fn scoped_into<U, S>(this: FromStreamInner<U>, _: &S) -> FromStreamInner<U>
        where
            S: Stream<Item: Into<U>>,
        {
            this
        }

        pub const fn scoped_identity<S: Stream>(
            this: FromStreamInner<S::Item>,
            _: &S,
        ) -> FromStreamInner<S::Item> {
            this
        }
    }
}

#[macro_export]
macro_rules! actor {
    (Self $($rest:tt),*) => {
        compile_error!("`Self` is not allowed")
    };
    ($actor:ty $(=> {$(.$stream:ident $(as $type:ty)?),+ $(,)?})?) => {
        fn start(self) -> $crate::macros::__private::Addr<Self> {
            use $crate::macros::__private::*;

            {
                actor!(@handler $actor);
                actor!(@spawn $actor { $($(.$stream $(as $type)?)+)? });
            }

            let ctx = Context::new();
            let addr = ctx.address();
            self.spawn(ctx);
            addr
        }

        fn create<F>(f: F) -> $crate::macros::__private::Addr<Self>
        where
            F: $crate::macros::__private::FnOnce(
                    &mut $crate::macros::__private::Context<Self>
                ) -> Self,
        {
            use $crate::macros::__private::*;

            let mut ctx = Context::new();
            let addr = ctx.address();
            let this = f(&mut ctx);
            this.spawn(ctx);
            addr
        }

        fn start_in_arbiter<F>(
            wrk: &$crate::macros::__private::ArbiterHandle,
            f: F
        ) -> $crate::macros::__private::Addr<Self>
        where
            F: $crate::macros::__private::FnOnce(
                    &mut $crate::macros::__private::Context<Self>
                ) -> Self
                + $crate::macros::__private::Send + 'static,
        {
            use $crate::macros::__private::*;

            let (tx, rx) = channel::channel(16);

            let _ignored = wrk.spawn(async move {
                let mut ctx = Context::with_receiver(rx);
                let this = f(&mut ctx);
                this.spawn(ctx);
            });

            Addr::new(tx)
        }
    };
    (@handler $actor:ty) => {
        #[allow(non_local_definitions)]
        #[diagnostic::do_not_recommend]
        impl<T> Handler<FromStreamInner<T>> for $actor
        where
            Self: StreamHandler<T>,
        {
            type Result = ();

            fn handle(&mut self, msg: FromStreamInner<T>, ctx: &mut Context<Self>) -> Self::Result {
                match msg {
                    FromStreamInner::Started => StreamHandler::<T>::started(self, ctx),
                    FromStreamInner::Finished => StreamHandler::<T>::finished(self, ctx),
                    FromStreamInner::Value(value) => StreamHandler::<T>::handle(self, value, ctx),
                }
            }
        }
    };
    (@spawn $actor:ty { $(.$stream:ident $(as $type:ty)?)* }) => {
        #[allow(non_local_definitions)]
        impl ActorSpawn for $actor {
            fn spawn(self, ctx: Self::Context) {
                use $crate::macros::__private::*;

                actor!(@spawn_impl self ctx { $(.$stream $(as $type)?)* });
            }
        }
    };
    (@{$type:ty} ? $($fn1:ident)::+ : $($fn2:ident)::+) => {
        $($fn1)::+::<$type, _>
    };
    (@{} ? $($fn1:ident)::+ : $($fn2:ident)::+) => {
        $($fn2)::+
    };
    (@spawn_impl $self:ident $ctx:ident { $(.$stream:ident $(as $type:ty)?)* }) => {
        #[allow(unused_mut)]
        let mut this = $self;

        paste! {
            $(
                let [<stream_ $stream>] = {
                    let stream = Box::deref_mut(&mut this.$stream);
                    unsafe { &mut *ptr::from_mut(stream) }
                };
            )*
        }

        #[allow(unused_variables)]
        let addr = $ctx.address();

        let mut fut = $ctx.into_future(this);

        let _ignored = task::spawn_local({
            paste! {
                $(
                    let [<task_ $stream>] = {
                        let msg = actor!(@ { $($type)? } ? FromStreamInner::scoped_into : FromStreamInner::scoped_identity);

                        let send = FromStreamInner::send;

                        send(msg(FromStreamInner::Started, [<stream_ $stream>]), &addr);

                        let addr = addr.downgrade();

                        async move {
                            loop {
                                let item = [<stream_ $stream>].next().await;

                                let Some(addr) = addr.upgrade() else {
                                    break;
                                };

                                let Some(value) = item else {
                                    send(msg(FromStreamInner::Finished, [<stream_ $stream>]), &addr);
                                    break;
                                };

                                send(msg(FromStreamInner::Value(value.into()), [<stream_ $stream>]), &addr);
                            }
                        }
                    };
                )*
            }

            async move {
                paste! {
                    $(
                        let mut [<task_ $stream>] = pin!([<task_ $stream>].fuse());
                    )*
                }

                poll_fn(|cx| {
                    if fut.poll_unpin(cx).is_ready() {
                        return Poll::Ready(());
                    }

                    paste! {
                        $(
                            let _ignored = [<task_ $stream>].poll_unpin(cx);
                        )*
                    }

                    Poll::Pending
                })
                .fuse()
                .await
            }
        });
    };
}
