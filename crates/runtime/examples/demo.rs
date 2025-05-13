#![allow(unused_crate_dependencies, reason = "Not actually unused")]

use core::str;
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use calimero_runtime::logic::{VMContext, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{run, Constraint};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;
use serde_json::{json, to_vec as to_json_vec, Value};

fn parse_payload<const PRETTY: bool>(
    payload: impl AsRef<[u8]> + ToOwned<Owned = Vec<u8>>,
) -> EyreResult<String> {
    if let Ok(json) = serde_json::from_slice::<Value>(payload.as_ref()) {
        let func = const {
            if PRETTY {
                serde_json::to_string_pretty
            } else {
                serde_json::to_string
            }
        };

        return func(&json).map_err(Into::into);
    }

    let payload = match String::from_utf8(payload.to_owned()) {
        Ok(string) => return Ok(string),
        Err(err) => err.into_bytes(),
    };

    Ok(format!("{:?}", payload))
}

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

    let limits = VMLimits {
        max_memory_pages: 1 << 10, // 1 KiB
        max_stack_size: 200 << 10, // 200 KiB
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

    let mut execute = |name: &str, payload: Option<Value>| -> EyreResult<()> {
        println!("{}", "--".repeat(20).dimmed());
        println!(
            "method: {}\nparams: {}",
            name.bold(),
            payload
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "{}".to_owned())
                .bold()
        );

        let cx = VMContext::new(
            payload
                .map(|p| to_json_vec(&p))
                .transpose()?
                .unwrap_or_default(),
            [0; 32],
            [0; 32],
        );

        let outcome = run(&file, name, cx, &mut storage, &limits)?;

        // dbg!(&outcome);

        println!("New Root Hash: {:?}", outcome.root_hash);
        println!("Artifact Size: {}", outcome.artifact.len());
        println!("Logs:");

        if outcome.logs.is_empty() {
            println!("  <empty>");
        }

        for log in outcome.logs {
            let payload = parse_payload::<true>(log.into_bytes())?;

            for line in payload.lines() {
                println!("  | {}", line.bold());
            }
        }

        println!("Events:");

        if outcome.events.is_empty() {
            println!("  <empty>");
        }

        for event in outcome.events {
            println!("  kind: {}", event.kind.bold());
            println!("  data: {}", parse_payload::<false>(event.data)?.bold());
        }

        match outcome.returns {
            Ok(returns) => {
                println!("{}:", "Returns".green());

                let payload = returns
                    .map(|p| parse_payload::<true>(p))
                    .transpose()?
                    .unwrap_or_default();

                let mut lines = payload.lines().peekable();

                if lines.peek().is_none() {
                    println!("  <empty>");
                }

                for line in lines {
                    println!("  {}", line.bold());
                }
            }
            Err(err) => {
                println!("{}:", "Error".red());

                let error = match err {
                    calimero_runtime::errors::FunctionCallError::ExecutionError(payload) => {
                        parse_payload::<true>(payload)?
                    }
                    _ => format!("{:#?}", err),
                };

                for line in error.lines() {
                    println!("  {}", line.bold());
                }
            }
        }

        Ok(())
    };

    execute("init", None)?;

    execute("get", Some(json!({ "key": "foo" })))?;

    execute("set", Some(json!({ "key": "foo", "value": "bar" })))?;
    execute("get", Some(json!({ "key": "foo" })))?;

    execute("entries", None)?;

    execute("set", Some(json!({ "key": "foo", "value": "baz" })))?;
    execute("get", Some(json!({ "key": "foo" })))?;

    execute("set", Some(json!({ "key": "name", "value": "Jane" })))?;
    execute("get", Some(json!({ "key": "name" })))?;

    execute("entries", None)?;

    execute("get_result", Some(json!({ "key": "foo" })))?;
    execute("get_result", Some(json!({ "key": "height" })))?;

    execute("get_unchecked", Some(json!({ "key": "name" })))?;
    execute("get_unchecked", Some(json!({ "key": "age" })))?;

    // println!("{}", "--".repeat(20).dimmed());
    // println!("{:>35}", "Now, let's inspect the storage".bold());
    // println!("{}", "--".repeat(20).dimmed());

    // dbg!(storage);

    Ok(())
}
