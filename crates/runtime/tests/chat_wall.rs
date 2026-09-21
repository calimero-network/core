//! Where the real chat contract stops being able to append a message.
//!
//! # Why a real contract here, unlike `cost_is_flat`
//!
//! `cost_is_flat` deliberately uses a synthetic guest so that any growth it
//! measures is unambiguously a defect in the storage layer. This test wants the
//! opposite: the number a user actually hits. That number is a property of the
//! app's own write pattern — `send_message` derives its id from
//! `messages.len()`, pushes into an `AuthoredVector`, and writes several nested
//! collections per call — so only the real contract can produce it.
//!
//! # What "the wall" means
//!
//! Gas is charged per executed wasm operator. An O(n) read pattern does not
//! register as "reads are expensive"; it registers as gas spent decoding what
//! those reads returned. So as the collection grows, one `send_message` call
//! eventually exceeds `max_gas` and traps — and every later call traps too,
//! because the cost only grows. The collection is permanently unwritable from
//! that point (core#3602). The wall is the count of the last message that
//! landed.
//!
//! # This is a probe, not the gate
//!
//! It drives a contract in another repo and is `#[ignore]`d, so core CI never
//! runs it and it has rotted silently before. The property it is about, one
//! positional read costing O(n), is gated in-repo by the `vector_get_nth`
//! workload in `tools/storage-cost`, which runs on every PR; this probe adds
//! the end-to-end number in gas, against the real app, on demand.
//!
//! `GasExhausted` is the only outcome that produces a number; anything else
//! panics as contract drift, naming the method, because a wall it did not
//! measure is worse than no wall. A preflight makes drift show up in seconds
//! rather than after a five-minute run that "found" a wall at 0.
//!
//! # Running it
//!
//! Ignored by default: it needs a `curb.wasm` built from mero-chat-pwa, which
//! lives outside this workspace, and it takes a while.
//!
//!   cargo test -p calimero-runtime --test chat_wall -- --ignored --nocapture
//!
//! Override the guest with `CURB_WASM=/path/to/curb.wasm`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use calimero_account::AccountId;
use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

mod wall_harness;

use wall_harness::{Probe, Verdict};

/// The contract this probe drives, and the gate that covers the same property
/// without it.
const PROBE: Probe = Probe {
    app: "mero-chat",
    gate: "\n\
           This probe drives a contract in another repo (mero-chat-pwa) whose call \
           arguments are hand-written here, so they go stale.\n\
           \n\
           The in-repo gate for the same property does not depend on that contract:\n\
           \x20 cargo test -p storage-cost      (vector_get_nth, tools/storage-cost)\n\
           \x20 ./scripts/check-storage-cost.sh\n",
};

/// The real mero-chat contract, as a sibling checkout of this workspace.
///
/// Override with `CURB_LOGIC_DIR=/path/to/mero-chat-pwa/logic`.
const DEFAULT_LOGIC_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../mero-chat-pwa/logic");

/// Every core crate mero-chat pulls from the published git source, which has to
/// be redirected at THIS tree for the measurement to mean anything.
const PATCHED_CRATES: [(&str, &str); 4] = [
    ("calimero-sdk", "crates/sdk"),
    ("calimero-storage", "crates/storage"),
    ("calimero-storage-macros", "crates/storage-macros"),
    ("calimero-wasm-abi", "crates/wasm-abi"),
];

/// The git source mero-chat names for those crates. `[patch]` keys match on the
/// source URL, so this has to be byte-identical to its manifest.
const CORE_GIT_SOURCE: &str = "https://github.com/calimero-network/core.git";

/// Build the real contract against the core in THIS working tree.
///
/// mero-chat pins a published core (`calimero-sdk = { git = ..., tag = ... }`),
/// so a stock build measures the branch's VM against an app compiled for a
/// released storage layer — which is not the question. The patch is injected
/// with `cargo --config` rather than written into mero-chat's manifest: that
/// override is a workstation-only thing and must never be committed there, and
/// a test that requires a human to have edited another repo first is a test
/// that silently measures the wrong thing when they haven't.
///
/// Why not depend on mero-chat instead: it depends on core, so core cannot
/// depend on it without a cycle. Driving its source from outside is the only
/// direction that works.
fn build_curb_wasm() -> Vec<u8> {
    if let Ok(prebuilt) = std::env::var("CURB_WASM") {
        return std::fs::read(&prebuilt)
            .unwrap_or_else(|e| panic!("CURB_WASM={prebuilt} could not be read: {e}"));
    }

    let logic_dir = PathBuf::from(
        std::env::var("CURB_LOGIC_DIR").unwrap_or_else(|_| DEFAULT_LOGIC_DIR.to_owned()),
    );
    assert!(
        logic_dir.join("Cargo.toml").is_file(),
        "no mero-chat logic crate at {}\n\
         clone it beside this workspace, or set CURB_LOGIC_DIR",
        logic_dir.display(),
    );

    // A `[patch]` in mero-chat's own manifest wins over the one injected with
    // `--config`, so the probe would measure whatever tree that patch names and
    // report the number as if it came from here.
    let manifest = std::fs::read_to_string(logic_dir.join("Cargo.toml"))
        .expect("mero-chat logic manifest is readable");
    if manifest.contains(&format!("[patch.\"{CORE_GIT_SOURCE}\"]")) {
        PROBE.drift(&format!(
            "{}/Cargo.toml contains its own [patch.\"{CORE_GIT_SOURCE}\"] section.\n\
             That overrides the redirect this probe injects, so the build would measure \
             the tree that patch names, not this one. Remove it (it is a workstation-only \
             override that should never be committed) and rerun.",
            logic_dir.display(),
        ));
    }

    // Absolute paths: `--config` values are resolved against the invoking
    // directory, not the target manifest's.
    let core_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize core root");

    // Build into OUR target dir, not mero-chat's. The measurement borrows that
    // checkout; it should not leave artefacts in it.
    let target_dir = core_root.join("target/curb-wall");

    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(&logic_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .args([
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--profile",
            "app-release",
        ]);
    for (crate_name, rel) in PATCHED_CRATES {
        let path = core_root.join(rel);
        assert!(path.is_dir(), "missing core crate at {}", path.display());
        cmd.arg("--config").arg(format!(
            r#"patch."{CORE_GIT_SOURCE}".{crate_name}.path="{}""#,
            path.display()
        ));
    }

    // Patching changes dependency resolution, so cargo rewrites the lockfile.
    // Put it back: the lock belongs to mero-chat, and a stray modification
    // there is exactly the kind of thing that gets committed by accident.
    let lock_path = logic_dir.join("Cargo.lock");
    let lock_before = std::fs::read(&lock_path).ok();

    let out = cmd.output().expect("failed to spawn cargo build for curb");

    if let Some(bytes) = lock_before {
        if std::fs::read(&lock_path).ok().as_ref() != Some(&bytes) {
            let _ignored = std::fs::write(&lock_path, &bytes);
        }
    }

    assert!(
        out.status.success(),
        "building the real mero-chat contract against this core failed:\n\
         --- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let wasm_path = target_dir.join("wasm32-unknown-unknown/app-release/curb.wasm");
    std::fs::read(&wasm_path)
        .unwrap_or_else(|e| panic!("built, but no wasm at {}: {e}", wasm_path.display()))
}

/// Stop here even if nothing has walled, so a genuinely-flat build cannot run
/// forever. Well clear of the 1,187 the trie alone reached.
const DEFAULT_CEILING: usize = 20_000;

/// Raise it to chase a wall that has moved out of the default range:
///   CHAT_WALL_CEILING=60000 cargo test ... -- --ignored --nocapture
fn ceiling() -> usize {
    std::env::var("CHAT_WALL_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CEILING)
}

fn call(
    module: &calimero_runtime::Module,
    storage: &mut InMemoryStorage,
    method: &str,
    args: &serde_json::Value,
) -> Outcome {
    module
        .run(
            [0_u8; 32].into(),
            AccountId::from([0_u8; 32]),
            [0_u8; 32].into(),
            method,
            &serde_json::to_vec(args).expect("encode args"),
            storage,
            None,
            None,
        )
        .expect("run must return an Outcome")
}

/// The `init` arguments. Kept next to the other two so the whole cross-repo
/// contract this probe depends on is visible in one screenful.
fn init_args() -> serde_json::Value {
    serde_json::json!({
        "name": "wall",
        "context_type": "Channel",
        "description": "",
        "created_at": 0_u64,
        "creator_username": "alice",
    })
}

/// The `send_message` arguments. This is the part of the cross-repo contract
/// that goes stale; defined once so fixing it is one edit.
fn send_message_args(i: usize) -> serde_json::Value {
    serde_json::json!({
        "message": format!("message {i}"),
        "mentions": [],
        "mentions_usernames": [],
        "parent_message": null,
        "timestamp": i as u64,
        "files": null,
        "images": null,
    })
}

/// The `get_messages` arguments: one page, from the top.
fn get_messages_args() -> serde_json::Value {
    serde_json::json!({
        "parent_message": null,
        "limit": 20,
        "offset": 0,
        "search_term": null,
    })
}

/// How many messages `get_messages` says the store holds. Drifts if the
/// response shape changes, which is a contract change like any other.
fn total_count(outcome: &Outcome) -> usize {
    let body = outcome
        .returns
        .as_ref()
        .unwrap_or_else(|_| PROBE.drift("get_messages failed while reading total_count"))
        .as_ref()
        .unwrap_or_else(|| PROBE.drift("get_messages returned no value at all"));
    let parsed: serde_json::Value = serde_json::from_slice(body)
        .unwrap_or_else(|e| PROBE.drift(&format!("get_messages did not return JSON: {e}")));
    parsed
        .pointer("/output/total_count")
        .or_else(|| parsed.get("total_count"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| {
            PROBE.drift(&format!(
                "no total_count in the get_messages response: {parsed}"
            ))
        }) as usize
}

/// Prove the contract still answers the calls this probe makes, on a THROWAWAY
/// store (so the measured run still starts empty), before spending minutes on
/// a sweep.
fn preflight(module: &calimero_runtime::Module) {
    let mut storage = InMemoryStorage::default();

    PROBE.expect_ok(&call(module, &mut storage, "init", &init_args()), "init");
    PROBE.expect_ok(
        &call(module, &mut storage, "send_message", &send_message_args(0)),
        "send_message",
    );
    let read = call(module, &mut storage, "get_messages", &get_messages_args());
    PROBE.expect_ok(&read, "get_messages");

    let total = total_count(&read);
    if total != 1 {
        PROBE.drift(&format!(
            "after one send_message, get_messages reports total_count={total}, not 1. \
             The write is not landing where the read looks, so every number this probe \
             would produce is an artifact."
        ));
    }
}

#[test]
#[ignore = "cross-repo probe: needs curb.wasm from mero-chat-pwa. The in-repo gate \
            for the same property is `cargo test -p storage-cost` (vector_get_nth)."]
fn how_many_messages_before_send_message_walls() {
    let wasm = build_curb_wasm();

    let limits = VMLimits::default();
    println!("guest:   real mero-chat contract, built against this core tree");
    println!("max_gas: {:?}", limits.max_gas);

    let module = Engine::with_limits(limits)
        .compile(&wasm)
        .expect("compile metered module");

    // Before anything is measured: does the contract still answer these calls?
    preflight(&module);

    // One store for the whole run: the point is that cost depends on what the
    // store already holds, so it must persist across calls.
    let mut storage = InMemoryStorage::default();

    PROBE.expect_ok(&call(&module, &mut storage, "init", &init_args()), "init");

    // Geometric read probes: get_messages is O(n) in the app, so probing it
    // every append would dominate the run. These points are enough to see the
    // curve and to catch the first failure within a factor of two.
    let ceiling = ceiling();
    let probes: Vec<usize> = [
        100, 250, 500, 1_000, 2_000, 2_200, 2_400, 2_600, 2_800, 3_000, 4_000, 8_000, 16_000,
        20_000, 24_000, 28_000, 30_000, 32_000, 36_000, 40_000, 50_000, 60_000,
    ]
    .into_iter()
    .filter(|p| *p <= ceiling)
    .collect();
    let mut next_probe = 0_usize;

    let mut landed = 0_usize;
    let mut write_wall: Option<usize> = None;
    let mut last_read_ok: Option<usize> = None;
    let mut read_wall: Option<usize> = None;

    println!("\n  n        write_gas      w_reads   write_ms | read_gas        r_reads   read_ms");

    for i in 0..ceiling {
        let started = Instant::now();
        let outcome = call(&module, &mut storage, "send_message", &send_message_args(i));
        let write_ms = started.elapsed().as_secs_f64() * 1000.0;

        if let Err(error) = &outcome.returns {
            match PROBE.classify("send_message", error) {
                Verdict::Wall { limit } => {
                    println!("\nWRITE WALL at {landed} (gas limit {limit})");
                    write_wall = Some(landed);
                    break;
                }
                // Preflight passed, so the contract answered this call a moment
                // ago with an empty store. Failing now for a non-gas reason is
                // a state-dependent bug, not a cost measurement, and reporting
                // it as a wall would put a fictional number in a document.
                Verdict::Drift(detail) => PROBE.drift(&format!(
                    "{detail}\n\
                     This appeared only after {landed} messages, so it is state-dependent \
                     rather than a stale signature."
                )),
            }
        }
        landed += 1;

        if next_probe < probes.len() && landed == probes[next_probe] {
            next_probe += 1;

            let started = Instant::now();
            let read = call(&module, &mut storage, "get_messages", &get_messages_args());
            let read_ms = started.elapsed().as_secs_f64() * 1000.0;

            match &read.returns {
                Ok(_) => {
                    // Confirms the store really accumulated: a harness whose
                    // writes silently vanished would show a flat curve and land
                    // every call, which looks exactly like success.
                    let total = total_count(&read);
                    if total != landed {
                        PROBE.drift(&format!(
                            "store holds {total} but {landed} were appended — the cost \
                             curve would be an artifact, not a result"
                        ));
                    }

                    if read_wall.is_none() {
                        last_read_ok = Some(landed);
                    }
                    println!(
                        "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>7.1} | {:>12?}  {:>7}  {read_ms:>7.1}",
                        outcome.gas_used, outcome.storage_reads, read.gas_used, read.storage_reads,
                    );
                }
                Err(error) => match PROBE.classify("get_messages", error) {
                    Verdict::Wall { .. } => {
                        println!(
                            "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>7.1} | READ WALL (gas exhausted)",
                            outcome.gas_used, outcome.storage_reads,
                        );
                        if read_wall.is_none() {
                            read_wall = Some(landed);
                        }
                    }
                    Verdict::Drift(detail) => PROBE.drift(&detail),
                },
            }
        }
    }

    println!("\n--- result ---");
    println!("messages appended:  {landed}");
    match write_wall {
        Some(n) => println!("write wall (send_message):  {n}"),
        None => println!("write wall (send_message):  none below {ceiling}"),
    }
    match (last_read_ok, read_wall) {
        (Some(ok), Some(bad)) => {
            println!("read wall  (get_messages):  last OK at {ok}, first exhausted at {bad}")
        }
        (_, Some(bad)) => println!("read wall  (get_messages):  exhausted by {bad}"),
        (_, None) => println!("read wall  (get_messages):  none below {landed}"),
    }

    // Preflight proved one message lands and every non-gas failure above panics
    // as drift, so nothing appended means the classification is wrong.
    assert!(
        landed > 0,
        "no message was appended even though preflight succeeded - the failure \
         classification in this file is broken"
    );

    // A wall in the first handful of messages is not a wall; it is a symptom
    // that the contract now does something unbounded per call.
    if let Some(n) = write_wall.or(read_wall) {
        assert!(
            n >= 50,
            "walled after only {n} messages. Preflight passed, so the calls are \
             well-formed, but a ceiling this low is a change in the contract's work \
             per call, not the cost curve this probe exists to measure. Investigate \
             before quoting the number."
        );
    }
}
