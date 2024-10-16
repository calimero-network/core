use std::panic::set_hook;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use borsh::{from_slice as from_borsh_slice, to_vec as to_borsh_vec};
use uuid::Uuid;

use crate::event::AppEvent;
use crate::state::AppState;
use crate::sys;
use crate::sys::{
    log_utf8, panic_utf8, Buffer, BufferMut, Event, Location, PtrSizedInt, RegisterId, ValueReturn,
};

#[doc(hidden)]
pub mod ext;

const DATA_REGISTER: RegisterId = RegisterId::new(PtrSizedInt::MAX.as_usize() - 1);

const STATE_KEY: &[u8] = b"STATE";

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
fn expected_boolean<T>(e: u32) -> T {
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

    let succeed: bool = unsafe {
        buffer.set_len(len);

        sys::read_register(register_id, BufferMut::new(&mut buffer))
            .try_into()
            .unwrap_or_else(expected_boolean)
    };

    if !succeed {
        panic_str("Buffer is too small.");
    }

    Some(buffer)
}

#[inline]
fn read_register_sized<const N: usize>(register_id: RegisterId) -> Option<[u8; N]> {
    let len = register_len(register_id)?;

    let mut buffer = [0; N];

    let succeed: bool = unsafe {
        sys::read_register(register_id, BufferMut::new(&mut buffer))
            .try_into()
            .unwrap_or_else(expected_boolean)
    };

    if !succeed {
        panic_str(&format!(
            "register content length ({len}) does not match buffer length ({N})"
        ));
    }

    Some(buffer)
}

#[must_use]
pub fn context_id() -> [u8; 32] {
    unsafe { sys::context_id(DATA_REGISTER) }
    read_register_sized(DATA_REGISTER).expect("Must have context identity.")
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
pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    unsafe { sys::storage_read(Buffer::from(key), DATA_REGISTER) }
        .try_into()
        .unwrap_or_else(expected_boolean::<bool>)
        .then(|| read_register(DATA_REGISTER).unwrap_or_else(expected_register))
}

#[must_use]
pub fn state_read<T: AppState>() -> Option<T> {
    let data = storage_read(STATE_KEY)?;
    match from_borsh_slice(&data) {
        Ok(state) => Some(state),
        Err(err) => panic_str(&format!("Cannot deserialize app state: {err:?}")),
    }
}

#[inline]
pub fn storage_write(key: &[u8], value: &[u8]) -> bool {
    unsafe { sys::storage_write(Buffer::from(key), Buffer::from(value), DATA_REGISTER).try_into() }
        .unwrap_or_else(expected_boolean)
}

pub fn state_write<T: AppState>(state: &T) {
    let data = match to_borsh_vec(state) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize app state: {err:?}")),
    };
    let _ = storage_write(STATE_KEY, &data);
}

/// Generate a new random UUID v4.
#[inline]
#[must_use]
pub fn generate_uuid() -> Uuid {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::generate_uuid(DATA_REGISTER) };
        Uuid::from_slice(&read_register_sized::<16>(DATA_REGISTER).expect("Must have UUID"))
            .expect("UUID must be valid")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Uuid::new_v4()
    }
}

/// Gets the current time.
#[inline]
#[must_use]
pub fn time_now() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::time_now(DATA_REGISTER) };
        u64::from_le_bytes(
            read_register(DATA_REGISTER)
                .expect("Must have time")
                .try_into()
                .expect("Invalid time bytes"),
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Impossible to overflow in normal circumstances"
    )]
    #[expect(clippy::expect_used, reason = "Effectively infallible here")]
    {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64
    }
}
