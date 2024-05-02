use std::borrow::Cow;

use crate::env;
use crate::state::AppState;

pub trait AppEvent {
    fn kind<'a>(&'a self) -> Cow<'a, str>;
    fn data<'a>(&'a self) -> Cow<'a, [u8]>;
}

pub struct EncodedAppEvent<'a> {
    pub kind: Cow<'a, str>,
    pub data: Cow<'a, [u8]>,
}

thread_local! {
    static HANDLER: std::cell::RefCell<fn(Box<dyn AppEventExt>)> = panic!("uninitialized handler");
}

#[track_caller]
#[inline(never)]
fn handler<E: AppEvent + AppEventExt>(event: Box<dyn AppEventExt>) {
    if let Ok(event) = E::downcast(event) {
        env::emit(event);
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
    #[derive(PartialEq)]
    pub struct TypeId {
        id: usize,
    }

    #[inline(never)]
    pub fn type_id_of<T: ?Sized>() -> TypeId {
        TypeId {
            id: type_id_of::<T> as usize,
        }
    }

    pub trait Reflect {
        fn id(&self) -> TypeId {
            type_id_of::<Self>()
        }

        fn name(&self) -> &'static str {
            std::any::type_name::<Self>()
        }
    }

    impl<T> Reflect for T {}
}

use reflect::Reflect;

pub trait AppEventExt: Reflect {
    // todo! experiment with &dyn AppEventExt downcast_ref to &Self
    // yes, this will mean delegated downcasting would have to be referential
    // but that's not bad, not one bit
    fn downcast(event: Box<dyn AppEventExt>) -> Result<Self, Box<dyn AppEventExt>>
    where
        Self: Sized,
    {
        downcast(event)
    }
}

impl dyn AppEventExt {
    pub fn is<T: AppEventExt>(&self) -> bool {
        self.id() == reflect::type_id_of::<T>()
    }
}

pub fn downcast<T: AppEventExt>(event: Box<dyn AppEventExt>) -> Result<T, Box<dyn AppEventExt>> {
    if event.is::<T>() {
        Ok(*unsafe { Box::from_raw(Box::into_raw(event) as *mut T) })
    } else {
        Err(event)
    }
}

pub enum NoEvent {}
impl AppEvent for NoEvent {
    fn kind<'a>(&'a self) -> Cow<'a, str> {
        match *self {}
    }

    fn data<'a>(&'a self) -> Cow<'a, [u8]> {
        match *self {}
    }
}
impl AppEventExt for NoEvent {}
