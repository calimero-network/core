use std::panic::set_hook;

use borsh::{from_slice as from_borsh_slice, to_vec as to_borsh_vec};

use crate::event::AppEvent;
use crate::state::AppState;
use crate::sys::{
    log_utf8, panic_utf8, Buffer, BufferMut, Event, Location, PtrSizedInt, RegisterId, ValueReturn,
};
use crate::{sys, Id};

#[doc(hidden)]
pub mod ext;

const DATA_REGISTER: RegisterId = RegisterId::new(PtrSizedInt::MAX.as_usize() - 1);

const ROOT_STATE_KEY: Id = Id([0; 32]);

#[track_caller]
#[inline]
pub fn panic() -> ! {
    unsafe { sys::panic(Location::caller()) }
}

#[track_caller]
#[inline]
pub fn panic_str(message: &str) -> ! {
    unsafe { panic_utf8(Buffer::from(message), Location::caller()) }
}

#[track_caller]
#[inline]
fn expected_register<T>() -> T {
    panic_str("Expected a register to be set, but it was not.");
}

#[track_caller]
#[inline]
fn expected_boolean(e: u32) -> bool {
    panic_str(&format!("Expected 0|1. Got {e}"));
}

pub fn setup_panic_hook() {
    set_hook(Box::new(|info| {
        #[expect(clippy::option_if_let_else, reason = "Clearer this way")]
        let message = match info.payload().downcast_ref::<&'static str>() {
            Some(message) => *message,
            None => info
                .payload()
                .downcast_ref::<String>()
                .map_or("<no message>", |message| &**message),
        };

        unsafe { panic_utf8(Buffer::from(message), Location::from(info.location())) }
    }));
}

#[track_caller]
pub fn unreachable() -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();

    #[cfg(not(target_arch = "wasm32"))]
    unreachable!()
}

#[inline]
#[must_use]
pub fn register_len(register_id: RegisterId) -> Option<usize> {
    let len = unsafe { sys::register_len(register_id) };

    if len == PtrSizedInt::MAX {
        return None;
    }

    Some(len.as_usize())
}

#[inline]
pub fn read_register(register_id: RegisterId) -> Option<Vec<u8>> {
    let len = register_len(register_id)?;

    let mut buffer = Vec::with_capacity(len);

    let succeed = unsafe {
        buffer.set_len(len);

        sys::read_register(register_id, BufferMut::new(&mut buffer))
    };

    let succeed = succeed.try_into().unwrap_or_else(expected_boolean);

    if !succeed {
        panic_str("Buffer is too small.");
    }

    Some(buffer)
}

#[inline]
fn read_register_sized<const N: usize>(register_id: RegisterId) -> Option<[u8; N]> {
    let len = register_len(register_id)?;

    let mut buffer = [0; N];

    let succeed = unsafe { sys::read_register(register_id, BufferMut::new(&mut buffer)) };

    let succeed = succeed.try_into().unwrap_or_else(expected_boolean);

    if !succeed {
        panic_str(&format!(
            "register content length ({len}) does not match buffer length ({N})"
        ));
    }

    Some(buffer)
}

#[must_use]
pub fn executor_id() -> [u8; 32] {
    unsafe { sys::executor_id(DATA_REGISTER) }

    read_register_sized(DATA_REGISTER).expect("Must have executor identity.")
}

#[inline]
#[must_use]
pub fn input() -> Option<Vec<u8>> {
    unsafe { sys::input(DATA_REGISTER) }

    read_register(DATA_REGISTER)
}

#[inline]
pub fn value_return<T, E>(result: &Result<T, E>)
where
    T: AsRef<[u8]>,
    E: AsRef<[u8]>,
{
    unsafe { sys::value_return(ValueReturn::from(result.as_ref())) }
}

#[inline]
pub fn log(message: &str) {
    unsafe { log_utf8(Buffer::from(message)) }
}

#[inline]
pub fn emit<T: AppEvent>(event: &T) {
    let kind = event.kind();
    let data = event.data();

    unsafe { sys::emit(Event::new(&kind, &data)) }
}

#[inline]
pub fn storage_create(is_collection: bool) -> Id {
    unsafe { sys::storage_create(is_collection.into(), DATA_REGISTER) };

    Id(read_register_sized(DATA_REGISTER).expect("Must have storage identity."))
}

#[inline]
pub fn storage_read(key: Id) -> Option<Vec<u8>> {
    unsafe { sys::storage_read(Buffer::from(&*key), DATA_REGISTER) }
        .try_into()
        .unwrap_or_else(expected_boolean)
        .then(|| read_register(DATA_REGISTER).unwrap_or_else(expected_register))
}

#[must_use]
pub fn state_read<T: AppState>() -> Option<T> {
    let data = storage_read(ROOT_STATE_KEY)?;

    match from_borsh_slice(&data) {
        Ok(state) => Some(state),
        Err(err) => panic_str(&format!("Cannot deserialize app state: {err:?}")),
    }
}

#[inline]
pub fn storage_write(key: Id, value: &[u8]) -> bool {
    unsafe {
        sys::storage_write(Buffer::from(&*key), Buffer::from(value), DATA_REGISTER).try_into()
    }
    .unwrap_or_else(expected_boolean)
}

pub fn state_write<T: AppState>(state: &T) {
    let data = match to_borsh_vec(state) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize app state: {err:?}")),
    };

    let _ = storage_write(ROOT_STATE_KEY, &data);
}

#[inline]
pub fn storage_delete(key: Id) -> bool {
    unsafe { sys::storage_delete(Buffer::from(&*key), DATA_REGISTER).try_into() }
        .unwrap_or_else(expected_boolean)
}

#[inline]
pub fn storage_adopt(key: Id, parent: Id) -> bool {
    unsafe { sys::storage_adopt(Buffer::from(&*key), Buffer::from(&*parent)) }
        .try_into()
        .unwrap_or_else(expected_boolean)
}

#[inline]
pub fn storage_orphan(key: Id, parent: Id) -> bool {
    unsafe { sys::storage_orphan(Buffer::from(&*key), Buffer::from(&*parent)) }
        .try_into()
        .unwrap_or_else(expected_boolean)
}
