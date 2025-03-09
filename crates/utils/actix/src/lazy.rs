#[cfg(test)]
#[path = "lazy_tests.rs"]
mod lazy_tests;

use core::future::Future;
use core::{fmt, mem};
use std::any::{type_name, TypeId};
use std::collections::VecDeque;
use std::sync::{Arc, Weak};

use actix::dev::{Envelope, EnvelopeProxy, ToEnvelope};
use actix::fut::wrap_stream;
use actix::prelude::{
    Actor, Addr, AsyncContext, Handler, MailboxError, Message, Recipient, SendError,
};
use actix::{ActorStreamExt, Context};
use async_stream::stream;
use calimero_primitives::reflect::{Reflect, ReflectExt};
use calimero_primitives::utils;
use itertools::Itertools;
use tokio::sync::{oneshot, Mutex, MutexGuard, Notify, OwnedMutexGuard};

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

type LazyResolver<T> = Mutex<LazyInner<T>>;

pub trait Receiver: Sized {
    type Item;

    fn erase(data: &Arc<LazyResolver<Self>>) -> Option<(TypeId, DynErased)>;
}

impl<M> Receiver for Recipient<M>
where
    M: Message<Result: Send> + Send,
{
    type Item = (M, Option<oneshot::Sender<M::Result>>);

    fn erase(_data: &Arc<LazyResolver<Self>>) -> Option<(TypeId, DynErased)> {
        None
    }
}

impl<A> Receiver for Addr<A>
where
    A: Actor<Context: AsyncContext<A>>,
{
    type Item = Envelope<A>;

    fn erase(data: &Arc<LazyResolver<Self>>) -> Option<(TypeId, DynErased)> {
        let id = TypeId::of::<LazyResolver<Self>>();

        let item = DynErased::erase::<A, _>(Arc::downgrade(data));

        Some((id, item))
    }
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

trait Resolve<A: Actor>: Reflect {
    fn resolve(&self, act: &mut A, ctx: &mut A::Context);
}

impl<A, T> Resolve<A> for Mutex<LazyInner<T>>
where
    A: Actor<Context: AsyncContext<A>>,
    T: Receiver<Item: IntoEnvelope<A>>,
    Addr<A>: IntoRef<T>,
{
    fn resolve(&self, act: &mut A, ctx: &mut A::Context) {
        let mut inner = self.spin_lock();

        for item in inner.queue.drain(..) {
            item.into_envelope().handle(act, ctx);
        }

        inner.recvr = Some(ctx.address().into_ref());
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

// pub struct DynResolve {
//     resolver: DynResolveInner,
//     identity: std::any::TypeId,
// }

// mod dyn_resolve {
//     // https://github.com/rust-lang/rust/issues/69757
//     // https://github.com/rust-lang/rust/issues/46139
//     erase! {
//         <T, A> => T: Resolve<A>,
//             where A: Actor + 'static,
//         Should we only support Weak<T>?
//     }
// }

impl DynErased {
    const fn erase<A, T>(data: Weak<T>) -> Self
    where
        A: Actor,
        T: Resolve<A> + 'static,
    {
        #[expect(trivial_casts, reason = "false flag, doesn't compile without it")]
        let data = data as Weak<dyn Resolve<A> + 'static>;

        // SAFETY: if the constraints above hold, and use of
        //         AbstractDyn is restricted to dyn Resolve<A>
        //         this should be safe
        unsafe { mem::transmute(data) }
    }

    const fn downcast_ref<A: Actor>(&self) -> &Weak<dyn Resolve<A>> {
        // SAFETY: if the constraints above hold, and use of
        //         AbstractDyn is restricted to dyn Resolve<A>
        //         this should be safe
        unsafe { mem::transmute(self) }
    }

    const fn downcast<A: Actor>(self) -> Weak<dyn Resolve<A>> {
        // SAFETY: if the constraints above hold, and use of
        //         AbstractDyn is restricted to dyn Resolve<A>
        //         this should be safe
        unsafe { mem::transmute(self) }
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
    items: VecDeque<(TypeId, /* dyn Resolve<A> */ DynErased)>,
}

pub struct Lazy<T: Receiver> {
    inner: Arc<LazyResolver<T>>,
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
        let path = type_name::<Self>();
        let path = utils::compact_path(path).format("");
        write!(f, "{} {{ {:p} }}", path, self.store)
    }
}

impl<T: Receiver> Lazy<T> {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(LazyInner {
            recvr: None,
            queue: Default::default(),
        }));

        let mut items = VecDeque::new();

        if let Some(item) = T::erase(&inner) {
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

impl<A> Lazy<Addr<A>>
where
    A: Actor<Context: AsyncContext<A>>,
{
    pub fn recipient<M>(&self) -> Lazy<Recipient<M>>
    where
        A: Handler<M>,
        A::Context: ToEnvelope<A, M>,
        M: Message<Result: Send> + Send + 'static,
    {
        let recvr = {
            let inner = self.inner.spin_lock();

            inner.recvr.as_ref().map(|addr| addr.clone().recipient())
        };

        let is_ready = recvr.is_some();

        let mut store = self.store.spin_lock();

        let this_id = TypeId::of::<LazyResolver<Recipient<M>>>();

        let inner = 'done: {
            for (that_id, item) in store.items.iter() {
                if this_id == *that_id {
                    if let Some(weak) = item.downcast_ref::<A>().upgrade() {
                        if let Ok(resolver) = weak.downcast_arc::<LazyResolver<Recipient<M>>>() {
                            break 'done resolver;
                        }
                    }

                    break;
                }
            }

            let inner = Arc::new(Mutex::new(LazyInner {
                recvr,
                queue: Default::default(),
            }));

            if !is_ready {
                let item = DynErased::erase::<A, _>(Arc::downgrade(&inner));

                store.items.push_back((this_id, item));
            }

            inner
        };

        Lazy {
            inner,
            store: self.store.clone(),
        }
    }
}

impl<T: Receiver + 'static> Lazy<T> {
    pub fn init<A>(&self, factory: impl FnOnce(&mut A::Context) -> A) -> Option<T>
    where
        A: Actor<Context = Context<A>>,
        T::Item: IntoEnvelope<A>,
        Addr<A>: IntoRef<T>,
    {
        let mut store = self.store.clone().spin_lock_owned();

        if store.ready {
            return None;
        }

        #[expect(trivial_casts, reason = "false flag, doesn't compile without it")]
        let maybe_inner = store
            .items
            .is_empty()
            .then(|| self.inner.clone() as Arc<dyn Resolve<A>>);

        let pending_items = stream!({
            if let Some(inner) = maybe_inner {
                yield inner;
            }

            for (_id, item) in store.items.drain(..) {
                let Some(item) = item.downcast::<A>().upgrade() else {
                    continue;
                };

                yield item;
            }

            store.ready = true;

            if let Some(notify) = store.event.take() {
                notify.notify_waiters();
            }
        });

        let resolve_pending = wrap_stream(pending_items)
            .map(|item, act, ctx| item.resolve(act, ctx))
            .finish();

        let addr = A::create(|ctx| {
            let _ignored = ctx.wait(resolve_pending);

            factory(ctx)
        });

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

    pub fn try_send<M>(&self, msg: M) -> Result<(), SendError<M>>
    where
        M: Message,
        T: Sender<M>,
    {
        let mut inner = self.inner.spin_lock();

        if let Some(rx) = inner.recvr.as_ref() {
            return rx.try_send(msg);
        }

        let envelope = T::pack(msg, None);

        inner.queue.push_back(envelope);

        Ok(())
    }
}

impl<T: Receiver> PartialEq for Lazy<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

trait SpinLock<T> {
    fn spin_lock(&self) -> MutexGuard<'_, T>;
    fn spin_lock_owned(self: Arc<Self>) -> OwnedMutexGuard<T>;
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

    fn spin_lock_owned(self: Arc<Self>) -> OwnedMutexGuard<T> {
        loop {
            if let Ok(guard) = self.clone().try_lock_owned() {
                break guard;
            }

            std::hint::spin_loop();
        }
    }
}
