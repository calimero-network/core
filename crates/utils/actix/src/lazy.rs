#[cfg(test)]
#[path = "lazy_tests.rs"]
mod lazy_tests;

use core::fmt;
use core::future::Future;
use std::collections::VecDeque;
use std::sync::{Arc, Weak};

use actix::dev::{Envelope, EnvelopeProxy, ToEnvelope};
use actix::fut::wrap_stream;
use actix::prelude::{
    Actor, Addr, AsyncContext, Handler, MailboxError, Message, Recipient, SendError,
};
use actix::{ActorFutureExt, ActorStreamExt, Context, WrapFuture};
use async_stream::stream;
use tokio::sync::{oneshot, Mutex, MutexGuard, Notify};

pub type LazyAddr<A> = Lazy<Addr<A>>;
pub type LazyRecipient<M> = Lazy<Recipient<M>>;

pub trait IntoRef<T> {
    fn into_ref(self) -> T;
}

impl<M> IntoRef<Recipient<M>> for Recipient<M>
where
    M: Message<Result: Send> + Send,
{
    fn into_ref(self) -> Recipient<M> {
        self
    }
}

impl<A, M> IntoRef<Recipient<M>> for Addr<A>
where
    A: Actor<Context: ToEnvelope<A, M>> + Handler<M>,
    M: Message<Result: Send> + Send + 'static,
{
    fn into_ref(self) -> Recipient<M> {
        self.recipient()
    }
}

impl<A> IntoRef<Addr<A>> for Addr<A>
where
    A: Actor,
{
    fn into_ref(self) -> Addr<A> {
        self
    }
}

pub trait Receiver {
    type Item;
}

impl<M> Receiver for Recipient<M>
where
    M: Message<Result: Send> + Send,
{
    type Item = (M, Option<oneshot::Sender<M::Result>>);
}

impl<A> Receiver for Addr<A>
where
    A: Actor,
{
    type Item = Envelope<A>;
}

pub trait IntoEnvelope<A: Actor> {
    fn into_envelope(self) -> Envelope<A>;
}

impl<A, M> IntoEnvelope<A> for (M, Option<oneshot::Sender<M::Result>>)
where
    A: Actor<Context: AsyncContext<A>> + Handler<M>,
    M: Message<Result: Send> + Send + 'static,
{
    fn into_envelope(self) -> Envelope<A> {
        let (msg, tx) = self;
        Envelope::new(msg, tx)
    }
}

impl<A> IntoEnvelope<A> for Envelope<A>
where
    A: Actor,
{
    fn into_envelope(self) -> Envelope<A> {
        self
    }
}

pub trait Sender<M>: Receiver
where
    M: Message,
{
    fn pack(msg: M, tx: Option<oneshot::Sender<M::Result>>) -> Self::Item;

    fn send(
        &self,
        msg: M,
    ) -> impl Future<Output = Result<M::Result, MailboxError>> + Send + 'static;
    fn do_send(&self, msg: M);
    fn try_send(&self, msg: M) -> Result<(), SendError<M>>;
}

impl<M> Sender<M> for Recipient<M>
where
    M: Message<Result: Send> + Send + 'static,
{
    fn pack(msg: M, tx: Option<oneshot::Sender<M::Result>>) -> Self::Item {
        (msg, tx)
    }

    fn send(&self, msg: M) -> impl Future<Output = Result<M::Result, MailboxError>> + 'static {
        self.send(msg)
    }

    fn do_send(&self, msg: M) {
        self.do_send(msg)
    }

    fn try_send(&self, msg: M) -> Result<(), SendError<M>> {
        self.try_send(msg)
    }
}

impl<A, M> Sender<M> for Addr<A>
where
    A: Actor + Handler<M>,
    A::Context: AsyncContext<A> + ToEnvelope<A, M>,
    M: Message<Result: Send> + Send + 'static,
{
    fn pack(msg: M, tx: Option<oneshot::Sender<M::Result>>) -> Self::Item {
        Envelope::new(msg, tx)
    }

    fn send(&self, msg: M) -> impl Future<Output = Result<M::Result, MailboxError>> + 'static {
        self.send(msg)
    }

    fn do_send(&self, msg: M) {
        self.do_send(msg)
    }

    fn try_send(&self, msg: M) -> Result<(), SendError<M>> {
        self.try_send(msg)
    }
}

trait Resolve<A: Actor> {
    fn apply(&self, act: &mut A, ctx: &mut A::Context);

    fn finalize(&self, addr: Addr<A>);
}

impl<A, T> Resolve<A> for Mutex<LazyInner<T>>
where
    A: Actor,
    T: Receiver<Item: IntoEnvelope<A>>,
    Addr<A>: IntoRef<T>,
{
    fn apply(&self, act: &mut A, ctx: &mut <A as Actor>::Context) {
        let mut inner = self.spin_lock();

        for item in inner.queue.drain(..) {
            item.into_envelope().handle(act, ctx);
        }
    }

    fn finalize(&self, addr: Addr<A>) {
        let mut inner = self.spin_lock();

        inner.recvr = Some(addr.into_ref());
    }
}

pub trait ReceiverExt: Receiver + Sized {
    fn erase(_data: Weak<Mutex<LazyInner<Self>>>) -> Option<DynErased> {
        None
    }
}

impl<M> ReceiverExt for Recipient<M> where M: Message<Result: Send> + Send {}

impl<A> ReceiverExt for Addr<A>
where
    A: Actor,
{
    fn erase(data: Weak<Mutex<LazyInner<Self>>>) -> Option<DynErased> {
        Some(DynErased::erase::<A, _>(data))
    }
}

#[expect(
    dead_code,
    reason = "both fields represent the layout of a trait object"
)]
#[derive(Clone, Copy, Debug)]
pub struct DynErased {
    data: *const (),
    meta: *const (),
}

unsafe impl Send for DynErased {}

const _: () = {
    // this is a sanity check to ensure the
    // location of the vtable matches what
    // we expect in DynErased, technically
    // this should be consistent since futures
    // equally rely on the same vtable layout

    use std::mem::size_of;

    trait Trait {
        fn method(&self);
    }

    struct Item;

    impl Trait for Item {
        fn method(&self) {}
    }

    union U<'a> {
        erased: DynErased,
        object: &'a dyn Trait,
    }

    let item = Item;

    let unified = U { object: &item };

    let erased = unsafe { unified.erased };

    let size_of_dyn = size_of::<DynErased>() - size_of::<&dyn Trait>();
    let ptr_is_good = {
        let ptr = erased.data.cast::<u8>();
        let cmp = unsafe { ptr.offset_from(&raw const item as _) };
        cmp as usize
    };

    [[()][size_of_dyn]][ptr_is_good]
};

impl DynErased {
    const fn erase<A, T>(data: Weak<T>) -> Self
    where
        A: Actor,
        T: Resolve<A>,
    {
        #[expect(trivial_casts, reason = "false flag, doesn't compile without it")]
        let data = data as Weak<dyn Resolve<A>>;

        // SAFETY: if the constraints above hold, and use of
        //         AbstractDyn is restricted to dyn Resolve<A>
        //         this should be safe
        unsafe { std::mem::transmute(data) }
    }

    const fn downcast_ref<A: Actor>(&self) -> &Weak<dyn Resolve<A>> {
        // SAFETY: if the constraints above hold, and use of
        //         AbstractDyn is restricted to dyn Resolve<A>
        //         this should be safe
        unsafe { std::mem::transmute(self) }
    }

    const fn downcast<A: Actor>(self) -> Weak<dyn Resolve<A>> {
        // SAFETY: if the constraints above hold, and use of
        //         AbstractDyn is restricted to dyn Resolve<A>
        //         this should be safe
        unsafe { std::mem::transmute(self) }
    }
}

#[derive(Debug)]
pub struct LazyInner<T: Receiver> {
    recvr: Option<T>,
    queue: VecDeque<T::Item>,
}

struct LazyStore {
    ready: bool,
    event: Option<Arc<Notify>>,
    items: VecDeque</* dyn Resolve<A> */ DynErased>,
}

pub struct Lazy<T: Receiver> {
    inner: Arc<Mutex<LazyInner<T>>>,
    store: Arc<Mutex<LazyStore>>,
}

impl<T: Receiver> Clone for Lazy<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            store: self.store.clone(),
        }
    }
}

impl<T: Receiver> fmt::Debug for Lazy<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!(
            "Lazy {{ {:x} }}",
            Arc::as_ptr(&self.store) as usize
        ))
    }
}

impl<T: ReceiverExt> Lazy<T> {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(LazyInner {
            recvr: None,
            queue: Default::default(),
        }));

        let mut items = VecDeque::new();

        if let Some(item) = T::erase(Arc::downgrade(&inner)) {
            items.push_back(item);
        }

        let store = Arc::new(Mutex::new(LazyStore {
            ready: false,
            event: None,
            items,
        }));

        Self { inner, store }
    }
}

impl<A: Actor> Lazy<Addr<A>> {
    pub fn recipient<M>(&self) -> Lazy<Recipient<M>>
    where
        A: Handler<M>,
        A::Context: ToEnvelope<A, M> + AsyncContext<A>,
        M: Message<Result: Send> + Send + 'static,
    {
        let recvr = {
            let inner = self.inner.spin_lock();

            inner.recvr.as_ref().map(|addr| addr.clone().recipient())
        };

        let is_ready = recvr.is_some();

        let inner = Arc::new(Mutex::new(LazyInner {
            recvr,
            queue: Default::default(),
        }));

        let store = self.store.clone();

        if !is_ready {
            let mut store = store.spin_lock();

            store
                .items
                .push_back(DynErased::erase::<A, _>(Arc::downgrade(&inner)));
        }

        Lazy { inner, store }
    }
}

impl<T: Receiver> Lazy<T> {
    pub async fn init<A>(&self, factory: impl FnOnce(&mut A::Context) -> A) -> Option<T>
    where
        A: Actor<Context = Context<A>>,
        T::Item: IntoEnvelope<A>,
        Addr<A>: IntoRef<T>,
    {
        let store = self.store.clone().lock_owned().await;

        if store.ready {
            return None;
        }

        let (addr_tx, addr_rx) = oneshot::channel::<Addr<A>>();

        let (store_tx, store_rx) = oneshot::channel();

        let pending_items = stream! {
            for item in store.items.iter() {
                let Some(item) = item.downcast_ref::<A>().upgrade() else {
                    continue;
                };

                yield item;
            }

            store_tx.send(store).ok().expect("send store to complete Lazy init");
        };

        let apply_pending = wrap_stream(pending_items)
            .map(|item, act, ctx| item.apply(act, ctx))
            .finish();

        let finalize = async move {
            let addr = addr_rx.await.expect("receive addr to complete Lazy init");
            let mut store = store_rx.await.expect("receive store to complete Lazy init");

            while let Some(item) = store.items.pop_front() {
                let Some(item) = item.downcast::<A>().upgrade() else {
                    continue;
                };

                item.finalize(addr.clone());
            }

            store.ready = true;

            if let Some(notify) = store.event.take() {
                notify.notify_waiters();
            }
        };

        let task = apply_pending.then(|_, act, _| finalize.into_actor(act));

        let addr = A::create(|ctx| {
            let _ignored = ctx.spawn(task);

            factory(ctx)
        });

        addr_tx
            .send(addr.clone())
            .ok()
            .expect("send addr to complete Lazy init");

        Some(addr.into_ref())
    }
}

impl<T: Receiver + Clone> Lazy<T> {
    async fn async_get(&self) -> Option<T> {
        let inner = self.inner.lock().await;

        inner.recvr.clone()
    }

    pub fn try_get(&self) -> Option<T> {
        let inner = self.inner.spin_lock();

        inner.recvr.clone()
    }

    pub async fn get(&self) -> T {
        if let Some(recvr) = self.async_get().await {
            return recvr;
        }

        let notify = {
            let mut store = self.store.lock().await;

            if store.ready {
                return self
                    .async_get()
                    .await
                    .expect("ready store without receiver");
            }

            store.event.get_or_insert_default().clone()
        };

        notify.notified().await;

        self.async_get()
            .await
            .expect("received notification without receiver")
    }
}

impl<T: Receiver> Lazy<T> {
    pub async fn send<M>(&self, msg: M) -> Result<M::Result, MailboxError>
    where
        M: Message,
        T: Sender<M>,
    {
        let mut inner = self.inner.lock().await;

        if let Some(rx) = inner.recvr.as_ref() {
            let tx = rx.send(msg);

            drop(inner);

            return tx.await;
        }

        let (tx, rx) = oneshot::channel();

        let envelope = T::pack(msg, Some(tx));

        inner.queue.push_back(envelope);

        drop(inner);

        rx.await.map_err(|_| MailboxError::Closed)
    }

    pub fn do_send<M>(&self, msg: M)
    where
        M: Message,
        T: Sender<M>,
    {
        let mut inner = self.inner.spin_lock();

        if let Some(rx) = inner.recvr.as_ref() {
            rx.do_send(msg);

            return;
        }

        let envelope = T::pack(msg, None);

        inner.queue.push_back(envelope);
    }

    pub fn try_send<M>(&self, msg: M) -> Result<(), MailboxError>
    where
        M: Message,
        T: Sender<M>,
    {
        let mut inner = self.inner.spin_lock();

        if let Some(rx) = inner.recvr.as_ref() {
            return rx.try_send(msg).map_err(|_| MailboxError::Closed);
        }

        let envelope = T::pack(msg, None);

        inner.queue.push_back(envelope);

        Ok(())
    }
}

trait SpinLock<T> {
    fn spin_lock(&self) -> MutexGuard<'_, T>;
}

impl<T> SpinLock<T> for Mutex<T> {
    fn spin_lock(&self) -> MutexGuard<'_, T> {
        loop {
            if let Ok(guard) = self.try_lock() {
                break guard;
            }

            std::hint::spin_loop();
        }
    }
}
