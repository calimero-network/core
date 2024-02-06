// use serde::{Serialize, Deserialize};

// #[derive(Serialize, Deserialize)]
// pub struct Data {
//     message: String,
// }

// impl Data {
//     pub fn new() -> Self {
//         Self {
//             message: String::new(),
//         }
//     }

//     pub fn modify(&mut self, new_message: &str) {
//         self.message = new_message.to_owned();
//     }

//     pub fn get(&self) -> &str {
//         &self.message
//     }
// }

// // Expose the methods to C
// #[no_mangle]
// pub extern "C" fn data_new() -> *mut Data {
//     Box::into_raw(Box::new(Data::new()))
// }

// #[no_mangle]
// pub extern "C" fn data_modify(ptr: *mut Data, message_ptr: *const u8, message_len: usize) {
//     let data = unsafe { &mut *ptr };
//     let slice = unsafe { std::slice::from_raw_parts(message_ptr, message_len) };
//     let message = std::str::from_utf8(slice).unwrap();
//     data.modify(message);
// }

// #[no_mangle]
// pub extern "C" fn data_get(ptr: *const Data) -> *const u8 {
//     let data = unsafe { &*ptr };
//     data.get().as_ptr()
// }

use serde::{Serialize, Deserialize};
use serde_json;

#[derive(Serialize, Deserialize)]
pub struct Data {
    message: String,
}

impl Data {
    // Define the `new` method to construct a `Data` instance
    pub fn new() -> Self {
        Self {
            message: String::new(), // Initialize `message` with a new, empty string
        }
    }

    // You might also want to define other methods for `Data` here
    pub fn modify(&mut self, new_message: &str) {
        self.message = new_message.to_owned();
    }

    // Getter method to access `message`
    pub fn get(&self) -> &str {
        &self.message
    }
}

#[no_mangle]
pub extern "C" fn data_new() -> *mut Data {
    Box::into_raw(Box::new(Data::new()))
}

// Function to create a new Data instance from a serialized string
#[no_mangle]
pub extern "C" fn create_data_from_json(ptr: *const u8, len: usize) -> *mut Data {
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    let json_str = std::str::from_utf8(slice).unwrap();
    let data: Data = serde_json::from_str(json_str).unwrap();
    Box::into_raw(Box::new(data))
}

// Function to modify the Data instance based on a serialized string
#[no_mangle]
pub extern "C" fn modify_data_from_json(data_ptr: *mut Data, ptr: *const u8, len: usize) {
    let data = unsafe { &mut *data_ptr };
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    let json_str = std::str::from_utf8(slice).unwrap();
    let new_data: Data = serde_json::from_str(json_str).unwrap();
    data.message = new_data.message;
}

#[no_mangle]
pub extern "C" fn data_get(ptr: *const Data) -> *const u8 {
    let data = unsafe { &*ptr };
    data.message.as_ptr()
}

#[no_mangle]
pub extern "C" fn alloc(size: usize) -> *mut u8 {
    let mut buffer = Vec::with_capacity(size);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer); // Prevent Rust from dropping the buffer, handing over ownership
    ptr
}

#[no_mangle]
pub extern "C" fn dealloc(ptr: *mut u8, cap: usize) {
    unsafe {
        let _ = Vec::from_raw_parts(ptr, 0, cap);
    }
}