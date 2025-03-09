use actix::fut::wrap_future;
use actix::{Actor, ActorFutureExt, AsyncContext, Context, Handler, Message, WrapFuture};
use futures_util::FutureExt;
use tokio::sync::oneshot;
use tokio::task;

use crate::{LazyAddr, LazyRecipient};

struct Counter {
    value: usize,
}

impl Actor for Counter {
    type Context = Context<Self>;
}

#[derive(Debug, Message)]
#[rtype("()")]
struct Add(usize);

impl Handler<Add> for Counter {
    type Result = ();

    fn handle(&mut self, Add(value): Add, _ctx: &mut Context<Self>) -> Self::Result {
        self.value += value;
    }
}

#[derive(Debug, Message)]
#[rtype(usize)]
struct GetValue;

impl Handler<GetValue> for Counter {
    type Result = usize;

    fn handle(&mut self, _msg: GetValue, _ctx: &mut Context<Self>) -> Self::Result {
        self.value
    }
}

#[actix::test]
async fn test_addr() {
    let addr = LazyAddr::new();

    addr.do_send(Add(3));

    let task = task::spawn({
        let addr = addr.clone();
        async move {
            addr.send(Add(10)).await.unwrap();
        }
    });

    task::yield_now().await;

    assert!(!task.is_finished());

    let _ignored = addr
        .init(|_ctx| Counter { value: 0 })
        .expect("already initialized??");

    task::yield_now().await;

    assert!(task.is_finished());

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 13);

    addr.send(Add(55)).await.unwrap();

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 68);
}

#[actix::test]
async fn test_recipient() {
    let recipient = LazyRecipient::new();

    recipient.do_send(Add(3));

    let task = task::spawn({
        let recipient = recipient.clone();
        async move {
            recipient.send(Add(10)).await.unwrap();
        }
    });

    task::yield_now().await;

    assert!(!task.is_finished());

    let (tx, rx) = oneshot::channel();

    let _ignored = recipient
        .init(|ctx: &mut Context<_>| {
            let task = wrap_future::<_, Counter>(async {}).then(|_, act, _ctx| {
                tx.send(act.value).unwrap();
                async {}.into_actor(act)
            });

            let _ignored = ctx.spawn(task);

            Counter { value: 0 }
        })
        .expect("already initialized??");

    task::yield_now().await;

    assert!(task.is_finished());

    let value = rx.now_or_never().unwrap().unwrap();

    assert_eq!(value, 13);
}

#[actix::test]
async fn wait_until_ready() {
    let addr = LazyAddr::new();

    let irrefutable_add = |v| {
        let addr = addr.clone();
        async move {
            let addr = addr.get().await;

            addr.send(Add(v)).await.unwrap();
        }
    };

    let conditional_add = |v| {
        if let Some(addr) = addr.try_get() {
            addr.do_send(Add(v));
        }
    };

    // this should be thrown away
    conditional_add(32);

    // this should eventually be processed
    let task_1 = task::spawn(irrefutable_add(57));

    // this should be queued to be processed
    let task_2 = addr.send(Add(10));

    let addr = addr
        .init(|_ctx| Counter { value: 0 })
        .expect("already initialized??");

    let (task_1, task_2) = tokio::join!(task_1, task_2);

    task_1.unwrap();
    task_2.unwrap();

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 67);

    addr.send(Add(35)).await.unwrap();

    let task_4 = conditional_add(32);

    task_4.await.unwrap();

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 134);
}

// #[actix::test]
// async fn derive_recipient() {
//     let recipient = LazyRecipient::<Add>::new();

//     let task = task::spawn({
//         let recipient = recipient.clone();
//         async move {
//             let recipient = recipient.get().await;

//             recipient.do_send(Add(57));
//         }
//     });

//     let recipient = recipient
//         .init(|pending| {
//             Counter::create(|ctx| {
//                 pending.process(ctx);
//                 Counter { value: 0 }
//             })
//         })
//         .await;

//     recipient.send(Add(35)).await;

//     let value = recipient.send(GetValue).await.unwrap();

//     assert_eq!(value, 92);
// }

// #[actix::test]
// async fn cloned_any() {
//     let recipient = LazyRecipient::<Add>::new();

//     let task = task::spawn({
//         let recipient = recipient.clone();
//         async move {
//             let recipient = recipient.get().await;

//             recipient.do_send(Add(57));
//         }
//     });

//     let recipient = recipient
//         .init(|pending| {
//             Counter::create(|ctx| {
//                 pending.process(ctx);
//                 Counter { value: 0 }
//             })
//         })
//         .await;

//     recipient.send(Add(35)).await;

//     let value = recipient.send(GetValue).await.unwrap();

//     assert_eq!(value, 92);
// }
