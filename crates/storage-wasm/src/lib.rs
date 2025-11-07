#![allow(clippy::missing_errors_doc)]

use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::slice;
use std::sync::Mutex;

use borsh::{to_vec, BorshDeserialize};
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::env::time_now;
use calimero_storage::index::Index;
use calimero_storage::interface::{Interface, StorageError};
use calimero_storage::js::JsUnorderedMap;
use calimero_storage::store::MainStorage;
use once_cell::sync::Lazy;

static LAST_ERROR: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

static INIT_SENTINEL: Lazy<()> = Lazy::new(|| {
    log_message("[dispatcher][storage-wasm] INITIALIZING shim");
    panic::set_hook(Box::new(|info| {
        let payload = if let Some(message) = info.payload().downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "unknown panic payload".to_owned()
        };
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        log_message(&format!(
            "[dispatcher][storage-wasm] panic hook: {payload} @ {location}"
        ));
    }));
});

#[inline]
fn log_message(message: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        calimero_sdk::env::log(message);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        eprintln!("storage-wasm: {message}");
    }
}

macro_rules! log_trace {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        log_message(&format!("[dispatcher][storage-wasm] {}", msg));
    }};
}

fn set_error<E: std::fmt::Display>(err: E) -> i32 {
    let _ = Lazy::force(&INIT_SENTINEL);
    log_trace!("set_error: {err}");
    if let Ok(mut guard) = LAST_ERROR.lock() {
        *guard = Some(err.to_string());
    }
    -1
}

fn clear_error() {
    if let Ok(mut guard) = LAST_ERROR.lock() {
        drop(guard.take());
    }
}

#[no_mangle]
pub extern "C" fn cs_clear_error() {
    log_trace!("cs_clear_error: clearing last error");
    clear_error();
}

fn read_id(id_ptr: u32, id_len: u32, context: &str) -> Result<Id, i32> {
    if id_len != 32 {
        return Err(set_error(format!("{context} expects 32-byte ID")));
    }
    if id_ptr == 0 {
        return Err(set_error(format!("{context} received null pointer")));
    }

    let id_bytes = unsafe { slice::from_raw_parts(id_ptr as *const u8, 32) };

    let mut id_array = [0_u8; 32];
    id_array.copy_from_slice(id_bytes);
    Ok(Id::new(id_array))
}

fn read_bytes(ptr: u32, len: u32, context: &str) -> Result<Vec<u8>, i32> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if ptr == 0 {
        return Err(set_error(format!("{context} received null pointer")));
    }

    let bytes = unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) };
    Ok(bytes.to_vec())
}

fn write_id(id: &Id, out_ptr: u32) {
    if out_ptr == 0 {
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(id.as_bytes().as_ptr(), out_ptr as *mut u8, 32);
    }
    log_trace!("write_id: id={id} out_ptr={out_ptr}");
}

fn write_len(out_len_ptr: u32, len: u32) {
    if out_len_ptr == 0 {
        return;
    }
    unsafe {
        *(out_len_ptr as *mut u32) = len;
    }
    log_trace!("write_len: ptr={out_len_ptr} len={len}");
}

fn load_js_map(id: Id) -> Result<JsUnorderedMap, i32> {
    log_trace!("load_js_map: id={id}");
    match JsUnorderedMap::load(id) {
        Ok(Some(map)) => {
            log_trace!("load_js_map: id={} loaded", map.id());
            Ok(map)
        }
        Ok(None) => {
            log_trace!("load_js_map: id={id} not found");
            Err(set_error("map not found"))
        }
        Err(err) => {
            log_trace!("load_js_map: id={id} error={err}");
            Err(set_error(err))
        }
    }
}

fn ensure_root_index() -> Result<(), i32> {
    match Index::<MainStorage>::get_hashes_for(Id::root()) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => {
            let timestamp = time_now();
            let metadata = Metadata::new(timestamp, timestamp);
            Index::<MainStorage>::add_root(ChildInfo::new(Id::root(), [0; 32], metadata))
                .map_err(set_error)
        }
        Err(err) => Err(set_error(err)),
    }
}

fn save_js_map(map: &mut JsUnorderedMap) -> Result<(), i32> {
    log_trace!("save_js_map: id={}", map.id());
    match map.save() {
        Ok(_) => Ok(()),
        Err(StorageError::CannotCreateOrphan(_)) => {
            log_trace!(
                "save_js_map: id={} orphan detected, attempting to attach to root",
                map.id()
            );
            if let Err(code) = ensure_root_index() {
                return Err(code);
            }
            match Interface::<MainStorage>::add_child_to(Id::root(), map) {
                Ok(_) => Ok(()),
                Err(StorageError::CannotCreateOrphan(_)) => {
                    log_trace!("save_js_map: id={} still orphan after attach", map.id());
                    Err(set_error("cannot create orphan"))
                }
                Err(err) => {
                    log_trace!("save_js_map: id={} attach error={err}", map.id());
                    Err(set_error(err))
                }
            }
        }
        Err(err) => {
            log_trace!("save_js_map: id={} error={err}", map.id());
            Err(set_error(err))
        }
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_owned()
    }
}

#[no_mangle]
pub extern "C" fn cs_alloc(size: u32) -> u32 {
    let mut buf = Vec::<u8>::with_capacity(size as usize);
    let ptr = buf.as_mut_ptr();
    mem::forget(buf);
    ptr as u32
}

#[no_mangle]
pub extern "C" fn cs_dealloc(ptr: u32, length: u32) {
    if ptr == 0 || length == 0 {
        return;
    }
    unsafe {
        drop(Vec::from_raw_parts(
            ptr as *mut u8,
            length as usize,
            length as usize,
        ));
    }
}

#[no_mangle]
pub extern "C" fn cs_storage_read(id_ptr: u32, id_len: u32, out_len_ptr: u32) -> u32 {
    clear_error();
    log_trace!("cs_storage_read: start id_len={id_len}");

    let id = match read_id(id_ptr, id_len, "storage_read") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_storage_read: invalid id");
            return 0;
        }
    };

    match Interface::<MainStorage>::find_by_id_raw(id) {
        Some(data) => {
            let len = data.len() as u32;
            let ptr = cs_alloc(len);
            unsafe {
                let dst = ptr as *mut u8;
                std::ptr::copy_nonoverlapping(data.as_ptr(), dst, len as usize);
                let len_ptr = out_len_ptr as *mut u32;
                *len_ptr = len;
            }
            log_trace!("cs_storage_read: found len={len}");
            ptr
        }
        None => {
            unsafe {
                *(out_len_ptr as *mut u32) = 0;
            }
            log_trace!("cs_storage_read: miss");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_storage_save(
    id_ptr: u32,
    id_len: u32,
    data_ptr: u32,
    data_len: u32,
    created_at: u64,
    updated_at: u64,
) -> i32 {
    clear_error();
    log_trace!("cs_storage_save: start id_len={id_len} data_len={data_len}");

    let id = match read_id(id_ptr, id_len, "storage_save") {
        Ok(id) => id,
        Err(code) => return code,
    };

    let data = match read_bytes(data_ptr, data_len, "storage_save data") {
        Ok(data) => data,
        Err(code) => return code,
    };
    let metadata = Metadata::new(created_at, updated_at);

    match Interface::<MainStorage>::save_raw(id, data, metadata) {
        Ok(_hash) => {
            log_trace!("cs_storage_save: success");
            1
        }
        Err(StorageError::CannotCreateOrphan(_)) => {
            log_trace!("cs_storage_save: cannot create orphan");
            set_error("cannot create orphan")
        }
        Err(err) => {
            log_trace!("cs_storage_save: error={err}");
            set_error(err)
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_metadata_new_serialized(out_len_ptr: u32) -> u32 {
    clear_error();
    log_trace!("cs_metadata_new_serialized: ENTRY");
    let timestamp = time_now();
    let metadata = Metadata::new(timestamp, timestamp);

    match to_vec(&metadata) {
        Ok(bytes) => {
            let len = bytes.len() as u32;
            let ptr = cs_alloc(len);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                *(out_len_ptr as *mut u32) = len;
            }
            log_trace!("cs_metadata_new_serialized: len={len}");
            ptr
        }
        Err(err) => {
            log_trace!("cs_metadata_new_serialized: error={err}");
            let _ = set_error(err);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_storage_metadata(id_ptr: u32, id_len: u32, out_len_ptr: u32) -> u32 {
    clear_error();
    log_trace!("cs_storage_metadata: start id_len={id_len}");

    let id = match read_id(id_ptr, id_len, "storage_metadata") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_storage_metadata: invalid id");
            return 0;
        }
    };

    match Interface::<MainStorage>::generate_comparison_data(Some(id)) {
        Ok(data) => match to_vec(&data.metadata) {
            Ok(bytes) => {
                let len = bytes.len() as u32;
                let ptr = cs_alloc(len);
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                    *(out_len_ptr as *mut u32) = len;
                }
                log_trace!("cs_storage_metadata: len={len}");
                ptr
            }
            Err(err) => {
                log_trace!("cs_storage_metadata: error serializing metadata: {err}");
                let _ = set_error(err);
                0
            }
        },
        Err(err) => {
            log_trace!("cs_storage_metadata: generate_comparison_data error={err}");
            let _ = set_error(err);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_last_error_length() -> u32 {
    let len = if let Ok(guard) = LAST_ERROR.lock() {
        guard.as_ref().map(|err| err.len() as u32).unwrap_or(0)
    } else {
        0
    };
    log_trace!("cs_last_error_length: len={len}");
    len
}

#[no_mangle]
pub extern "C" fn cs_last_error(buffer_ptr: u32, buffer_len: u32) -> u32 {
    log_trace!("cs_last_error: start buffer_len={buffer_len}");
    let error_message = LAST_ERROR.lock().ok().and_then(|guard| guard.clone());

    match error_message {
        Some(err) => {
            let err_bytes = err.as_bytes();
            let len = err_bytes.len();
            if len as u32 > buffer_len {
                let _ = set_error("buffer too small");
                return 0;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(err_bytes.as_ptr(), buffer_ptr as *mut u8, len);
            }
            log_trace!("cs_last_error: wrote len={len}");
            len as u32
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn cs_metadata_deserialize(
    metadata_ptr: u32,
    metadata_len: u32,
    created_at_ptr: u32,
    updated_at_ptr: u32,
) -> i32 {
    clear_error();
    log_trace!("cs_metadata_deserialize: start metadata_len={metadata_len}");
    let metadata_bytes =
        unsafe { slice::from_raw_parts(metadata_ptr as *const u8, metadata_len as usize) };

    match Metadata::try_from_slice(metadata_bytes) {
        Ok(metadata) => {
            unsafe {
                *(created_at_ptr as *mut u64) = metadata.created_at();
                *(updated_at_ptr as *mut u64) = metadata.updated_at();
            }
            log_trace!("cs_metadata_deserialize: success");
            1
        }
        Err(err) => set_error(err),
    }
}

#[no_mangle]
pub extern "C" fn cs_time_now() -> u64 {
    let now = time_now();
    log_trace!("cs_time_now: {now}");
    now
}

#[no_mangle]
pub extern "C" fn cs_map_new(out_id_ptr: u32) -> i32 {
    clear_error();
    log_trace!("cs_map_new: start");

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut map = JsUnorderedMap::new();

        if let Err(code) = save_js_map(&mut map) {
            log_trace!("cs_map_new: save failed code={code}");
            return Err(code);
        }

        write_id(&map.id(), out_id_ptr);
        log_trace!("cs_map_new: created id={}", map.id());
        Ok(())
    }));

    match result {
        Ok(Ok(())) => 1,
        Ok(Err(code)) => code,
        Err(payload) => {
            let message = panic_message(payload);
            log_trace!("cs_map_new: panic {message}");
            set_error(message)
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_map_get(
    map_id_ptr: u32,
    map_id_len: u32,
    key_ptr: u32,
    key_len: u32,
    out_len_ptr: u32,
) -> u32 {
    clear_error();
    log_trace!("cs_map_get: start map_id_ptr={map_id_ptr} key_len={key_len}");

    let id = match read_id(map_id_ptr, map_id_len, "map_get") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_get: invalid id");
            return 0;
        }
    };

    let key = match read_bytes(key_ptr, key_len, "map_get key") {
        Ok(key) => key,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_get: invalid key pointer/length");
            return 0;
        }
    };
    log_trace!("cs_map_get: id={id} key_len={}", key.len());

    let map = match load_js_map(id) {
        Ok(map) => map,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_get: load failed");
            return 0;
        }
    };

    match map.get(&key) {
        Ok(Some(value)) => {
            let len = value.len() as u32;
            let ptr = cs_alloc(len);
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), ptr as *mut u8, value.len());
            }
            write_len(out_len_ptr, len);
            log_trace!("cs_map_get: id={} hit len={len}", map.id());
            ptr
        }
        Ok(None) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_get: id={} miss", map.id());
            0
        }
        Err(err) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_get: id={} error={err}", map.id());
            let _ = set_error(err);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_map_insert(
    map_id_ptr: u32,
    map_id_len: u32,
    key_ptr: u32,
    key_len: u32,
    value_ptr: u32,
    value_len: u32,
    out_prev_len_ptr: u32,
) -> u32 {
    clear_error();
    log_trace!("cs_map_insert: start key_len={key_len} value_len={value_len}");

    let id = match read_id(map_id_ptr, map_id_len, "map_insert") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_prev_len_ptr, 0);
            log_trace!("cs_map_insert: invalid id");
            return 0;
        }
    };

    let key = match read_bytes(key_ptr, key_len, "map_insert key") {
        Ok(key) => key,
        Err(_) => {
            write_len(out_prev_len_ptr, 0);
            log_trace!("cs_map_insert: invalid key pointer/length");
            return 0;
        }
    };
    let value = match read_bytes(value_ptr, value_len, "map_insert value") {
        Ok(value) => value,
        Err(_) => {
            write_len(out_prev_len_ptr, 0);
            log_trace!("cs_map_insert: invalid value pointer/length");
            return 0;
        }
    };
    log_trace!(
        "cs_map_insert: id={} key_len={} value_len={}",
        id,
        key.len(),
        value.len()
    );

    let mut map = match load_js_map(id) {
        Ok(map) => map,
        Err(_) => {
            write_len(out_prev_len_ptr, 0);
            log_trace!("cs_map_insert: load failed");
            return 0;
        }
    };

    match map.insert(&key, &value) {
        Ok(prev) => {
            if let Err(_) = save_js_map(&mut map) {
                write_len(out_prev_len_ptr, 0);
                log_trace!("cs_map_insert: save failed");
                return 0;
            }

            if let Some(previous) = prev {
                let len = previous.len() as u32;
                let ptr = cs_alloc(len);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        previous.as_ptr(),
                        ptr as *mut u8,
                        previous.len(),
                    );
                }
                write_len(out_prev_len_ptr, len);
                log_trace!("cs_map_insert: id={} replaced prev_len={len}", map.id());
                ptr
            } else {
                write_len(out_prev_len_ptr, 0);
                log_trace!("cs_map_insert: id={} inserted new entry", map.id());
                0
            }
        }
        Err(err) => {
            write_len(out_prev_len_ptr, 0);
            log_trace!("cs_map_insert: id={} error={err}", map.id());
            let _ = set_error(err);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_map_remove(
    map_id_ptr: u32,
    map_id_len: u32,
    key_ptr: u32,
    key_len: u32,
    out_len_ptr: u32,
) -> u32 {
    clear_error();
    log_trace!("cs_map_remove: start key_len={key_len}");

    let id = match read_id(map_id_ptr, map_id_len, "map_remove") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_remove: invalid id");
            return 0;
        }
    };

    let key = match read_bytes(key_ptr, key_len, "map_remove key") {
        Ok(key) => key,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_remove: invalid key pointer/length");
            return 0;
        }
    };
    log_trace!("cs_map_remove: id={} key_len={}", id, key.len());

    let mut map = match load_js_map(id) {
        Ok(map) => map,
        Err(_) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_remove: load failed");
            return 0;
        }
    };

    match map.remove(&key) {
        Ok(Some(value)) => {
            if let Err(_) = save_js_map(&mut map) {
                write_len(out_len_ptr, 0);
                log_trace!("cs_map_remove: save failed after removal");
                return 0;
            }

            let len = value.len() as u32;
            let ptr = cs_alloc(len);
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), ptr as *mut u8, value.len());
            }
            write_len(out_len_ptr, len);
            log_trace!("cs_map_remove: id={} removed len={len}", map.id());
            ptr
        }
        Ok(None) => {
            if let Err(_) = save_js_map(&mut map) {
                write_len(out_len_ptr, 0);
                log_trace!("cs_map_remove: save failed after miss");
                return 0;
            }
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_remove: id={} missing", map.id());
            0
        }
        Err(err) => {
            write_len(out_len_ptr, 0);
            log_trace!("cs_map_remove: id={} error={err}", map.id());
            let _ = set_error(err);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn cs_map_contains(
    map_id_ptr: u32,
    map_id_len: u32,
    key_ptr: u32,
    key_len: u32,
) -> i32 {
    clear_error();
    log_trace!("cs_map_contains: start key_len={key_len}");

    let id = match read_id(map_id_ptr, map_id_len, "map_contains") {
        Ok(id) => id,
        Err(code) => {
            log_trace!("cs_map_contains: invalid id");
            return code;
        }
    };

    let key = match read_bytes(key_ptr, key_len, "map_contains key") {
        Ok(key) => key,
        Err(code) => {
            log_trace!("cs_map_contains: invalid key pointer/length");
            return code;
        }
    };
    log_trace!("cs_map_contains: id={} key_len={}", id, key.len());

    let map = match load_js_map(id) {
        Ok(map) => map,
        Err(code) => {
            log_trace!("cs_map_contains: load failed");
            return code;
        }
    };

    match map.contains(&key) {
        Ok(true) => {
            log_trace!("cs_map_contains: id={} -> true", map.id());
            1
        }
        Ok(false) => {
            log_trace!("cs_map_contains: id={} -> false", map.id());
            0
        }
        Err(err) => {
            log_trace!("cs_map_contains: id={} error={err}", map.id());
            set_error(err)
        }
    }
}
