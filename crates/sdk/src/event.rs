use crate::env;
use crate::marker::{AppEvent, AppState};

thread_local! {
    static HANDLER: std::cell::RefCell<fn(Box<dyn AppEventExt>)> = panic!("uninitialized handler");
}

fn emit_event_stub<T: AppEvent>(event: T) {
    println!("{:#}", serde_json::to_string_pretty(&event).unwrap());
}

#[track_caller]
fn handler<E: AppEvent + AppEventExt>(event: Box<dyn AppEventExt>) {
    match E::downcast(event) {
        Ok(event) => emit_event_stub(event),
        Err(event) => env::panic_str(&format!("unexpected event: {:?}", (*event).name())),
    }
}

pub fn register<S: AppState>()
where
    for<'a> S::Event<'a>: AppEventExt,
{
    HANDLER.set(handler::<S::Event<'static>>);
}

#[track_caller]
pub fn emit<E: AppEventExt + 'static>(event: E) {
    HANDLER.with_borrow(|handler| *handler)(Box::new(event))
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

impl AppEvent for () {}
impl AppEventExt for () {}
