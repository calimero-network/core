#![allow(dead_code)]

use calimero_runtime::{logic, run, store, Constraint};
use owo_colors::OwoColorize;
use rand::distributions::{Distribution, Standard};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;

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

#[derive(Debug, Serialize, Deserialize)]
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

fn main() -> eyre::Result<()> {
    let file = include_bytes!("../../../apps/rock-paper-scissors/res/rock_paper_scissors.wasm");

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

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "First, we create a keypair for Joe".bold());
    println!("{}", "--".repeat(20).dimmed());

    let joe_seed: [u8; 32] = rand::thread_rng().gen();

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "seed": joe_seed,
        }))?,
        executor_public_key: [0; 32],
    };
    let create_keypair_outcome = run(file, "create_keypair", cx, &mut storage, &limits)?;
    dbg!(&create_keypair_outcome);

    let joe_keypair = serde_json::from_slice::<KeyComponents>(
        &create_keypair_outcome
            .returns?
            .expect("Expected a return value"),
    )?;

    dbg!(&joe_keypair);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Next, we create a keypair for Melissa".bold());
    println!("{}", "--".repeat(20).dimmed());

    let melissa_seed: [u8; 32] = rand::thread_rng().gen();

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "seed": melissa_seed,
        }))?,
        executor_public_key: [0; 32],
    };
    let create_keypair_outcome = run(file, "create_keypair", cx, &mut storage, &limits)?;
    dbg!(&create_keypair_outcome);

    let melissa_keypair = serde_json::from_slice::<KeyComponents>(
        &create_keypair_outcome
            .returns?
            .expect("Expected a return value"),
    )?;

    dbg!(&melissa_keypair);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe joins the game".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "player_name": "Joe",
            "public_key": joe_keypair.pk,
        }))?,
        executor_public_key: [0; 32],
    };
    let join_outcome = run(file, "join", cx, &mut storage, &limits)?;
    dbg!(&join_outcome);

    let joe_idx =
        serde_json::from_slice::<usize>(&join_outcome.returns?.expect("Expected a return value"))?;

    dbg!(&joe_idx);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa joins the game".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "player_name": "Melissa",
            "public_key": melissa_keypair.pk,
        }))?,
        executor_public_key: [0; 32],
    };
    let join_outcome = run(file, "join", cx, &mut storage, &limits)?;
    dbg!(&join_outcome);

    let melissa_idx =
        serde_json::from_slice::<usize>(&join_outcome.returns?.expect("Expected a return value"))?;

    dbg!(&melissa_idx);

    println!("{}", "--".repeat(20).dimmed());
    println!(
        "{:>35}",
        "Now, let's view the active state for the game".bold()
    );
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: vec![],
        executor_public_key: [0; 32],
    };
    let state_outcome = run(file, "state", cx, &mut storage, &limits)?;
    dbg!(&state_outcome);

    let game_state = serde_json::from_slice::<[Option<(String, State)>; 2]>(
        &state_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&game_state);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe makes a choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let joe_nonce: [u8; 32] = rand::thread_rng().gen();
    let joe_choice: Choice = rand::random();

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "signing_key": joe_keypair.sk,
            "choice": joe_choice,
            "nonce": joe_nonce,
        }))?,
        executor_public_key: [0; 32],
    };
    let prepare_outcome = run(file, "prepare", cx, &mut storage, &limits)?;
    dbg!(&prepare_outcome);

    let (joe_commitment, joe_signature) = serde_json::from_slice::<(String, String)>(
        &prepare_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&joe_commitment, &joe_signature);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa makes a choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let melissa_nonce: [u8; 32] = rand::thread_rng().gen();
    let melissa_choice: Choice = rand::random();

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "signing_key": melissa_keypair.sk,
            "choice": melissa_choice,
            "nonce": melissa_nonce,
        }))?,
        executor_public_key: [0; 32],
    };
    let prepare_outcome = run(file, "prepare", cx, &mut storage, &limits)?;
    dbg!(&prepare_outcome);

    let (melissa_commitment, melissa_signature) = serde_json::from_slice::<(String, String)>(
        &prepare_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&melissa_commitment, &melissa_signature);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe commits to his choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "player_idx": joe_idx,
            "commitment": joe_commitment,
            "signature": joe_signature,
        }))?,
        executor_public_key: [0; 32],
    };
    let commit_outcome = run(file, "commit", cx, &mut storage, &limits)?;
    dbg!(&commit_outcome);

    serde_json::from_slice::<()>(&commit_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa commits to her choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "player_idx": melissa_idx,
            "commitment": melissa_commitment,
            "signature": melissa_signature,
        }))?,
        executor_public_key: [0; 32],
    };
    let commit_outcome = run(file, "commit", cx, &mut storage, &limits)?;
    dbg!(&commit_outcome);

    serde_json::from_slice::<()>(&commit_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Joe reveals his choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "player_idx": joe_idx,
            "nonce": joe_nonce,
        }))?,
        executor_public_key: [0; 32],
    };
    let reveal_outcome = run(file, "reveal", cx, &mut storage, &limits)?;
    dbg!(&reveal_outcome);

    serde_json::from_slice::<()>(&reveal_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, Melissa reveals her choice".bold());
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "player_idx": melissa_idx,
            "nonce": melissa_nonce,
        }))?,
        executor_public_key: [0; 32],
    };
    let reveal_outcome = run(file, "reveal", cx, &mut storage, &limits)?;
    dbg!(&reveal_outcome);

    serde_json::from_slice::<()>(&reveal_outcome.returns?.expect("Expected a return value"))?;

    println!("{}", "--".repeat(20).dimmed());
    println!(
        "{:>35}",
        "Now, let's view the active state for the game".bold()
    );
    println!("{}", "--".repeat(20).dimmed());

    let cx = logic::VMContext {
        input: vec![],
        executor_public_key: [0; 32],
    };
    let state_outcome = run(file, "state", cx, &mut storage, &limits)?;
    dbg!(&state_outcome);

    let game_state = serde_json::from_slice::<[Option<(String, State)>; 2]>(
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

    let cx = logic::VMContext {
        input: serde_json::to_vec(&json!({
            "player_idx": melissa_idx,
            "commitment": melissa_commitment,
            "signature": melissa_signature,
        }))?,
        executor_public_key: [0; 32],
    };
    let reset_outcome = run(file, "reset", cx, &mut storage, &limits)?;
    dbg!(&reset_outcome);

    serde_json::from_slice::<()>(&reset_outcome.returns?.expect("Expected a return value"))?;

    let cx = logic::VMContext {
        input: vec![],
        executor_public_key: [0; 32],
    };
    let state_outcome = run(file, "state", cx, &mut storage, &limits)?;
    dbg!(&state_outcome);

    let game_state = serde_json::from_slice::<[Option<(String, State)>; 2]>(
        &state_outcome.returns?.expect("Expected a return value"),
    )?;

    dbg!(&game_state);

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's inspect the storage".bold());
    println!("{}", "--".repeat(20).dimmed());

    dbg!(storage);

    for event in reveal_outcome.events {
        if event.kind == "GameOver" {
            let winner = serde_json::from_slice::<GameOver>(&event.data)?.winner;
            match winner {
                Some(0) => println!("[{:?} x {:?}] Joe won!", joe_choice, melissa_choice),
                Some(1) => println!("[{:?} x {:?}] Melissa won!", joe_choice, melissa_choice),
                _ => println!("[{:?} x {:?}] It was a draw!", joe_choice, melissa_choice),
            }
        }
    }

    Ok(())
}
