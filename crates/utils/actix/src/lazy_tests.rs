use actix::{Actor, Context, Handler, Message};
use tokio::{task, time};

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

    let task = task::spawn({
        let addr = addr.clone();
        async move {
            println!("sending value");
            addr.send(Add(10)).await.unwrap();
            println!("sent value");
        }
    });

    task::yield_now().await;

    assert!(!task.is_finished());

    let _ignored = addr
        .init(|_ctx| Counter { value: 0 })
        .await
        .expect("already initialized??");

    task::yield_now().await;

    assert!(task.is_finished());

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 10);

    addr.send(Add(55)).await.unwrap();

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 65);
}

#[actix::test]
async fn test_recipient() {
    let addr = LazyRecipient::new();

    addr.do_send(Add(10));

    let _ignored = addr
        .init(|_ctx| Counter { value: 0 })
        .await
        .expect("already initialized??");
}

#[actix::test]
async fn wait_until_ready() {
    let recipient = LazyAddr::<Counter>::new();

    let irrefutable_add = |v| {
        task::spawn({
            let recipient = recipient.clone();
            async move {
                let recipient = recipient.get().await;

                recipient.send(Add(v)).await.unwrap();
            }
        })
    };

    let conditional_add = |v| {
        task::spawn({
            let recipient = recipient.clone();
            async move {
                if let Some(recipient) = recipient.try_get() {
                    recipient.do_send(Add(v));
                }
            }
        })
    };

    let task_1 = irrefutable_add(57);
    let task_2 = conditional_add(32);

    time::sleep(time::Duration::from_secs(1)).await;

    let task_3 = recipient.send(Add(10));

    let recipient = recipient
        .init(|_ctx| Counter { value: 0 })
        .await
        .expect("already initialized??");

    task_1.await.unwrap();
    task_2.await.unwrap();
    task_3.await.unwrap();

    let value = recipient.send(GetValue).await.unwrap();

    assert_eq!(value, 67);

    recipient.send(Add(35)).await.unwrap();

    let task_4 = conditional_add(32);

    task_4.await.unwrap();

    let value = recipient.send(GetValue).await.unwrap();

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
