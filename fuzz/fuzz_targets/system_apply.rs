#![no_main]
//! System log entries: every node applies each committed entry of the system
//! group (group 1) through `SystemMachine::apply`, whatever bytes the leader
//! logged.
//!
//! The input is entries, one per line, applied in order after a genesis entry and
//! the sign-ups of `alice` and `bob` (each with a passkey, and a personal
//! organization `p-<name>`), so entries that need no proof of their own (an
//! organization, a team, an alias) reach the rules past the proof check.
//! Apply never panics; an entry that does not decode stops the group
//! (`StateTx::fail`) and changes nothing, and one that decodes never stops it; and
//! a second machine, restored from the genesis state's snapshot, gives every
//! entry the same answer and ends with the same snapshot, which restores to
//! itself.

use std::sync::{Arc, OnceLock};

use calimero_identity::testing::SoftAuthenticator;
use calimero_identity::RelyingParty;
use calimero_log::{Engine, GroupState, Machine, StateTx};
use calimero_node::proof::{Consent, Statement};
use calimero_node::system::{
    account_args, Authority, Governance, PersonalOrgSeed, SystemCmd, SystemMachine,
};
use calimero_node::types::{Account, NodeRecord, NodeRole};
use libfuzzer_sys::fuzz_target;
use serde_json::json;

const CLUSTER: &str = "fuzz";
/// The log index of the input's first entry, after the setup's three.
const FIRST: u64 = 4;

/// A log transport for a store no group runs on: nothing is sent.
struct Nowhere;

impl calimero_log::Transport for Nowhere {
    fn send(
        &self,
        _t: calimero_log::NodeId,
        _a: String,
        _g: calimero_log::GroupId,
        _r: calimero_log::RpcRequest,
    ) -> calimero_log::BoxFuture<Result<calimero_log::RpcResponse, String>> {
        Box::pin(async { Err("no peers".into()) })
    }
}

/// The system group's state store, opened once for the whole run.
fn group() -> &'static GroupState {
    static GROUP: OnceLock<(tempfile::TempDir, Engine, GroupState)> = OnceLock::new();
    &GROUP
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let engine = Engine::open(
                1,
                dir.path(),
                &[3; 32],
                Arc::new(Nowhere),
                calimero_log::Config::default(),
            )
            .expect("a fresh store opens");
            let group = engine.group_state(1);
            (dir, engine, group)
        })
        .2
}

/// `id`'s sign-up with a new passkey, as the API logs it.
fn sign_up(id: &str) -> SystemCmd {
    let mut pk = SoftAuthenticator::new("localhost", "http://localhost:3000");
    let rp = RelyingParty {
        id: "localhost".into(),
        origins: vec!["http://localhost:3000".into()],
        require_uv: false,
    };
    let challenge = calimero_identity::random::<32>();
    let credential = calimero_identity::verify_registration(&rp, &challenge, &pk.register(&challenge))
        .expect("a fresh passkey registers");
    let account: Account = serde_json::from_value(json!({
        "accountId": id, "name": id, "createdAt": 0, "passkeys": [credential],
    }))
    .expect("an account record");
    let statement = Statement::new(&format!("account:{id}"), "__create_account", &account_args(&account, None));
    let challenge = statement.challenge().expect("a statement has a challenge");
    let consent = Consent {
        statement,
        assertion: pk.assert(&challenge),
        at: 0,
    };
    SystemCmd::CreateAccount {
        at: 0,
        consent: serde_json::to_value(consent).expect("a consent serializes"),
        account,
        invite: None,
        refresh: None,
        personal_org: Some(PersonalOrgSeed {
            org_id: format!("p-{id}"),
            release: Some("2.0.0".into()),
        }),
    }
}

/// The snapshot of a cluster with its genesis entry and two sign-ups applied:
/// four replicas, every decision the owner's.
fn genesis() -> &'static [u8] {
    static GENESIS: OnceLock<Vec<u8>> = OnceLock::new();
    GENESIS.get_or_init(|| {
        let who = Authority::Owner;
        let cmd = SystemCmd::Genesis {
            session_proofs: true,
            governance: Governance {
                releases: who,
                app_upgrades: who,
                outbound: who,
                ..Governance::default()
            },
            release: Some("2.0.0".into()),
            operator_key: None,
            nodes: (1..=4)
                .map(|id| NodeRecord {
                    id,
                    addr: format!("http://n{id}/"),
                    zone: String::new(),
                    role: NodeRole::Replica,
                    pool: String::new(),
                })
                .collect(),
        };
        let mut m = SystemMachine::new(CLUSTER.into());
        for (index, cmd) in [cmd, sign_up("alice"), sign_up("bob")].iter().enumerate() {
            let entry = serde_json::to_vec(cmd).expect("a command serializes");
            let out = m.apply(index as u64 + 1, &entry, &mut StateTx::new(group()));
            assert!(out.starts_with(b"{\"ok\""), "setup entry {}: {}", index + 1, String::from_utf8_lossy(&out));
        }
        m.snapshot().expect("the state serializes")
    })
}

fn machine() -> SystemMachine {
    let mut m = SystemMachine::new(CLUSTER.into());
    m.restore(genesis()).expect("the genesis snapshot restores");
    m
}

fuzz_target!(|data: &[u8]| {
    let entries: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
    let mut first = machine();
    let mut answers = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let index = i as u64 + FIRST;
        let decodes = serde_json::from_slice::<SystemCmd>(entry).is_ok();
        let before = (!decodes).then(|| first.snapshot().expect("the state serializes"));
        let mut tx = StateTx::new(group());
        answers.push(first.apply(index, entry, &mut tx));
        assert_eq!(tx.failed(), !decodes, "entry {index}: a stop must mean it did not decode");
        if let Some(before) = before {
            assert_eq!(
                first.snapshot().expect("the state serializes"),
                before,
                "entry {index} did not decode yet changed the state"
            );
        }
    }

    let mut second = machine();
    for (i, entry) in entries.iter().enumerate() {
        let index = i as u64 + FIRST;
        let out = second.apply(index, entry, &mut StateTx::new(group()));
        assert_eq!(out, answers[i], "entry {index} answered differently on another replica");
    }
    let end = first.snapshot().expect("the state serializes");
    assert_eq!(second.snapshot().expect("the state serializes"), end, "the replicas' states differ");
    let mut restored = SystemMachine::new(CLUSTER.into());
    restored.restore(&end).expect("a snapshot restores");
    assert_eq!(restored.snapshot().expect("the state serializes"), end, "a snapshot does not restore to itself");
});
