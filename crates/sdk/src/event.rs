use std::borrow::Cow;

use crate::env;
use crate::state::AppState;

pub trait AppEvent {
    fn kind(&self) -> Cow<'_, str>;
    fn data(&self) -> Cow<'_, [u8]>;
}

#[derive(Debug)]
pub struct EncodedAppEvent<'a> {
    pub kind: Cow<'a, str>,
    pub data: Cow<'a, [u8]>,
}

thread_local! {
    static HANDLER: std::cell::RefCell<fn(Box<dyn AppEventExt>)> = panic!("uninitialized handler");
}

#[track_caller]
#[inline(never)]
fn handler<E: AppEventExt + 'static>(event: Box<dyn AppEventExt>) {
    if let Ok(event) = E::downcast(event) {
        env::emit(&event);
    }
}

pub fn register<S: AppState>()
where
    for<'a> S::Event<'a>: AppEventExt,
{
    HANDLER.set(handler::<S::Event<'static>>);
}

#[track_caller]
pub fn emit<'a, E: AppEventExt + 'a>(event: E) {
    let f = HANDLER.with_borrow(|handler| *handler);
    let f: fn(Box<dyn AppEventExt + 'a>) = unsafe { std::mem::transmute::<_, _>(f) };
    f(Box::new(event))
}

mod reflect {
    pub use std::any::TypeId;

    pub trait Reflect {
        fn id(&self) -> TypeId
        where
            Self: 'static,
        {
            TypeId::of::<Self>()
        }

        fn name(&self) -> &'static str {
            std::any::type_name::<Self>()
        }
    }

    impl<T> Reflect for T {}
}

use reflect::Reflect;

pub trait AppEventExt: AppEvent + Reflect {
    // todo! experiment with &dyn AppEventExt downcast_ref to &Self
    // yes, this will mean delegated downcasting would have to be referential
    // but that's not bad, not one bit
    fn downcast(event: Box<dyn AppEventExt>) -> Result<Self, Box<dyn AppEventExt>>
    where
        Self: Sized + 'static,
    {
        downcast(event)
    }
}

impl dyn AppEventExt {
    pub fn is<T: AppEventExt + 'static>(&self) -> bool {
        self.id() == reflect::TypeId::of::<T>()
    }
}

pub fn downcast<T: AppEventExt + 'static>(
    event: Box<dyn AppEventExt>,
) -> Result<T, Box<dyn AppEventExt>> {
    if event.is::<T>() {
        Ok(*unsafe { Box::from_raw(Box::into_raw(event) as *mut T) })
    } else {
        Err(event)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum NoEvent {}
impl AppEvent for NoEvent {
    fn kind(&self) -> Cow<'_, str> {
        match *self {}
    }

    fn data(&self) -> Cow<'_, [u8]> {
        match *self {}
    }
}
impl AppEventExt for NoEvent {}
