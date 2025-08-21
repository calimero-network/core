use std::panic::set_hook;

use calimero_sys::{
    self as sys, Buffer, BufferMut, Event, Location, PtrSizedInt, Ref, RegisterId, ValueReturn,
};

use crate::event::AppEvent;

#[doc(hidden)]
pub mod ext;

const DATA_REGISTER: RegisterId = RegisterId::new(PtrSizedInt::MAX.as_usize() - 1);

#[track_caller]
#[inline]
pub fn panic() -> ! {
    unsafe { sys::panic(Ref::new(&Location::caller())) }
}

#[track_caller]
#[inline]
pub fn panic_str(message: &str) -> ! {
    unsafe {
        sys::panic_utf8(
            Ref::new(&Buffer::from(message)),
            Ref::new(&Location::caller()),
        )
    }
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

        unsafe {
            sys::panic_utf8(
                Ref::new(&Buffer::from(message)),
                Ref::new(&Location::from(info.location())),
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

        sys::read_register(register_id, Ref::new(&BufferMut::new(&mut buffer)))
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

    #[expect(
        clippy::needless_borrows_for_generic_args,
        reason = "we don't want to copy the buffer, but write to the same one that's returned"
    )]
    let succeed: bool = unsafe {
        sys::read_register(register_id, Ref::new(&BufferMut::new(&mut buffer)))
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
    unsafe { sys::value_return(Ref::new(&ValueReturn::from(result.as_ref()))) }
}

#[inline]
pub fn log(message: &str) {
    unsafe { sys::log_utf8(Ref::new(&Buffer::from(message))) }
}

#[inline]
pub fn emit<T: AppEvent>(event: &T) {
    let kind = event.kind();
    let data = event.data();

    unsafe { sys::emit(Ref::new(&Event::new(&kind, &data))) }
}

pub fn commit(root_hash: &[u8; 32], artifact: &[u8]) {
    unsafe {
        sys::commit(
            Ref::new(&Buffer::from(&root_hash[..])),
            Ref::new(&Buffer::from(artifact)),
        )
    }
}

#[inline]
pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    unsafe { sys::storage_read(Ref::new(&Buffer::from(key)), DATA_REGISTER) }
        .try_into()
        .unwrap_or_else(expected_boolean::<bool>)
        .then(|| read_register(DATA_REGISTER).unwrap_or_else(expected_register))
}

#[inline]
pub fn storage_remove(key: &[u8]) -> bool {
    unsafe { sys::storage_remove(Ref::new(&Buffer::from(key)), DATA_REGISTER).try_into() }
        .unwrap_or_else(expected_boolean)
}

#[inline]
pub fn storage_write(key: &[u8], value: &[u8]) -> bool {
    unsafe {
        sys::storage_write(
            Ref::new(&Buffer::from(key)),
            Ref::new(&Buffer::from(value)),
            DATA_REGISTER,
        )
        .try_into()
    }
    .unwrap_or_else(expected_boolean)
}

/// Fill the buffer with random bytes.
#[inline]
pub fn random_bytes(buf: &mut [u8]) {
    unsafe { sys::random_bytes(Ref::new(&BufferMut::new(buf))) }
}

/// Gets the current time.
#[inline]
#[must_use]
pub fn time_now() -> u64 {
    let mut bytes = [0; 8];

    #[expect(
        clippy::needless_borrows_for_generic_args,
        reason = "we don't want to copy the buffer, but write to the same one that's returned"
    )]
    unsafe {
        sys::time_now(Ref::new(&BufferMut::new(&mut bytes)));
    }

    u64::from_le_bytes(bytes)
}

// ========================================
// STREAMING BLOB API
// ========================================

/// Create a new blob write handle for streaming data.
/// Returns a file descriptor that can be used with blob_write() and blob_close().
pub fn blob_create() -> u64 {
    unsafe { sys::blob_create() }.as_usize() as u64
}

/// Open a blob for reading by its 32-byte ID.
/// Returns a file descriptor that can be used with blob_read() and blob_close().
/// Returns 0 if the blob is not found.
pub fn blob_open(blob_id: &[u8; 32]) -> u64 {
    unsafe { sys::blob_open(Ref::new(&Buffer::from(&blob_id[..]))) }.as_usize() as u64
}

/// Read data from a blob handle opened with blob_open().
/// Reads into the provided buffer and returns the number of bytes read.
/// Returns 0 when end of blob is reached.
pub fn blob_read(fd: u64, buffer: &mut [u8]) -> u64 {
    unsafe {
        sys::blob_read(
            PtrSizedInt::new(fd as usize),
            Ref::new(&BufferMut::new(buffer)),
        )
    }
    .as_usize() as u64
}

/// Write data to a blob handle created with blob_create().
/// Returns the number of bytes written.
pub fn blob_write(fd: u64, data: &[u8]) -> u64 {
    unsafe { sys::blob_write(PtrSizedInt::new(fd as usize), Ref::new(&Buffer::from(data))) }
        .as_usize() as u64
}

/// Close a blob handle and finalize the blob.
/// For write handles: Finalizes the blob and returns its 32-byte ID.
/// For read handles: Returns the original blob's ID and cleans up the handle.
/// Panics if the operation fails (e.g. blob finalization fails for write handles).
pub fn blob_close(fd: u64) -> [u8; 32] {
    let mut blob_id_buf = [0u8; 32];
    let success: bool = unsafe {
        sys::blob_close(
            PtrSizedInt::new(fd as usize),
            Ref::new(&BufferMut::new(&mut blob_id_buf)),
        )
        .try_into()
    }
    .unwrap_or_else(expected_boolean);

    if success {
        blob_id_buf
    } else {
        panic_str("Blob operation failed")
    }
}

// ========================================
// NETWORK-AWARE BLOB API
// ========================================

/// Announce a blob to a specific context for network discovery.
/// This makes the blob discoverable by other nodes in the context.
/// Returns true if the announcement was successful.
///
/// # Security
/// For security reasons, a context can only announce blobs to itself.
/// If the provided context_id doesn't match the current context, this function returns false.
///
/// # Arguments
/// * `blob_id` - The 32-byte ID of the blob to announce
/// * `target_context_id` - The 32-byte ID of the context to announce the blob in (must match current context)
///
/// # Example
/// ```no_run
/// use calimero_sdk::env;
///
/// // Create and write a blob
/// let fd = env::blob_create();
/// env::blob_write(fd, b"Hello, World!");
/// let blob_id = env::blob_close(fd);
///
/// // Announce it to the current context
/// let current_context = env::context_id();
/// let announced = env::blob_announce_to_context(&blob_id, &current_context);
/// ```
pub fn blob_announce_to_context(blob_id: &[u8; 32], target_context_id: &[u8; 32]) -> bool {
    // Security check: only allow announcing to the current context
    let current_context = context_id();
    if current_context != *target_context_id {
        // Attempting to announce to a different context is not allowed
        return false;
    }

    unsafe {
        sys::blob_announce_to_context(
            Buffer::from(&blob_id[..]),
            Buffer::from(&target_context_id[..]),
        )
        .try_into()
    }
    .unwrap_or_else(expected_boolean)
}
