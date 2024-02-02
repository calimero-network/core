use wasmer::{Store, Module, Instance, imports, Global, Value, NativeFunc};
use wasmer::{Memory, MemoryType};

use std::fs::File;
use std::io::{Write, Read};

fn create_memory(store: &Store) -> Memory {
    let memory_type = MemoryType::new(1, None, false); // 1 page minimum, no maximum, not shared
    Memory::new(store, memory_type).unwrap()
}

fn create_counter_global(store: &Store) -> Global {
    Global::new_mut(
        store,
        Value::I32(0),
    )
}

fn load_memory_from_file(instance: &Instance, file_path: &str) -> anyhow::Result<()> {
    let memory = instance.exports.get_memory("memory")?;
    let memory_view = memory.view::<u8>();

    let mut file = File::open(file_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    for (i, byte) in buffer.iter().enumerate() {
        memory_view[i].set(*byte);
    }

    Ok(())
}

fn dump_memory_to_file(instance: &Instance, file_path: &str) -> anyhow::Result<()> {
    let memory = instance.exports.get_memory("memory")?;
    let memory_view = memory.view::<u8>();

    let mut file = File::create(file_path)?;

    let chunk_size = 64 * 1024; 
    let mut buffer = Vec::with_capacity(chunk_size);

    for cell in memory_view.iter() {
        buffer.push(cell.get());
        // Write the buffer to file every time it reaches the chunk size
        if buffer.len() >= chunk_size {
            file.write_all(&buffer)?;
            buffer.clear(); // Clear the buffer to start filling it again
        }
    }

    // Don't forget to write any remaining bytes in the buffer to the file.
    if !buffer.is_empty() {
        file.write_all(&buffer)?;
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let store = Store::default();
    let wasm_bytes = std::fs::read("/Users/ijerkovic/Dev/calimero/cali2.0-experimental/simple-wasmer/module.wasm")?;

    let module = Module::new(&store, wasm_bytes)?;

    let counter_global = create_counter_global(&store);
    let memory = create_memory(&store);

    let import_object = imports! {
        "env" => {
            "__memory_base" => Global::new(&store, Value::I32(0)),
            "memory" => memory,
        },
        "GOT.mem" => {
            "counter" => counter_global,
        },
    };

    let instance = Instance::new(&module, &import_object)?;

    load_memory_from_file(&instance, "memory_state.bin")?;

    let increment: NativeFunc<(), ()> = instance.exports.get_native_function("increment")?;
    let get_counter: NativeFunc<(), i32> = instance.exports.get_native_function("get_counter")?;

    increment.call()?;

    let counter_value = get_counter.call()?;
    println!("Counter value: {}", counter_value);

    dump_memory_to_file(&instance, "memory_state.bin")?;

    Ok(())
}
