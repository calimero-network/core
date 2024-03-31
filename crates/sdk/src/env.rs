use crate::sys::{self, PtrSized};
pub use sys::RegisterId;

const DATA_REGISTER: sys::RegisterId = sys::RegisterId::new(PtrSized::MAX.as_usize() - 1);

const STATE_KEY: &[u8] = b"STATE";

fn expected_register<T>() -> T {
    panic_str("Expected a register to be set, but it was not.");
}

pub fn panic() -> ! {
    unsafe { sys::panic() }
}

pub fn panic_str(message: &str) -> ! {
    unsafe { sys::panic_utf8(sys::Buffer::from(message)) }
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

        let payload = match info.location() {
            Some(location) => format!("panicked at {}: {}", location, message),
            None => format!("fatal: panicked at unknown location: {}", message),
        };

        panic_str(&payload);
    }));
}

pub fn abort() -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();

    #[cfg(not(target_arch = "wasm32"))]
    panic()
}

pub fn register_len(register_id: sys::RegisterId) -> Option<usize> {
    let len = unsafe { sys::register_len(register_id) };

    if len == PtrSized::MAX {
        return None;
    }

    Some(len.into())
}

pub fn read_register(register_id: RegisterId) -> Option<Vec<u8>> {
    let len = register_len(register_id)?;

    let mut buffer = Vec::with_capacity(len);

    unsafe {
        sys::read_register(register_id, sys::PtrSized::from(buffer.as_mut_ptr()));

        buffer.set_len(len);
    }

    Some(buffer)
}

pub fn input() -> Option<Vec<u8>> {
    unsafe { sys::input(DATA_REGISTER) };
    read_register(DATA_REGISTER)
}

pub fn value_return(result: &[u8]) {
    unsafe { sys::value_return(sys::Buffer::from(result)) }
}

pub fn log(message: &str) {
    unsafe { sys::log_utf8(sys::Buffer::from(message)) }
}

pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    match unsafe { sys::storage_read(sys::Buffer::from(key), DATA_REGISTER) }.as_bool() {
        Some(false) => None,
        Some(true) => Some(read_register(DATA_REGISTER).unwrap_or_else(expected_register)),
        None => panic_str("Storage read failed."),
    }
}

pub fn state_read<T: borsh::BorshDeserialize>() -> Option<T> {
    let data = storage_read(STATE_KEY)?;
    match borsh::from_slice(&data) {
        Ok(state) => Some(state),
        Err(err) => panic_str(&format!("Cannot deserialize app state: {:?}", err)),
    }
}

pub fn storage_write(key: &[u8], value: &[u8]) -> bool {
    unsafe {
        sys::storage_write(
            sys::Buffer::from(key),
            sys::Buffer::from(value),
            DATA_REGISTER,
        )
        .as_bool()
    }
    .unwrap_or_else(|| panic_str("Storage write failed."))
}

pub fn state_write<T: borsh::BorshSerialize>(state: &T) {
    let data = match borsh::to_vec(state) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize app state: {:?}", err)),
    };
    storage_write(STATE_KEY, &data);
}
