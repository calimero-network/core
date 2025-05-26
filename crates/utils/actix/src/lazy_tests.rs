use std::pin::pin;

use actix::fut::wrap_future;
use actix::{Actor, ActorFutureExt, AsyncContext, Context, Handler, Message, WrapFuture};
use futures_util::FutureExt;
use tokio::sync::oneshot;
use tokio::{join, task, try_join};

use crate::{LazyAddr, LazyRecipient};

struct Counter {
    value: usize,
}

impl Actor for Counter {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // this is here as a sanity check test to ensure
        // "started" is called before any queued messages
        assert_eq!(0, self.value);

        // `wait` here will execute before any queued messages
        let _ignored = ctx.wait(async {}.into_actor(self).then(|_, act, _ctx| {
            assert_eq!(0, act.value);
            async {}.into_actor(act)
        }));

        // `spawn` will execute after all pending actor's messages
        let _ignored = ctx.spawn(async {}.into_actor(self).then(|_, act, _ctx| {
            assert_ne!(0, act.value);
            async {}.into_actor(act)
        }));
    }
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

    let _ignored = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        Counter { value: 0 }
    });

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

    let _ignored = Actor::create(|ctx| {
        assert!(recipient.init(ctx));

        let task = wrap_future::<_, Counter>(async {}).then(move |_, act, _ctx| {
            tx.send(act.value).unwrap();
            async {}.into_actor(act)
        });

        let _ignored = ctx.spawn(task);

        Counter { value: 0 }
    });

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

    task::yield_now().await;

    assert!(!task_1.is_finished());

    // this should be queued to be processed
    let task_2 = addr.send(Add(10));

    let addr = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        Counter { value: 0 }
    });

    let (task_1, task_2) = tokio::join!(task_1, task_2);

    task_1.unwrap();
    task_2.unwrap();

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 67);

    addr.send(Add(35)).await.unwrap();

    conditional_add(32);

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 134);
}

#[actix::test]
async fn early_poll_completes() {
    let addr = LazyAddr::new();

    let mut will_send = pin!(addr.send(Add(5)));

    // quick poll to schedule the message
    assert_eq!(None, will_send.as_mut().now_or_never());

    let mut late_addr = pin!(addr.get());

    // poll, let's schedule a waiter
    assert_eq!(None, late_addr.as_mut().now_or_never());

    let live_addr = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        Counter { value: 0 }
    });

    assert_eq!(None, will_send.as_mut().now_or_never());
    assert_eq!(None, late_addr.as_mut().now_or_never());

    task::yield_now().await;

    // by now, the message should have been processed
    will_send
        .now_or_never()
        .expect("result should be ready by now")
        .expect("message must've been handled");

    // by now, the address should be ready
    let late_addr = late_addr
        .now_or_never()
        .expect("addr should be ready by now");

    assert_eq!(live_addr, late_addr);

    let ready_addr = addr
        .get()
        .now_or_never()
        .expect("addr should be ready by now");

    assert_eq!(live_addr, ready_addr);

    let value = live_addr.send(GetValue).await.unwrap();

    assert_eq!(value, 5);
}

#[actix::test]
async fn pending_is_prioritized() {
    let addr = LazyAddr::new();

    addr.do_send(Add(2));

    let mut will_send = pin!(addr.send(Add(5)));

    // schedule the message
    assert_eq!(None, will_send.as_mut().now_or_never());

    let addr = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        Counter { value: 0 }
    });

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 7);

    will_send
        .now_or_never()
        .expect("we know this is ready")
        .expect("we know there was no error");
}

#[actix::test]
async fn derive_recipient() {
    let addr = LazyAddr::new();

    addr.do_send(Add(3));

    let wait_send = task::spawn({
        let recipient = addr.recipient();
        async move {
            let recipient = recipient.get().await;

            recipient.send(Add(57)).await.unwrap();
        }
    });

    let queue_send = task::spawn({
        let recipient = addr.recipient();
        async move {
            recipient.send(Add(57)).await.unwrap();
        }
    });

    task::yield_now().await;

    assert!(!wait_send.is_finished());
    assert!(!queue_send.is_finished());

    let addr = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        Counter { value: 0 }
    });

    task::yield_now().await;

    assert!(!wait_send.is_finished());
    assert!(queue_send.is_finished());

    task::yield_now().await;

    assert!(wait_send.is_finished());

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 117);
}

#[actix::test]
async fn multiple_recipients() {
    let addr = LazyAddr::new();

    let recipient = addr.recipient();

    let mut set = task::JoinSet::new();

    for i in 1..=10 {
        // task1
        let _ignored = set.spawn({
            let recipient = addr.recipient();
            async move {
                let recipient = recipient.get().await;

                recipient.send(Add(i)).await.unwrap();
            }
        });

        // task2
        let _ignored = set.spawn({
            let recipient = addr.recipient();
            async move {
                recipient.send(Add(i * 100)).await.unwrap();
            }
        });

        addr.clone().do_send(Add(i * 10000));
    }

    // this should allow task2 to schedule it's messages
    task::yield_now().await;

    let addr = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        Counter { value: 0 }
    });

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 55_55_00);

    // this should allow task1 progress
    task::yield_now().await;

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 55_55_55);

    let value = recipient.send(GetValue).await.unwrap();

    assert_eq!(value, 55_55_55);

    while let Some(task) = set.join_next().await {
        task.unwrap();
    }
}

#[actix::test]
async fn partial_eq() {
    let addr1 = LazyAddr::<Counter>::new();
    let addr2 = addr1.clone();

    assert_eq!(addr1, addr2);

    let addr3 = LazyAddr::<Counter>::new();

    assert_ne!(addr1, addr3);
    assert_ne!(addr2, addr3);

    let recipient1 = addr1.recipient::<Add>();
    let recipient2 = addr1.recipient();
    let recipient3 = recipient1.clone();

    assert_eq!(recipient1, recipient2);
    assert_eq!(recipient2, recipient3);

    let recipient4 = LazyRecipient::<Add>::new();

    assert_ne!(recipient1, recipient4);
    assert_ne!(recipient2, recipient4);
    assert_ne!(recipient3, recipient4);
}

#[actix::test]
async fn double_init_fails() {
    let addr = LazyAddr::new();

    let _ignored = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        assert!(!addr.init(ctx));

        Counter { value: 0 }
    });
}

#[actix::test]
async fn conflictive_init_fails() {
    let addr = LazyAddr::new();

    addr.do_send(Add(2));

    let init = || {
        let addr = addr.clone();

        async move {
            let _ignored = Actor::create(|ctx| {
                assert!(addr.init(ctx), "actor initialization failed");

                Counter { value: 0 }
            });
        }
    };

    let t1 = task::spawn_local(init());
    let t2 = task::spawn_local(init());

    let (t1_initialized, t2_initialized) = join!(t1, t2);

    t1_initialized.unwrap();
    let err = t2_initialized.expect_err("second init should fail");

    assert!(
        err.to_string().contains("actor initialization failed"),
        "found instead: {err}",
    );

    addr.send(Add(3)).await.unwrap();

    let value = addr.send(GetValue).await.unwrap();

    assert_eq!(value, 5);
}

#[actix::test]
async fn locks_arent_held_across_await_points() {
    let addr = LazyAddr::new();

    let get_fut = task::spawn({
        let (addr1, addr2) = (addr.clone(), addr.clone());

        async move {
            let mut addr1 = pin!(addr1.get());
            let mut addr2 = pin!(addr2.get());

            assert_eq!(None, addr1.as_mut().now_or_never());
            assert_eq!(None, addr2.as_mut().now_or_never());

            task::yield_now().await;

            let a = addr1
                .as_mut()
                .now_or_never()
                .expect("addr should be ready by now");

            let b = addr2
                .as_mut()
                .now_or_never()
                .expect("addr should be ready by now");

            assert_eq!(a, b);
        }
    });

    let send_fut = task::spawn({
        let (addr1, addr2) = (addr.clone(), addr.clone());

        async move {
            try_join!(addr1.send(Add(2)), addr2.send(Add(3))).unwrap();
        }
    });

    task::yield_now().await;

    let addr = Actor::create(|ctx| {
        assert!(addr.init(ctx));
        Counter { value: 0 }
    });

    let get_value = addr.send(GetValue);

    task::yield_now().await;

    assert!(get_fut.is_finished());
    assert!(send_fut.is_finished());

    let value = get_value.await.unwrap();

    assert_eq!(value, 5);
}
