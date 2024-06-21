use calimero_runtime::{logic, run, store, Constraint};
use serde_json::json;

fn main() -> eyre::Result<()> {
    let file = include_bytes!("../../../apps/only-peers/res/only_peers.wasm");

    let mut storage = store::InMemoryStorage::default();

    let limits = logic::VMLimits {
        max_stack_size: 200 << 10, // 200 KiB
        max_memory_pages: 1 << 10, // 1 KiB
        max_registers: 100,
        max_register_size: (100 << 20).validate()?, // 100 MiB
        max_registers_capacity: 1 << 30,            // 1 GiB
        max_logs: 100,
        max_log_size: 16 << 10, // 16 KiB
        max_events: 100,
        max_event_kind_size: 100,
        max_event_data_size: 16 << 10,                  // 16 KiB
        max_storage_key_size: (1 << 20).try_into()?,    // 1 MiB
        max_storage_value_size: (10 << 20).try_into()?, // 10 MiB
    };

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({}))?,
    };
    let get_outcome = run(file, "foo", cx, &mut storage, &limits)?;
    let returns = String::from_utf8(get_outcome.returns.unwrap().unwrap()).unwrap();
    println!("{returns}");

    Ok(())
}
