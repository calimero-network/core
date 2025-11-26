#![allow(unused_crate_dependencies)]

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;
use eyre::Result as EyreResult;
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
        eyre::bail!("Gen-ext wasm file not found");
    }

    let file = File::open(path)?.bytes().collect::<Result<Vec<u8>, _>>()?;

    let mut storage = InMemoryStorage::default();

    let engine = Engine::default();

    let module = engine.compile(&file)?;

    let input = to_json_vec(&json!({
        "block_height": 167_345_193,
        "account_id": "nearkat.testnet",
    }))?;

    let outcome = module.run(
        [0; 32].into(),
        [0; 32].into(),
        "view_account",
        &input,
        &mut storage,
        None,
        None,
    )?;

    let returns = String::from_utf8(outcome.returns.unwrap().unwrap()).unwrap();

    println!("{returns}");

    Ok(())
}
