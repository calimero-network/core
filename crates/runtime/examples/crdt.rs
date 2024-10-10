#![allow(unused_crate_dependencies, reason = "Not actually unused")]

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use calimero_runtime::logic::{VMContext, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{run, Constraint};
use calimero_storage::address::Id;
use calimero_storage::interface::Action;
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;
use serde_json::{json, to_vec as to_json_vec};

fn main() -> EyreResult<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("Usage: {args:?} <path-to-wasm>");
        return Ok(());
    }

    let path = &args[1];
    let path = Path::new(path);
    if !path.exists() {
        eyre::bail!("KV wasm file not found");
    }

    let file = File::open(path)?.bytes().collect::<Result<Vec<u8>, _>>()?;

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

    let cx = VMContext::new(Vec::new(), [0; 32]);

    let get_outcome = run(&file, "init", cx, &mut storage, &limits)?;
    dbg!(get_outcome);

    let action = Action::Add {
        id: Id::new(),
        type_id: 1,
        data: Vec::new(),
        ancestors: Vec::new(),
    };
    let serialized_action = serde_json::to_string(&action)?;
    let input = std::fmt::format(format_args!("{{\"action\": {}}}", serialized_action));
    print!("{}", input);

    let cx = VMContext::new(input.as_bytes().to_owned(), [0; 32]);

    let get_outcome = run(&file, "apply_action", cx, &mut storage, &limits)?;
    dbg!(get_outcome);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's inspect the storage".bold());
    println!("{}", "--".repeat(20).dimmed());

    dbg!(storage);

    Ok(())
}
