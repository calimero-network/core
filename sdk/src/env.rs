use crate::sys;

const DATA_REGISTER: u64 = u64::MAX - 1;

const STATE_KEY: &[u8] = b"STATE";

fn expected_register<T>() -> T {
    panic_str("Expected a register to be set, but it was not.");
}

pub fn abort() -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();

    #[cfg(not(target_arch = "wasm32"))]
    unsafe {
        sys::panic()
    }
}

pub fn panic_str(message: &str) -> ! {
    unsafe { sys::panic_utf8(message.len() as u64, message.as_ptr() as u64) }
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

pub fn read_register(register_id: u64) -> Option<Vec<u8>> {
    let len = match register_len(register_id)?.try_into() {
        Ok(len) => len,
        Err(_) => abort(),
    };

    let mut buffer = Vec::with_capacity(len);

    unsafe {
        sys::read_register(register_id, buffer.as_mut_ptr() as u64);

        buffer.set_len(len);
    }

    Some(buffer)
}

pub fn register_len(register_id: u64) -> Option<u64> {
    let len = unsafe { sys::register_len(register_id) };

    if len == std::u64::MAX {
        None
    } else {
        Some(len)
    }
}

pub fn input() -> Option<Vec<u8>> {
    unsafe { sys::input(DATA_REGISTER) };
    read_register(DATA_REGISTER)
}

pub fn value_return(value: &[u8]) {
    unsafe { sys::value_return(value.len() as _, value.as_ptr() as _) }
}

pub fn log(message: &str) {
    unsafe { sys::log_utf8(message.len() as _, message.as_ptr() as _) }
}

pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    match unsafe { sys::storage_read(key.len() as _, key.as_ptr() as _, DATA_REGISTER) } {
        0 => None,
        1 => Some(read_register(DATA_REGISTER).unwrap_or_else(expected_register)),
        _ => abort(),
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
    match unsafe {
        sys::storage_write(
            key.len() as _,
            key.as_ptr() as _,
            value.len() as _,
            value.as_ptr() as _,
            DATA_REGISTER,
        )
    } {
        0 => false,
        1 => true,
        _ => abort(),
    }
}

pub fn state_write<T: borsh::BorshSerialize>(state: &T) {
    let data = match borsh::to_vec(state) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize app state: {:?}", err)),
    };
    storage_write(STATE_KEY, &data);
}
