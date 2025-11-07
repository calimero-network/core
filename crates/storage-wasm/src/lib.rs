#![allow(clippy::missing_errors_doc)]

use std::mem;
use std::slice;
use std::sync::Mutex;

use borsh::{to_vec, BorshDeserialize};
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;
use calimero_storage::interface::{Interface, StorageError};
use calimero_storage::js::JsUnorderedMap;
use calimero_storage::store::MainStorage;
use once_cell::sync::Lazy;

static LAST_ERROR: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

fn set_error<E: std::fmt::Display>(err: E) -> i32 {
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

fn read_id(id_ptr: u32, id_len: u32, context: &str) -> Result<Id, i32> {
    let id_bytes = unsafe { slice::from_raw_parts(id_ptr as *const u8, id_len as usize) };

    if id_bytes.len() != 32 {
        return Err(set_error(format!("{context} expects 32-byte ID")));
    }

    let mut id_array = [0_u8; 32];
    id_array.copy_from_slice(id_bytes);
    Ok(Id::new(id_array))
}

fn read_bytes(ptr: u32, len: u32) -> Vec<u8> {
    if len == 0 {
        return Vec::new();
    }
    unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) }.to_vec()
}

fn write_id(id: &Id, out_ptr: u32) {
    if out_ptr == 0 {
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(id.as_bytes().as_ptr(), out_ptr as *mut u8, 32);
    }
}

fn write_len(out_len_ptr: u32, len: u32) {
    if out_len_ptr == 0 {
        return;
    }
    unsafe {
        *(out_len_ptr as *mut u32) = len;
    }
}

fn load_js_map(id: Id) -> Result<JsUnorderedMap, i32> {
    match JsUnorderedMap::load(id) {
        Ok(Some(map)) => Ok(map),
        Ok(None) => Err(set_error("map not found")),
        Err(err) => Err(set_error(err)),
    }
}

fn save_js_map(map: &mut JsUnorderedMap) -> Result<(), i32> {
    map.save().map(|_| ()).map_err(|err| set_error(err))
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

    let id_bytes = unsafe { slice::from_raw_parts(id_ptr as *const u8, id_len as usize) };

    if id_bytes.len() != 32 {
        return set_error("storage_read expects 32-byte ID") as u32;
    }

    let mut id_array = [0_u8; 32];
    id_array.copy_from_slice(id_bytes);
    let id = Id::new(id_array);

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
            ptr
        }
        None => {
            unsafe {
                *(out_len_ptr as *mut u32) = 0;
            }
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

    let id_bytes = unsafe { slice::from_raw_parts(id_ptr as *const u8, id_len as usize) };

    if id_bytes.len() != 32 {
        return set_error("storage_save expects 32-byte ID");
    }

    let mut id_array = [0_u8; 32];
    id_array.copy_from_slice(id_bytes);
    let id = Id::new(id_array);

    let data = unsafe { slice::from_raw_parts(data_ptr as *const u8, data_len as usize) };
    let metadata = Metadata::new(created_at, updated_at);

    match Interface::<MainStorage>::save_raw(id, data.to_vec(), metadata) {
        Ok(_hash) => 1,
        Err(StorageError::CannotCreateOrphan(_)) => set_error("cannot create orphan"),
        Err(err) => set_error(err),
    }
}

#[no_mangle]
pub extern "C" fn cs_metadata_new_serialized(out_len_ptr: u32) -> u32 {
    clear_error();
    let timestamp = calimero_storage::env::time_now();
    let metadata = Metadata::new(timestamp, timestamp);

    match to_vec(&metadata) {
        Ok(bytes) => {
            let len = bytes.len() as u32;
            let ptr = cs_alloc(len);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                *(out_len_ptr as *mut u32) = len;
            }
            ptr
        }
        Err(err) => set_error(err) as u32,
    }
}

#[no_mangle]
pub extern "C" fn cs_storage_metadata(id_ptr: u32, id_len: u32, out_len_ptr: u32) -> u32 {
    clear_error();

    let id_bytes = unsafe { slice::from_raw_parts(id_ptr as *const u8, id_len as usize) };

    if id_bytes.len() != 32 {
        return set_error("storage_metadata expects 32-byte ID") as u32;
    }

    let mut id_array = [0_u8; 32];
    id_array.copy_from_slice(id_bytes);
    let id = Id::new(id_array);

    match Interface::<MainStorage>::generate_comparison_data(Some(id)) {
        Ok(data) => match to_vec(&data.metadata) {
            Ok(bytes) => {
                let len = bytes.len() as u32;
                let ptr = cs_alloc(len);
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                    *(out_len_ptr as *mut u32) = len;
                }
                ptr
            }
            Err(err) => set_error(err) as u32,
        },
        Err(err) => set_error(err) as u32,
    }
}

#[no_mangle]
pub extern "C" fn cs_last_error_length() -> u32 {
    LAST_ERROR
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|msg| msg.len() as u32))
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn cs_last_error(buffer_ptr: u32, buffer_len: u32) -> u32 {
    if let Ok(mut guard) = LAST_ERROR.lock() {
        if let Some(message) = guard.as_ref() {
            let bytes = message.as_bytes();
            let needed = bytes.len() as u32;
            if buffer_ptr == 0 || buffer_len < needed {
                return needed;
            }
            unsafe {
                let dst = slice::from_raw_parts_mut(buffer_ptr as *mut u8, buffer_len as usize);
                dst[..bytes.len()].copy_from_slice(bytes);
            }
            drop(guard.take());
            needed
        } else {
            0
        }
    } else {
        0
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
    let metadata_bytes =
        unsafe { slice::from_raw_parts(metadata_ptr as *const u8, metadata_len as usize) };

    match Metadata::try_from_slice(metadata_bytes) {
        Ok(metadata) => {
            unsafe {
                *(created_at_ptr as *mut u64) = metadata.created_at();
                *(updated_at_ptr as *mut u64) = metadata.updated_at();
            }
            1
        }
        Err(err) => set_error(err),
    }
}

#[no_mangle]
pub extern "C" fn cs_time_now() -> u64 {
    calimero_storage::env::time_now()
}

#[no_mangle]
pub extern "C" fn cs_map_new(out_id_ptr: u32) -> i32 {
    clear_error();

    let mut map = JsUnorderedMap::new();

    if let Err(code) = save_js_map(&mut map) {
        return code;
    }

    write_id(&map.id(), out_id_ptr);
    1
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

    let id = match read_id(map_id_ptr, map_id_len, "map_get") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_len_ptr, 0);
            return 0;
        }
    };

    let key = read_bytes(key_ptr, key_len);

    let map = match load_js_map(id) {
        Ok(map) => map,
        Err(_) => {
            write_len(out_len_ptr, 0);
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
            ptr
        }
        Ok(None) => {
            write_len(out_len_ptr, 0);
            0
        }
        Err(err) => {
            write_len(out_len_ptr, 0);
            set_error(err) as u32
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

    let id = match read_id(map_id_ptr, map_id_len, "map_insert") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_prev_len_ptr, 0);
            return 0;
        }
    };

    let key = read_bytes(key_ptr, key_len);
    let value = read_bytes(value_ptr, value_len);

    let mut map = match load_js_map(id) {
        Ok(map) => map,
        Err(_) => {
            write_len(out_prev_len_ptr, 0);
            return 0;
        }
    };

    match map.insert(&key, &value) {
        Ok(prev) => {
            if let Err(_) = save_js_map(&mut map) {
                write_len(out_prev_len_ptr, 0);
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
                ptr
            } else {
                write_len(out_prev_len_ptr, 0);
                0
            }
        }
        Err(err) => {
            write_len(out_prev_len_ptr, 0);
            set_error(err) as u32
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

    let id = match read_id(map_id_ptr, map_id_len, "map_remove") {
        Ok(id) => id,
        Err(_) => {
            write_len(out_len_ptr, 0);
            return 0;
        }
    };

    let key = read_bytes(key_ptr, key_len);

    let mut map = match load_js_map(id) {
        Ok(map) => map,
        Err(_) => {
            write_len(out_len_ptr, 0);
            return 0;
        }
    };

    match map.remove(&key) {
        Ok(Some(value)) => {
            if let Err(_) = save_js_map(&mut map) {
                write_len(out_len_ptr, 0);
                return 0;
            }

            let len = value.len() as u32;
            let ptr = cs_alloc(len);
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), ptr as *mut u8, value.len());
            }
            write_len(out_len_ptr, len);
            ptr
        }
        Ok(None) => {
            if let Err(_) = save_js_map(&mut map) {
                write_len(out_len_ptr, 0);
                return 0;
            }
            write_len(out_len_ptr, 0);
            0
        }
        Err(err) => {
            write_len(out_len_ptr, 0);
            set_error(err) as u32
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

    let id = match read_id(map_id_ptr, map_id_len, "map_contains") {
        Ok(id) => id,
        Err(code) => return code,
    };

    let key = read_bytes(key_ptr, key_len);

    let map = match load_js_map(id) {
        Ok(map) => map,
        Err(code) => return code,
    };

    match map.contains(&key) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(err) => set_error(err),
    }
}
