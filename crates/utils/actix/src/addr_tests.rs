use std::{future::pending, task::Poll};

use actix::{dev::channel, Actor, Addr, Context, Handler, Message, ResponseFuture};
use futures_util::{stream, FutureExt, StreamExt};
use tokio::{sync::mpsc, task, time};
use tokio_stream::wrappers::ReceiverStream;

use crate::LazyAddr;

struct MyActor {
    value: usize,
}

impl Actor for MyActor {
    type Context = Context<Self>;
}

#[derive(Message)]
#[rtype("()")]
struct SetValue(usize);

impl Handler<SetValue> for MyActor {
    type Result = ();

    fn handle(&mut self, SetValue(value): SetValue, _ctx: &mut Context<Self>) -> Self::Result {
        self.value = value;
    }
}

#[actix::test]
async fn actix_channel_behaves_weird() {
    let (tx, rx) = channel::channel::<MyActor>(16);

    drop(tx);

    let mut rx = rx.map(|_| ());

    assert_eq!(
        None,
        rx.next().now_or_never(),
        "actix channel doesn't close when sender is dropped, though it should"
    );
}

#[actix::test]
async fn actix_channel_behaves_correctly_with_manual_check() {
    let (tx, mut rx) = channel::channel::<MyActor>(16);

    let weak_tx = tx.downgrade();

    drop(tx);

    let rx = stream::poll_fn(|cx| {
        rx.connected()
            .then(|| rx.poll_next_unpin(cx))
            .unwrap_or(Poll::Ready(None))
    });

    let mut rx = rx.map(|_| ());

    assert_eq!(
        Some(None),
        rx.next().now_or_never(),
        "actix channel behaves correctly when we manually check if it's connected"
    );

    let tx = weak_tx
        .upgrade()
        .expect("receiver still exists, so we can make a sender to it");

    tx.do_send(SetValue(10))
        .expect("we can send a message to the receiver");

    assert_eq!(Some(()), rx.next().await);
}

#[actix::test]
async fn mpsc_channel_behaves_correctly() {
    let (tx, rx) = mpsc::channel::<()>(1);

    drop(tx);

    let mut rx = ReceiverStream::new(rx);

    // this is what we expect from the actix channel, but for some reason it doesn't work
    assert_eq!(
        Some(None),
        rx.next().now_or_never(),
        "mpsc channel behaves correctly when sender is dropped"
    );
}

#[actix::test]
// #[should_panic = "attempted illegal use of uninitialized `actix::address::Recipient<calimero_utils_actix::recipient::recipient_tests::MyMessage>`"]
async fn uninitialized() {
    let recipient = LazyAddr::<MyActor>::new();

    let then = time::Instant::now();

    // let futs: Vec<_> = (1..50)
    //     .map(|_| tokio::spawn(recipient.get().send(MyMessage)))
    //     .collect();

    // time::sleep(time::Duration::from_secs(3)).await;

    recipient.init(|f| {
        MyActor::create(|ctx| {
            f.process(ctx);
            MyActor { value: 0 }
        })
    });

    drop(recipient);

    // for fut in futs {
    //     fut.await.unwrap().unwrap();
    // }

    // println!("--");
    // recipient.get().send(MyMessage).await;
    // drop(recipient);

    time::sleep(time::Duration::from_secs(3)).await;

    // pending::<()>().await;

    // let recipient = LazyRecipient::new_uninit();

    // recipient.send(MyMessage).await.unwrap();
}

#[actix::test]
// #[should_panic = "attempted illegal use of uninitialized `actix::address::Recipient<calimero_utils_actix::recipient::recipient_tests::MyMessage>`"]
async fn bad_initialization() {
    let recipient = LazyAddr::<MyActor>::new();

    recipient.init(|_unused| MyActor::create(|_ctx| MyActor { value: 0 }));

    drop(recipient);

    // for fut in futs {
    //     fut.await.unwrap().unwrap();
    // }

    // println!("--");
    // recipient.get().send(MyMessage).await;
    // drop(recipient);

    time::sleep(time::Duration::from_secs(3)).await;

    // pending::<()>().await;

    // let recipient = LazyRecipient::new_uninit();

    // recipient.send(MyMessage).await.unwrap();
}

// #[actix::test]
// async fn initialized() {
//     // let recipient = LazyAddr::new_uninit();

//     // recipient.init(MyActor.start());

//     // recipient.send(MyMessage).await.unwrap();
// }
