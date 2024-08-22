use crate::sys;

#[doc(hidden)]
pub mod ext;

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

#[track_caller]
#[inline]
fn expected_boolean<T>(e: u32) -> T {
    panic_str(&format!("Expected 0|1. Got {e}"));
}

#[must_use]
pub fn get_executor_identity() -> [u8; 32] {
    unsafe { sys::get_executor_identity(DATA_REGISTER) };
    read_register_sized(DATA_REGISTER).expect("Must have executor identity.")
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

#[inline]
#[must_use]
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

    let succeed: bool = unsafe {
        buffer.set_len(len);

        sys::read_register(register_id, sys::BufferMut::new(&mut buffer))
            .try_into()
            .unwrap_or_else(expected_boolean)
    };

    if !succeed {
        panic_str("Buffer is too small.");
    }

    Some(buffer)
}

#[inline]
fn read_register_sized<const N: usize>(register_id: sys::RegisterId) -> Option<[u8; N]> {
    let len = register_len(register_id)?;
    let buffer = [0; N];
    let succeed: bool = unsafe {
        sys::read_register(register_id, sys::BufferMut::new(buffer))
            .try_into()
            .unwrap_or_else(expected_boolean)
    };

    if !succeed {
        panic_str(&format!(
            "register content length ({}) does not match buffer length ({})",
            len, N
        ));
    }

    Some(buffer)
}

#[inline]
#[must_use]
pub fn input() -> Option<Vec<u8>> {
    unsafe { sys::input(DATA_REGISTER) };
    read_register(DATA_REGISTER)
}

#[inline]
pub fn value_return<T, E>(result: &Result<T, E>)
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
pub fn emit<T: crate::event::AppEvent>(event: &T) {
    let kind = event.kind();
    let data = event.data();

    unsafe { sys::emit(sys::Event::new(&kind, &data)) }
}

#[inline]
pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    unsafe { sys::storage_read(sys::Buffer::from(key), DATA_REGISTER) }
        .try_into()
        .unwrap_or_else(expected_boolean::<bool>)
        .then(|| read_register(DATA_REGISTER).unwrap_or_else(expected_register))
}

#[must_use]
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
    .unwrap_or_else(expected_boolean)
}

pub fn state_write<T: crate::state::AppState>(state: &T) {
    let data = match borsh::to_vec(state) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize app state: {:?}", err)),
    };
    let _ = storage_write(STATE_KEY, &data);
}
