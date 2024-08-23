use calimero_runtime::logic::{VMContext, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{logic, run, store, Constraint};
use eyre::Result as EyreResult;
use serde_json::{json, to_vec as to_json_vec};

fn main() -> EyreResult<()> {
    let file = include_bytes!("../../../apps/gen-ext/res/gen_ext.wasm");

    let mut storage = InMemoryStorage::default();

    let limits = VMLimits {
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

    let cx = VMContext {
        input: to_json_vec(&json!({
            "block_height": 167345193,
            "account_id": "nearkat.testnet",
        }))?,
        executor_public_key: [0; 32],
    };
    let get_outcome = run(file, "view_account", cx, &mut storage, &limits)?;
    let returns = String::from_utf8(get_outcome.returns.unwrap().unwrap()).unwrap();
    println!("{returns}");

    Ok(())
}
