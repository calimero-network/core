use serde_json::json;

use vm_runner::{logic, run, store};

fn main() -> color_eyre::Result<()> {
    let file = include_bytes!("../../apps/kv-store/res/kv_store.wasm");

    let mut storage = store::InMemoryStorage::default();

    let limits = logic::VMLimits {
        max_stack_size: 200 << 10, // 200 KiB
        max_memory_pages: 1 << 10, // 1 KiB
        max_registers: 100,
        max_register_size: 100 << 20,    // 100 MiB
        max_registers_capacity: 1 << 30, // 1 GiB
        max_logs: 100,
        max_log_size: 16 << 10,           // 16 KiB
        max_storage_key_size: 1 << 20,    // 1 MiB
        max_storage_value_size: 10 << 20, // 10 MiB
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

    Ok(())
}
