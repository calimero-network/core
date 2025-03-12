#[cfg(test)]
#[path = "macros_tests.rs"]
mod macros_tests;

#[doc(hidden)]
pub mod __private {
    #![allow(dead_code, unused_imports, reason = "Macros")]

    pub use std::boxed::Box;
    pub use std::ops::DerefMut;
    pub use std::ptr;

    pub use actix::{AsyncContext, Context, Handler, Message, StreamHandler};
    pub use futures_util::{pin_mut, FutureExt, Stream, StreamExt};
    pub use paste::paste;
    pub use tokio::{select, task};

    pub use crate::spawn_actor;

    #[derive(Debug, Message)]
    #[rtype("()")]
    pub enum FromStreamInner<T> {
        Started,
        Finished,
        Value(T),
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
macro_rules! spawn_actor {
    (@$type:ident ? $($fn1:ident)::+ : $($fn2:ident)::+) => {
        $($fn1)::+::<$type, _>
    };
    (@? $($fn1:ident)::+ : $($fn2:ident)::+) => {
        $($fn2)::+
    };
    ($self:ident @ Self $($rest:tt),*) => {
        compile_error!("`Self` is not allowed")
    };
    ($self:ident @ $actor:ident $(=> {$(.$stream:ident $(as $type:ident)?),+ $(,)?})?) => {{
        use $crate::macros::__private::*;

        paste! {
            $($(
                let [<stream_ $stream>] = {
                    let stream = Box::deref_mut(&mut $self.$stream);
                    unsafe { &mut *ptr::from_mut(stream) }
                };
            )+)?
        }

        let ctx = Context::new();

        let addr = ctx.address();

        let mut fut = ctx.into_future($self);

        #[allow(non_local_definitions)]
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

        let _ignored = task::spawn_local({
            paste! {
                $($(
                    let [<task_ $stream>] = {
                        let func = spawn_actor!(@ $($type)? ? FromStreamInner::scoped_into : FromStreamInner::scoped_identity);

                        addr.do_send(func(FromStreamInner::Started, [<stream_ $stream>]));

                        let addr = addr.downgrade();

                        async move {
                            loop {
                                let item = [<stream_ $stream>].next().await;

                                let Some(addr) = addr.upgrade() else {
                                    break;
                                };

                                let Some(value) = item else {
                                    addr.do_send(func(FromStreamInner::Finished, [<stream_ $stream>]));
                                    break;
                                };

                                addr.do_send(func(FromStreamInner::Value(value.into()), [<stream_ $stream>]));
                            }
                        }
                    };
                )+)?
            }

            async move {
                paste! {
                    $($(
                        pin_mut!([<task_ $stream>]);

                        let mut [<task_ $stream>] = [<task_ $stream>].fuse();
                    )+)?
                }

                loop {
                    paste! {
                        select! {
                            biased;
                            _ = &mut fut => { break },
                            $($( _ = &mut [<task_ $stream>] => {} )+)?
                        }
                    }
                }
            }
        });

        addr
    }};
}
