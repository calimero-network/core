use wasmer::{imports, Instance, Module, Store, Memory, WasmPtr, Array};
use serde::{Serialize, Deserialize};
use serde_json::to_string;
use std::fs::File;
use std::io::Read;
use std::fs;
use anyhow::Result;

// #[derive(Serialize, Deserialize)]
// struct Data {
//     message: String,
// }

// fn main() -> Result<()> {
//     let mut file = File::open("../wasm_data_struct/target/wasm32-unknown-unknown/release/wasm_data_struct.wasm")?;
//     let mut wasm_bytes = Vec::new();
//     file.read_to_end(&mut wasm_bytes)?;

//     let store = Store::default();
//     let module = Module::new(&store, wasm_bytes)?;
//     let import_object = imports! {};
//     let instance = Instance::new(&module, &import_object)?;
//     let memory = instance.exports.get_memory("memory")?.clone();
//     let alloc = instance.exports.get_function("alloc")?;
//     let dealloc = instance.exports.get_function("dealloc")?;
//     let create_data_from_json = instance.exports.get_function("create_data_from_json")?;
//     let modify_data_from_json = instance.exports.get_function("modify_data_from_json")?;

//     // Serialize Data to JSON string
//     let data = Data { message: "Hello, WASM!".to_string() };
//     let serialized_data = to_string(&data)?;
//     let bytes = serialized_data.as_bytes();
//     let len = bytes.len() as i32;

//     // Allocate memory in WASM for the serialized data
//     let alloc_ptr = alloc.call(&[len.into()])?.get(0).unwrap().i32().unwrap() as u32;


//     // Copy the serialized data into WASM memory
//     let memory_view = memory.view::<u8>();
//     for (i, &byte) in bytes.iter().enumerate() {
//         memory_view[alloc_ptr as usize + i].set(byte);
//     }

//     // Call WASM function to create Data from JSON
//     let data_ptr = create_data_from_json.call(&[alloc_ptr.into(), len.into()])?
//         .get(0)
//         .unwrap() // Unwrap the Option<Value>
//         .i32()    // Attempt to extract an i32
//         .unwrap(); // Unwrap the Result<i32, ValueTypeError>

//     // Modify the Data instance with new serialized data, if needed

//     // Deallocate the memory used for the serialized data
//     dealloc.call(&[alloc_ptr.into(), len.into()])?;

//     Ok(())
// }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let wasm_bytes = fs::read("../wasm_data_struct/target/wasm32-unknown-unknown/release/wasm_data_struct.wasm")?;
    let store = Store::default();
    let module = Module::new(&store, wasm_bytes)?;
    let import_object = imports! {};
    let instance = Instance::new(&module, &import_object)?;

    let memory = instance.exports.get_memory("memory")?.clone();
    let data_new = instance.exports.get_function("data_new")?;
    let data_get = instance.exports.get_function("data_get")?;

    // Create new Data instance
    let data_ptr = data_new.call(&[])?[0].unwrap_i32();

    // Call `data_get` to get the pointer to the message
    let message_ptr = data_get.call(&[data_ptr.into()])?[0].unwrap_i32() as u32;

    // Assuming the string is UTF-8 encoded and null-terminated
    let message = read_string_from_memory(&memory, message_ptr)?;
    println!("Message: {}", message);

    Ok(())
}

fn read_string_from_memory(memory: &Memory, ptr: u32) -> Result<String, Box<dyn std::error::Error>> {
    let memory_view = memory.view::<u8>();

    let mut end = ptr as usize;
    while memory_view[end].get() != 0 {
        end += 1;
    }

    let len = end - ptr as usize;
    let mut bytes = Vec::with_capacity(len);
    for i in ptr as usize..end {
        bytes.push(memory_view[i].get());
    }

    Ok(String::from_utf8(bytes)?)
}