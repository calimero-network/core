use calimero_vm_runner::{logic, run, store, Constraint};
use color_eyre::owo_colors::OwoColorize;
use serde_json::json;

fn main() -> color_eyre::Result<()> {
    let file = include_bytes!("../../apps/kv-store/res/kv_store.wasm");

    let mut storage = store::InMemoryStorage::default();

    let limits = logic::VMLimits {
        max_stack_size: 200 << 10, // 200 KiB
        max_memory_pages: 1 << 10, // 1 KiB
        max_registers: 100,
        max_register_size: (100 << 20).validate()?, // 100 MiB
        max_registers_capacity: 1 << 30,            // 1 GiB
        max_logs: 100,
        max_log_size: 16 << 10,                         // 16 KiB
        max_storage_key_size: (1 << 20).try_into()?,    // 1 MiB
        max_storage_value_size: (10 << 20).try_into()?, // 10 MiB
    };

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "key": "foo",
            "value": "bar"
        }))?,
    };
    let set_outcome = run(file, "set", cx, &mut storage, &limits)?;
    dbg!(set_outcome);

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "key": "foo"
        }))?,
    };
    let set_outcome = run(file, "get", cx, &mut storage, &limits)?;
    dbg!(set_outcome);

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "key": "food"
        }))?,
    };
    let set_outcome = run(file, "get_unchecked", cx, &mut storage, &limits)?;
    dbg!(set_outcome);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's inspect the storage".bold());
    println!("{}", "--".repeat(20).dimmed());

    dbg!(storage);

    Ok(())
}
