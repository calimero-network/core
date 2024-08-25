#![allow(unused_crate_dependencies)]

use calimero_runtime::logic::{VMContext, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{run, Constraint};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;
use serde_json::{json, to_vec as to_json_vec};

fn main() -> EyreResult<()> {
    let file = include_bytes!("../../../apps/kv-store/res/kv_store.wasm");

    let mut storage = InMemoryStorage::default();

    let limits = VMLimits::new(
        /*max_stack_size:*/ 200 << 10, // 200 KiB
        /*max_memory_pages:*/ 1 << 10, // 1 KiB
        /*max_registers:*/ 100,
        /*max_register_size:*/ (100 << 20).validate()?, // 100 MiB
        /*max_registers_capacity:*/ 1 << 30, // 1 GiB
        /*max_logs:*/ 100,
        /*max_log_size:*/ 16 << 10, // 16 KiB
        /*max_events:*/ 100,
        /*max_event_kind_size:*/ 100,
        /*max_event_data_size:*/ 16 << 10, // 16 KiB
        /*max_storage_key_size:*/ (1 << 20).try_into()?, // 1 MiB
        /*max_storage_value_size:*/ (10 << 20).try_into()?, // 10 MiB
    );

    let cx = VMContext::new(
        to_json_vec(&json!({
            "key": "foo"
        }))?,
        [0; 32],
    );
    let get_outcome = run(file, "get", cx, &mut storage, &limits)?;
    dbg!(get_outcome);

    let cx = VMContext::new(
        to_json_vec(&json!({
            "key": "foo",
            "value": "bar"
        }))?,
        [0; 32],
    );
    let set_outcome = run(file, "set", cx, &mut storage, &limits)?;
    dbg!(set_outcome);

    let cx = VMContext::new(
        to_json_vec(&json!({
            "key": "foo"
        }))?,
        [0; 32],
    );
    let get_outcome = run(file, "get", cx, &mut storage, &limits)?;
    dbg!(get_outcome);

    let cx = VMContext::new(
        to_json_vec(&json!({
            "key": "food"
        }))?,
        [0; 32],
    );
    let get_result_outcome = run(file, "get_result", cx, &mut storage, &limits)?;
    dbg!(get_result_outcome);

    let cx = VMContext::new(
        to_json_vec(&json!({
            "key": "food"
        }))?,
        [0; 32],
    );
    let get_unchecked_outcome = run(file, "get_unchecked", cx, &mut storage, &limits)?;
    dbg!(get_unchecked_outcome);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's inspect the storage".bold());
    println!("{}", "--".repeat(20).dimmed());

    dbg!(storage);

    Ok(())
}
