#[cfg(test)]
#[path = "lazy_tests.rs"]
mod lazy_tests;

use core::fmt;
use core::future::Future;
use core::pin::Pin;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use actix::dev::{Envelope, EnvelopeProxy, ToEnvelope};
use actix::fut::wrap_stream;
use actix::prelude::{
    Actor, ActorFuture, Addr, AsyncContext, Handler, MailboxError, Message, Recipient, SendError,
};
use actix::{ActorFutureExt, ActorStreamExt, WrapFuture};
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
    fn abstract_resolve(_data: Weak<Mutex<LazyInner<Self>>>) -> Option<AbstractDyn> {
        None
    }
}

impl<M> ReceiverExt for Recipient<M> where M: Message<Result: Send> + Send {}

impl<A> ReceiverExt for Addr<A>
where
    A: Actor,
{
    fn abstract_resolve(data: Weak<Mutex<LazyInner<Self>>>) -> Option<AbstractDyn> {
        Some(AbstractDyn::abstract_resolve::<A, _>(data))
    }
}

#[expect(
    dead_code,
    reason = "both fields represent the layout of a trait object"
)]
#[derive(Clone, Copy, Debug)]
pub struct AbstractDyn {
    data: *const u8,
    meta: *const u8,
}

unsafe impl Send for AbstractDyn {}

const _: () = {
    // SAFETY: this should ensure the poisiton of the vtable
    //         matches what we expect in AbstractReceiver

    use std::mem::{size_of, ManuallyDrop};

    union U<T> {
        recv: ManuallyDrop<AbstractDyn>,
        data: ManuallyDrop<T>,
    }

    trait Trait {
        fn method(&self);
    }

    struct Item;

    impl Trait for Item {
        fn method(&self) {}
    }

    let item = Item;

    let unified = U {
        #[expect(trivial_casts, reason = "false flag, doesn't compile without it")]
        data: ManuallyDrop::new(&item as &dyn Trait),
    };

    let recv = unsafe { ManuallyDrop::into_inner(unified.recv) };

    let size_of_dyn = (size_of::<usize>() * 2) - size_of::<&dyn Trait>();
    let ptr_is_good = unsafe { recv.data.offset_from(&raw const item as _) };

    // if this fails to compile, revisit this
    [[()][size_of_dyn]][ptr_is_good as usize]
};

impl AbstractDyn {
    fn abstract_resolve<A, T>(data: Weak<T>) -> Self
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

    fn downcast_ref<A: Actor>(&self) -> &Weak<dyn Resolve<A>> {
        // SAFETY: if the constraints above hold, and use of
        //         AbstractDyn is restricted to dyn Resolve<A>
        //         this should be safe
        // let ptr = self as *const _ as *const Weak<dyn Resolve<A>>;
        // unsafe { &*ptr }
        // unsafe { std::mem::transmute::<&AbstractDyn, &Weak<dyn Resolve<A>>>(&self) }
        // unsafe { std::mem::transmute::<&AbstractDyn, &Weak<dyn Resolve<A>>>(self) }
        unsafe { std::mem::transmute(self) }
    }

    fn downcast<A: Actor>(self) -> Weak<dyn Resolve<A>> {
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
    items: VecDeque<AbstractDyn>,
    event: Option<Arc<Notify>>,
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

        if let Some(item) = T::abstract_resolve(Arc::downgrade(&inner)) {
            items.push_back(item);
        }

        let store = Arc::new(Mutex::new(LazyStore { items, event: None }));

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
                .push_back(AbstractDyn::abstract_resolve::<A, _>(Arc::downgrade(
                    &inner,
                )));
        }

        Lazy { inner, store }
    }
}

impl<T: Receiver> Lazy<T> {
    pub async fn init<A, U, S>(&self, func: impl FnOnce(PendingMessages<'_, A, S>) -> U) -> T
    where
        A: Actor<Context: AsyncContext<A>>,
        U: IntoRef<Addr<A>> + 'static,
        T::Item: IntoEnvelope<A>,
        Addr<A>: IntoRef<T>,
        S: PendingGuard,
    {
        let (addr_tx, addr_rx) = oneshot::channel::<Addr<A>>();

        let store = self.store.clone().lock_owned().await;

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

            if let Some(notify) = &store.event {
                notify.notify_waiters();
            }
        };

        let task = apply_pending.then(|_, act, _| finalize.into_actor(act));

        #[expect(trivial_casts, reason = "false flag, doesn't compile without it")]
        let mut task = Some(Box::pin(task) as Pin<Box<_>>);

        let value = func(PendingMessages {
            inner: &mut task,
            _priv: PhantomData,
        });

        let addr = value.into_ref();

        addr_tx
            .send(addr.clone())
            .ok()
            .expect("send addr to complete Lazy init");

        addr.into_ref()
    }
}

impl<T: Receiver + Clone> Lazy<T> {
    pub async fn get(&self) -> T {
        {
            let inner = self.inner.lock().await;

            if let Some(recvr) = &inner.recvr {
                return recvr.clone();
            }
        };

        let notify = {
            let mut store = self.store.lock().await;

            store.event.get_or_insert_default().clone()
        };

        notify.notified().await;

        let inner = self.inner.lock().await;

        inner
            .recvr
            .clone()
            .expect("received event without being ready")
    }

    pub fn try_get(&self) -> Option<T> {
        let inner = self.inner.spin_lock();

        inner.recvr.clone()
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

#[must_use = "please call `finish`"]
pub struct PendingMessages<'a, A: Actor, S: PendingGuard> {
    inner: &'a mut Option<Pin<Box<dyn ActorFuture<A, Output = ()>>>>,
    _priv: PhantomData<S>,
}

impl<A: Actor, S: PendingGuard> Drop for PendingMessages<'_, A, S> {
    fn drop(&mut self) {
        assert!(
            self.inner.take().is_none(),
            "pending messages were not processed"
        );
    }
}

impl<A: Actor, S: PendingGuard> fmt::Debug for PendingMessages<'_, A, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingMessages").finish()
    }
}

mod private {
    #[diagnostic::on_unimplemented(
        message = "`PendingMessages` must be consumed by calling `.process(ctx)`"
    )]
    pub trait PendingGuard {}
}

use private::PendingGuard;

/// Call `.process(ctx)`
enum PendingHandle {}

impl PendingGuard for PendingHandle {}

impl<A> PendingMessages<'_, A, PendingHandle>
where
    A: Actor<Context: AsyncContext<A>>,
{
    pub fn process(self, ctx: &mut A::Context) {
        let inner = self.inner.take().expect("missing `PendingMessages` future");
        let _ignored = ctx.spawn(inner);
    }
}
