use std::future::Future;

use actix::dev::{MessageResponse, ToEnvelope};
use actix::fut::wrap_stream;
use actix::{
    Actor, ActorFutureExt, ActorStreamExt, Addr, AsyncContext, Handler, MailboxError, Message,
    WrapFuture,
};
use futures_util::stream::repeat;
use futures_util::{Stream, StreamExt};
use tokio::sync::oneshot;

pub trait AddrExt<A: Actor> {
    fn send_stream<S, M, F>(
        &self,
        stream: S,
        handler: Option<F>,
    ) -> impl Future<Output = Result<(), MailboxError>> + Send
    where
        A: Handler<M> + Handler<StreamMessage<S, F>>,
        A::Context: ToEnvelope<A, StreamMessage<S, F>>,
        M: Message,
        S: Send + 'static,
        F: Fn(M::Result) + Send + 'static;
}

#[derive(Debug, Message)]
#[rtype("()")]
pub struct StreamMessage<S, F>
where
    S: 'static,
    F: 'static,
{
    stream: S,
    handler: Option<F>,
}

impl<S, F> StreamMessage<S, F> {
    pub fn returns(self) -> StreamMessageResponse<S, F> {
        StreamMessageResponse {
            stream: self.stream,
            handler: self.handler,
        }
    }
}

#[derive(Debug)]
pub struct StreamMessageResponse<S, F> {
    stream: S,
    handler: Option<F>,
}

impl<A, M, S, F> MessageResponse<A, StreamMessage<S, F>> for StreamMessageResponse<S, F>
where
    A: Actor<Context: AsyncContext<A>> + Handler<M>,
    M: Message,
    S: Stream<Item = M> + 'static,
    F: Fn(M::Result) + Send + Clone,
{
    fn handle(self, ctx: &mut A::Context, tx: Option<oneshot::Sender<()>>) {
        let stream = self.stream.zip(repeat(self.handler));

        let fut = wrap_stream::<_, A>(stream).map(|(item, handler), act, ctx| {
            let tx = handler.map(|handler| {
                let (tx, rx) = oneshot::channel();

                let _ignored = ctx.spawn(
                    async move {
                        if let Ok(item) = rx.await {
                            handler(item);
                        }
                    }
                    .into_actor(act),
                );

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

impl<A: Actor> AddrExt<A> for Addr<A> {
    async fn send_stream<S, M, F>(&self, stream: S, handler: Option<F>) -> Result<(), MailboxError>
    where
        A: Handler<M> + Handler<StreamMessage<S, F>>,
        A::Context: ToEnvelope<A, StreamMessage<S, F>>,
        M: Message,
        S: Send + 'static,
        F: Fn(M::Result) + Send + 'static,
    {
        self.send(StreamMessage { stream, handler }).await
    }
}
