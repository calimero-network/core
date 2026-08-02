#![allow(dead_code, reason = "The fixture app is described, never called")]
//! Golden manifest: the whole `__calimero_abi()` that `#[app::logic]` generates
//! for one app, pinned as JSON.
//!
//! The app is defined inline so the macros expand inside this test crate, where
//! `cfg(test)` is active and the generated entry point therefore exists. Between
//! them the two fixtures cover the manifest fields an app can move: method
//! intent and xcall flags, nullable params and returns, the `init` shape, the
//! state root and version, and both spellings of a migration edge.

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde_json::{json, to_value};
use calimero_sdk::{app, borsh};
use calimero_storage::collections::LwwRegister;

#[derive(AbiType)]
pub struct Summary {
    total: u64,
    label: Option<String>,
}

#[derive(AbiType)]
pub struct SeedArgs {
    seed: u64,
}

#[app::event]
pub enum Event<'a> {
    Started,
    Named { who: &'a str },
    Count(u64),
}

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct StateV1 {
    total: LwwRegister<u64>,
}

#[app::state(version = 2, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(from = StateV1, method = carry_v1)]
pub struct State {
    total: LwwRegister<u64>,
}

#[app::logic]
impl State {
    #[app::init]
    pub fn init(args: SeedArgs) -> State {
        State {
            total: LwwRegister::new(args.seed),
        }
    }

    #[app::view]
    pub fn label(&self) -> app::Result<Option<String>> {
        Ok(None)
    }

    #[app::xcall(from_same_app)]
    pub fn touch(&mut self, note: String) {
        self.total.set(note.len() as u64);
    }

    pub fn summarize(&self, cap: Option<u64>) -> Summary {
        Summary {
            total: cap.unwrap_or(*self.total),
            label: None,
        }
    }

    // A two-argument `Result`: the ABI carries no error side, so only `u64`
    // reaches the manifest and `GateError` needs no `AbiType`.
    pub fn checked(&self) -> Result<u64, GateError> {
        Ok(*self.total)
    }
}

#[derive(Debug)]
pub enum GateError {
    Empty,
}

// Deliberately not `#[app::logic]`: nothing exports this, so it must not appear
// as a method.
impl State {
    pub fn helper(&self) -> u64 {
        *self.total
    }
}

#[test]
fn generated_manifest_is_the_golden() {
    assert_eq!(
        to_value(__calimero_abi_State::__calimero_abi()).expect("manifest serializes"),
        json!({
            "schema_version": "wasm-abi/1",
            "state_root": "State",
            "state_version": 2,
            "migrations": [{ "method": "carry_v1", "fromVersion": 1 }],
            "types": {
                "Event_Named": {
                    "kind": "record",
                    "fields": [{ "name": "who", "type": { "kind": "string" } }],
                },
                "SeedArgs": {
                    "kind": "record",
                    "fields": [{ "name": "seed", "type": { "kind": "u64" } }],
                },
                "State": {
                    "kind": "record",
                    "fields": [{
                        "name": "total",
                        "type": {
                            "kind": "record",
                            "fields": [],
                            "crdt_type": "lww_register",
                            "inner_type": { "kind": "u64" },
                        },
                    }],
                },
                "Summary": {
                    "kind": "record",
                    "fields": [
                        { "name": "total", "type": { "kind": "u64" } },
                        { "name": "label", "nullable": true, "type": { "kind": "string" } },
                    ],
                },
            },
            "methods": [
                { "name": "checked", "params": [], "returns": { "kind": "u64" } },
                {
                    "name": "init",
                    "params": [{ "name": "args", "type": { "$ref": "SeedArgs" } }],
                    "returns": { "kind": "unit" },
                },
                {
                    "name": "label",
                    "params": [],
                    "returns": { "kind": "string" },
                    "intent": "read_only",
                },
                {
                    "name": "summarize",
                    "params": [{ "name": "cap", "nullable": true, "type": { "kind": "u64" } }],
                    "returns": { "$ref": "Summary" },
                },
                {
                    "name": "touch",
                    "params": [{ "name": "note", "type": { "kind": "string" } }],
                    "returns": { "kind": "unit" },
                    "xcall_callable": true,
                    "xcall_callers": "same_app",
                },
            ],
            "events": [
                { "name": "Count", "payload": { "kind": "u64" } },
                { "name": "Named", "payload": { "$ref": "Event_Named" } },
                { "name": "Started" },
            ],
        })
    );
}

/// An app whose migration is a free `#[app::migrate] fn`: `migration = …` is
/// what tells the state type about the edge.
#[app::state(version = 2, migration = carry_free)]
pub struct FreeFn {
    total: LwwRegister<u64>,
}

#[app::migrate]
pub fn carry_free() -> FreeFn {
    FreeFn {
        total: LwwRegister::new(0),
    }
}

#[app::logic]
impl FreeFn {
    #[app::init]
    pub fn init() -> FreeFn {
        carry_free()
    }
}

#[test]
fn a_free_fn_migration_edge_reaches_the_manifest() {
    assert_eq!(
        to_value(__calimero_abi_FreeFn::__calimero_abi()).expect("manifest serializes"),
        json!({
            "schema_version": "wasm-abi/1",
            "state_root": "FreeFn",
            "state_version": 2,
            "migrations": [{ "method": "carry_free", "fromVersion": 1 }],
            "types": {
                "FreeFn": {
                    "kind": "record",
                    "fields": [{
                        "name": "total",
                        "type": {
                            "kind": "record",
                            "fields": [],
                            "crdt_type": "lww_register",
                            "inner_type": { "kind": "u64" },
                        },
                    }],
                },
            },
            "methods": [{ "name": "init", "params": [], "returns": { "kind": "unit" } }],
            "events": [],
        })
    );
}
