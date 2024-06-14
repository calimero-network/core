use std::collections::HashMap;

use crate::sys;

const DATA_REGISTER: sys::RegisterId = sys::RegisterId::new(sys::PtrSizedInt::MAX.as_usize() - 1);

const STATE_KEY: &[u8] = b"STATE";

#[track_caller]
#[inline]
pub fn panic() -> ! {
    unsafe { sys::panic(sys::Location::caller()) }
}

#[track_caller]
#[inline]
pub fn panic_str(message: &str) -> ! {
    unsafe { sys::panic_utf8(sys::Buffer::from(message), sys::Location::caller()) }
}

#[track_caller]
#[inline]
fn expected_register<T>() -> T {
    panic_str("Expected a register to be set, but it was not.");
}

pub fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = match info.payload().downcast_ref::<&'static str>() {
            Some(message) => *message,
            None => match info.payload().downcast_ref::<String>() {
                Some(message) => &**message,
                None => "<no message>",
            },
        };

        unsafe {
            sys::panic_utf8(
                sys::Buffer::from(message),
                sys::Location::from(info.location()),
            )
        }
    }));
}

#[track_caller]
pub fn unreachable() -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();

    #[cfg(not(target_arch = "wasm32"))]
    unreachable!()
}

#[inline(always)]
pub fn register_len(register_id: sys::RegisterId) -> Option<usize> {
    let len = unsafe { sys::register_len(register_id) };

    if len == sys::PtrSizedInt::MAX {
        return None;
    }

    Some(len.as_usize())
}

#[inline]
pub fn read_register(register_id: sys::RegisterId) -> Option<Vec<u8>> {
    let len = register_len(register_id)?;

    let mut buffer = Vec::with_capacity(len);

    unsafe {
        buffer.set_len(len);

        match sys::read_register(register_id, sys::BufferMut::new(&mut buffer)).try_into() {
            Ok(true) => (),
            Ok(false) => panic_str("Buffer is too small."),
            Err(val) => panic_str(&format!("Expected bool as 0|1, got: {}.", val)),
        }
    }

    Some(buffer)
}

#[inline]
pub fn input() -> Option<Vec<u8>> {
    unsafe { sys::input(DATA_REGISTER) };
    read_register(DATA_REGISTER)
}

#[inline]
pub fn value_return<T, E>(result: Result<T, E>)
where
    T: AsRef<[u8]>,
    E: AsRef<[u8]>,
{
    unsafe { sys::value_return(sys::ValueReturn::from(result.as_ref())) }
}

#[inline]
pub fn log(message: &str) {
    unsafe { sys::log_utf8(sys::Buffer::from(message)) }
}

#[inline]
pub fn emit<T: crate::event::AppEvent>(event: T) {
    let kind = event.kind();
    let data = event.data();

    unsafe { sys::emit(sys::Event::new(&kind, &data)) }
}

#[inline]
pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    match unsafe { sys::storage_read(sys::Buffer::from(key), DATA_REGISTER) }.try_into() {
        Ok(false) => None,
        Ok(true) => Some(read_register(DATA_REGISTER).unwrap_or_else(expected_register)),
        Err(val) => panic_str(&format!("Expected bool as 0|1, got: {}.", val)),
    }
}

pub fn state_read<T: crate::state::AppState>() -> Option<T> {
    let data = storage_read(STATE_KEY)?;
    match borsh::from_slice(&data) {
        Ok(state) => Some(state),
        Err(err) => panic_str(&format!("Cannot deserialize app state: {:?}", err)),
    }
}

#[inline]
pub fn storage_write(key: &[u8], value: &[u8]) -> bool {
    unsafe {
        sys::storage_write(
            sys::Buffer::from(key),
            sys::Buffer::from(value),
            DATA_REGISTER,
        )
        .try_into()
    }
    .unwrap_or_else(|val| panic_str(&format!("Expected bool as 0|1, got: {}.", val)))
}

pub fn state_write<T: crate::state::AppState>(state: &T) {
    let data = match borsh::to_vec(state) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize app state: {:?}", err)),
    };
    storage_write(STATE_KEY, &data);
}

pub mod internal {
    use super::*;

    #[inline]
    pub unsafe fn fetch(
        method: &str,
        url: &str,
        headers: HashMap<String, String>,
        body: &[u8],
    ) -> Result<Vec<u8>, String> {
        let headers = match borsh::to_vec(&headers) {
            Ok(data) => data,
            Err(err) => panic_str(&format!("Cannot serialize headers: {:?}", err)),
        };
        let method = sys::Buffer::from(method);
        let url = sys::Buffer::from(url);
        let headers = sys::Buffer::from(headers.as_slice());
        let body = sys::Buffer::from(body);
        match unsafe { sys::fetch(method, url, headers, body, DATA_REGISTER).try_into() } {
            Ok(true) => {
                let data = read_register(DATA_REGISTER).unwrap_or_else(expected_register);
                Ok(data)
            }
            Ok(false) => {
                let data = read_register(DATA_REGISTER).unwrap_or_else(expected_register);
                Err(String::from_utf8(data).unwrap_or_else(|_| {
                    panic_str("Cannot convert fetch response to UTF-8 string.")
                }))
            }
            Err(val) => panic_str(&format!("Expected bool as 0|1, got: {}.", val)),
        }
    }
}
