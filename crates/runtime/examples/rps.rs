#![allow(unused_crate_dependencies)]
#![allow(dead_code)]

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use calimero_runtime::logic::{VMContext, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{run, Constraint};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;
use rand::distributions::{Distribution, Standard};
use rand::{random, thread_rng, Rng};
use serde::{Deserialize, Serialize};
use serde_json::{from_slice as from_json_slice, json, to_vec as to_json_vec};

#[derive(Debug, Deserialize)]
struct KeyComponents {
    pk: String,
    sk: String,
}

#[derive(Debug, Deserialize)]
enum State {
    Committed(String),
    Revealed(Choice),
}

#[derive(Debug, Deserialize, Serialize)]
enum Choice {
    Rock,
    Paper,
    Scissors,
}

impl Distribution<Choice> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Choice {
        match rng.gen_range(0..3) {
            0 => Choice::Rock,
            1 => Choice::Paper,
            _ => Choice::Scissors,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GameOver {
    winner: Option<usize>,
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
        eyre::bail!("RPS wasm file not found");
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

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "First, we create a keypair for Joe".bold());
    println!("{}", "--".repeat(20).dimmed());

    let joe_seed: [u8; 32] = thread_rng().gen();

    let cx = VMContext::new(
        to_json_vec(&json!({
            "seed": joe_seed,
        }))?,
        [0; 32],
        [0; 32],
    );
    let create_keypair_outcome = run(&file, "create_keypair", cx, &mut storage, &limits)?;
    dbg!(&create_keypair_outcome);

    let joe_keypair = from_json_slice::<KeyComponents>(
        &create_keypair_outcome
            .returns?
            .expect("Expected a return value"),
    )?;

    dbg!(&joe_keypair);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Next, we create a keypair for Melissa".bold());
    println!("{}", "--".repeat(20).dimmed());

    let melissa_seed: [u8; 32] = thread_rng().gen();

    let cx = VMContext::new(
        to_json_vec(&json!({
            "seed": melissa_seed,
        }))?,
        [0; 32],
        [0; 32],
    );
    let create_keypair_outcome = run(&file, "create_keypair", cx, &mut storage, &limits)?;
    dbg!(&create_keypair_outcome);

    let melissa_keypair = from_json_slice::<KeyComponents>(
        &create_keypair_outcome
            .returns?
            .expect("Expected a return value"),
    )?;

    dbg!(&melissa_keypair);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe joins the game".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(
        to_json_vec(&json!({
            "player_name": "Joe",
            "public_key": joe_keypair.pk,
        }))?,
        [0; 32],
        [0; 32],
    );
    let join_outcome = run(&file, "join", cx, &mut storage, &limits)?;
    dbg!(&join_outcome);

    let joe_idx =
        from_json_slice::<usize>(&join_outcome.returns?.expect("Expected a return value"))?;

    dbg!(&joe_idx);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa joins the game".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(
        to_json_vec(&json!({
            "player_name": "Melissa",
            "public_key": melissa_keypair.pk,
        }))?,
        [0; 32],
        [0; 32],
    );
    let join_outcome = run(&file, "join", cx, &mut storage, &limits)?;
    dbg!(&join_outcome);

    let melissa_idx =
        from_json_slice::<usize>(&join_outcome.returns?.expect("Expected a return value"))?;

    dbg!(&melissa_idx);

    println!("{}", "--".repeat(20).dimmed());
    println!(
        "{:>35}",
        "Now, let's view the active state for the game".bold()
    );
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(vec![], [0; 32], [0; 32]);
    let state_outcome = run(&file, "state", cx, &mut storage, &limits)?;
    dbg!(&state_outcome);

    let game_state = from_json_slice::<[Option<(String, State)>; 2]>(
        &state_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&game_state);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe makes a choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let joe_nonce: [u8; 32] = thread_rng().gen();
    let joe_choice: Choice = random();

    let cx = VMContext::new(
        to_json_vec(&json!({
            "signing_key": joe_keypair.sk,
            "choice": joe_choice,
            "nonce": joe_nonce,
        }))?,
        [0; 32],
        [0; 32],
    );
    let prepare_outcome = run(&file, "prepare", cx, &mut storage, &limits)?;
    dbg!(&prepare_outcome);

    let (joe_commitment, joe_signature) = from_json_slice::<(String, String)>(
        &prepare_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&joe_commitment, &joe_signature);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa makes a choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let melissa_nonce: [u8; 32] = thread_rng().gen();
    let melissa_choice: Choice = random();

    let cx = VMContext::new(
        to_json_vec(&json!({
            "signing_key": melissa_keypair.sk,
            "choice": melissa_choice,
            "nonce": melissa_nonce,
        }))?,
        [0; 32],
        [0; 32],
    );
    let prepare_outcome = run(&file, "prepare", cx, &mut storage, &limits)?;
    dbg!(&prepare_outcome);

    let (melissa_commitment, melissa_signature) = from_json_slice::<(String, String)>(
        &prepare_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&melissa_commitment, &melissa_signature);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe commits to his choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(
        to_json_vec(&json!({
            "player_idx": joe_idx,
            "commitment": joe_commitment,
            "signature": joe_signature,
        }))?,
        [0; 32],
        [0; 32],
    );
    let commit_outcome = run(&file, "commit", cx, &mut storage, &limits)?;
    dbg!(&commit_outcome);

    from_json_slice::<()>(&commit_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa commits to her choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(
        to_json_vec(&json!({
            "player_idx": melissa_idx,
            "commitment": melissa_commitment,
            "signature": melissa_signature,
        }))?,
        [0; 32],
        [0; 32],
    );
    let commit_outcome = run(&file, "commit", cx, &mut storage, &limits)?;
    dbg!(&commit_outcome);

    from_json_slice::<()>(&commit_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe reveals his choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(
        to_json_vec(&json!({
            "player_idx": joe_idx,
            "nonce": joe_nonce,
        }))?,
        [0; 32],
        [0; 32],
    );
    let reveal_outcome = run(&file, "reveal", cx, &mut storage, &limits)?;
    dbg!(&reveal_outcome);

    from_json_slice::<()>(&reveal_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa reveals her choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(
        to_json_vec(&json!({
            "player_idx": melissa_idx,
            "nonce": melissa_nonce,
        }))?,
        [0; 32],
        [0; 32],
    );
    let reveal_outcome = run(&file, "reveal", cx, &mut storage, &limits)?;
    dbg!(&reveal_outcome);

    from_json_slice::<()>(&reveal_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!(
        "{:>35}",
        "Now, let's view the active state for the game".bold()
    );
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(vec![], [0; 32], [0; 32]);
    let state_outcome = run(&file, "state", cx, &mut storage, &limits)?;
    dbg!(&state_outcome);

    let game_state = from_json_slice::<[Option<(String, State)>; 2]>(
        &state_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&game_state);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's inspect the storage".bold());
    println!("{}", "--".repeat(20).dimmed());

    dbg!(&storage);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's reset the state".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = VMContext::new(
        to_json_vec(&json!({
            "player_idx": melissa_idx,
            "commitment": melissa_commitment,
            "signature": melissa_signature,
        }))?,
        [0; 32],
        [0; 32],
    );
    let reset_outcome = run(&file, "reset", cx, &mut storage, &limits)?;
    dbg!(&reset_outcome);

    from_json_slice::<()>(&reset_outcome.returns?.expect("Expected a return value"))?;

    let cx = VMContext::new(vec![], [0; 32], [0; 32]);
    let state_outcome = run(&file, "state", cx, &mut storage, &limits)?;
    dbg!(&state_outcome);

    let game_state = from_json_slice::<[Option<(String, State)>; 2]>(
        &state_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&game_state);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's inspect the storage".bold());
    println!("{}", "--".repeat(20).dimmed());

    dbg!(storage);

    for event in reveal_outcome.events {
        if event.kind == "GameOver" {
            let winner = from_json_slice::<GameOver>(&event.data)?.winner;
            match winner {
                Some(0) => println!("[{joe_choice:?} x {melissa_choice:?}] Joe won!"),
                Some(1) => println!("[{joe_choice:?} x {melissa_choice:?}] Melissa won!"),
                _ => println!("[{joe_choice:?} x {melissa_choice:?}] It was a draw!"),
            }
        }
    }

    Ok(())
}
