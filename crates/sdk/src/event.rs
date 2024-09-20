use core::any::TypeId;
use core::cell::RefCell;
use core::mem::transmute;
use std::borrow::Cow;

use crate::env;
use crate::state::AppState;

pub trait AppEvent {
    fn kind(&self) -> Cow<'_, str>;
    fn data(&self) -> Cow<'_, [u8]>;
}

#[derive(Debug)]
#[non_exhaustive]
pub struct EncodedAppEvent<'a> {
    pub kind: Cow<'a, str>,
    pub data: Cow<'a, [u8]>,
}

thread_local! {
    static HANDLER: RefCell<fn(Box<dyn AppEventExt>)> = panic!("uninitialized handler");
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
    let f: fn(Box<dyn AppEventExt + 'a>) = unsafe { transmute::<_, _>(f) };
    f(Box::new(event));
}

mod reflect {
    use core::any::{type_name, TypeId};

    pub trait Reflect {
        fn id(&self) -> TypeId
        where
            Self: 'static,
        {
            TypeId::of::<Self>()
        }

        fn name(&self) -> &'static str {
            type_name::<Self>()
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
        self.id() == TypeId::of::<T>()
    }
}

pub fn downcast<T: AppEventExt + 'static>(
    event: Box<dyn AppEventExt>,
) -> Result<T, Box<dyn AppEventExt>> {
    if event.is::<T>() {
        Ok(*unsafe { Box::from_raw(Box::into_raw(event).cast::<T>()) })
    } else {
        Err(event)
    }
}

#[derive(Clone, Copy, Debug)]
#[expect(clippy::exhaustive_enums, reason = "This will never have variants")]
pub enum NoEvent {}
impl AppEvent for NoEvent {
    fn kind(&self) -> Cow<'_, str> {
        unreachable!()
    }

    fn data(&self) -> Cow<'_, [u8]> {
        unreachable!()
    }
}
impl AppEventExt for NoEvent {}
