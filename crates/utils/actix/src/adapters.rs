use std::future::Future;
use std::marker::PhantomData;

use actix::dev::{MessageResponse, ToEnvelope};
use actix::fut::wrap_stream;
use actix::{
    Actor, ActorFutureExt, ActorStreamExt, Addr, AsyncContext, Handler, MailboxError, Message,
    WrapFuture,
};
use futures_util::stream::repeat;
use futures_util::{FutureExt, Stream, StreamExt, TryFutureExt};
use tokio::sync::oneshot;

pub trait AddrExt<A: Actor> {
    fn send_stream<S, M, F>(
        &self,
        stream: S,
        handler: Option<F>,
    ) -> impl Future<Output = Result<(), MailboxError>> + Send
    where
        A: Handler<StreamMessage<S, M, F>>,
        A::Context: ToEnvelope<A, StreamMessage<S, M, F>>,
        S: Send + 'static,
        M: Message + Send + 'static,
        F: Fn(M::Result) + Send + Clone + 'static;
}

#[derive(Debug, Message)]
#[rtype("()")]
pub struct StreamMessage<S, M, F>
where
    M: Message,
    F: Fn(M::Result) + Send + Clone,
{
    stream: S,
    handler: Option<F>,
    _marker: PhantomData<M>,
}

impl<A, S, M, F> MessageResponse<A, Self> for StreamMessage<S, M, F>
where
    A: Actor + Handler<M>,
    A::Context: AsyncContext<A>,
    S: Stream<Item = M> + 'static,
    M: Message,
    F: Fn(M::Result) + Send + Clone + 'static,
{
    fn handle(self, ctx: &mut A::Context, tx: Option<oneshot::Sender<()>>) {
        let stream = self.stream.zip(repeat(self.handler));

        let fut = wrap_stream::<_, A>(stream).map(|(item, handler), act, ctx| {
            let tx = handler.map(|handler| {
                let (tx, rx) = oneshot::channel();

                let _ignored = ctx.spawn(rx.map_ok(handler).map(|_| ()).into_actor(act));

                tx
            });

            act.handle(item, ctx).handle(ctx, tx);
        });

        let _ignored = ctx.spawn(fut.finish().map(|_, _, _| {
            if let Some(tx) = tx {
                let _ignored = tx.send(());
            }
        }));
    }
}

#[macro_export]
macro_rules! impl_stream_sender {
    ($($ty:path),*) => {
        const _: () = {
            use $crate::macros::__private::{Handler, Stream, Message};
            use $crate::adapters::StreamMessage;

            $(
                impl<S, M, F> Handler<StreamMessage<S, M, F>> for $ty
                where
                    S: Stream<Item = M> + 'static,
                    M: Message,
                    F: Fn(M::Result) + Send + Clone + 'static,
                    Self: Handler<M>,
                {
                    type Result = StreamMessage<S, M, F>;

                    fn handle(&mut self, msg: StreamMessage<S, M, F>, _ctx: &mut Self::Context) -> Self::Result {
                        msg
                    }
                }
            )*
        };
    };
}

impl<A: Actor> AddrExt<A> for Addr<A> {
    async fn send_stream<S, M, F>(&self, stream: S, handler: Option<F>) -> Result<(), MailboxError>
    where
        A: Handler<StreamMessage<S, M, F>>,
        A::Context: ToEnvelope<A, StreamMessage<S, M, F>>,
        S: Send + 'static,
        M: Message + Send + 'static,
        F: Fn(M::Result) + Send + Clone + 'static,
    {
        self.send(StreamMessage {
            stream,
            handler,
            _marker: PhantomData,
        })
        .await
    }
}

pub trait ActorExt: Actor {
    fn forward_handler<M>(
        &mut self,
        ctx: &mut Self::Context,
        msg: M,
        receiver: oneshot::Sender<M::Result>,
    ) where
        Self: Handler<M>,
        M: Message;
}

impl<A: Actor> ActorExt for A {
    fn forward_handler<M>(
        &mut self,
        ctx: &mut Self::Context,
        msg: M,
        receiver: oneshot::Sender<M::Result>,
    ) where
        Self: Handler<M>,
        M: Message,
    {
        self.handle(msg, ctx).handle(ctx, Some(receiver))
    }
}
