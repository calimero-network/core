use wasmer::{Store, Module, Instance, imports, Function, Global, Value, NativeFunc};
use wasmer::{GlobalType, Mutability, Type};
use wasmer::{Memory, MemoryType};

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

fn main() -> anyhow::Result<()> {
    let store = Store::default();
    let wasm_bytes = std::fs::read("/Users/ijerkovic/Dev/calimero/mini-p2p/module.wasm")?;

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

    let increment: NativeFunc<(), ()> = instance.exports.get_native_function("increment")?;
    let get_counter: NativeFunc<(), i32> = instance.exports.get_native_function("get_counter")?;

    increment.call()?;

    let counter_value = get_counter.call()?;
    println!("Counter value: {}", counter_value);

    Ok(())
}
