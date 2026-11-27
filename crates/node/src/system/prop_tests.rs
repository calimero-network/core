//! Property: a system log applies to the same state on every replica, whether a
//! replica applied it from the start or installed a snapshot part way and applied
//! the rest (a lagging node, a learner, a restart from a checkpoint).
//!
//! Random sequences of system entries (organizations, teams, memberships, nodes, API
//! keys, sessions, the rule marks, replays of earlier entries), each with its proof
//! or without it, go through [`Machine::apply`] as the log hands them over: once on
//! one machine, and once on a second that is snapshotted and restored into a fresh
//! machine at a random point. Each entry must answer the same bytes on both, and the
//! two must end with the same snapshot.

use calimero_identity::testing::SoftAuthenticator;
use calimero_log::{Engine, GroupState, Machine, StateTx};
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::test_runner::{Config, TestError, TestRunner};
use serde_json::{json, Value};

use super::tests::{account_with, all, consent, operator_signer, p, signed};
use super::{api_key_args, Authority, SystemCmd, SystemMachine};
use crate::types::{ApiKeyRecord, NodeRecord, NodeRole, OrgRole, TeamJoin};

/// The cluster's name, as the machines of every replica know it.
const CLUSTER: &str = "test";
/// The accounts the log signs up first; every other entry acts as one of them.
const PEOPLE: [&str; 3] = ["alice", "bob", "carol"];
/// The shared organizations an entry may name (and the actor's personal one).
const ORGS: [&str; 2] = ["o0", "o1"];
const TEAMS: [&str; 2] = ["t0", "t1"];
/// Sequences are this long at most, and run this many times: a few seconds in a
/// debug build, well within the test time budget.
const MAX_OPS: usize = 40;
const CASES: u32 = 256;

/// How an entry is proven: by its actor (or the operator), not at all, or by
/// someone else's passkey over the same statement.
#[derive(Clone, Copy, Debug)]
enum Proof {
    Own,
    Missing,
    Other,
}

/// One entry to log, by indexes into the fixed sets above.
#[derive(Clone, Debug)]
enum Op {
    CreateOrg {
        actor: usize,
        org: usize,
    },
    RenameOrg {
        actor: usize,
        org: usize,
        name: bool,
        proof: Proof,
    },
    SetOrgRole {
        actor: usize,
        org: usize,
        account: usize,
        role: usize,
        proof: Proof,
    },
    RemoveOrgMember {
        actor: usize,
        org: usize,
        account: usize,
        proof: Proof,
    },
    CreateTeam {
        actor: usize,
        org: usize,
        team: usize,
        parent: Option<usize>,
        open: bool,
    },
    SetTeamMember {
        actor: usize,
        org: usize,
        team: usize,
        account: usize,
        lead: bool,
        proof: Proof,
    },
    RegisterNode {
        id: u64,
        proof: Proof,
    },
    RemoveNode {
        id: u64,
        proof: Proof,
    },
    CreateApiKey {
        actor: usize,
        key: usize,
        proof: Proof,
    },
    RevokeApiKey {
        actor: usize,
        key: usize,
        proof: Proof,
    },
    RevokeAllSessions {
        actor: usize,
    },
    RequireCurrentRules,
    ExpireProofs,
    /// An entry logged earlier, logged again (`back` from the end).
    Replay {
        back: usize,
    },
}

fn proof() -> impl Strategy<Value = Proof> {
    prop_oneof![6 => Just(Proof::Own), 1 => Just(Proof::Missing), 1 => Just(Proof::Other)]
}

fn op() -> impl Strategy<Value = Op> {
    let who = || 0..PEOPLE.len();
    // The shared organizations, or (the last index) the actor's personal one.
    let org = || 0..=ORGS.len();
    let team = || 0..TEAMS.len();
    prop_oneof![
        3 => (who(), 0..ORGS.len()).prop_map(|(actor, org)| Op::CreateOrg { actor, org }),
        1 => (who(), org(), any::<bool>(), proof())
            .prop_map(|(actor, org, name, proof)| Op::RenameOrg { actor, org, name, proof }),
        4 => (who(), org(), who(), 0..4_usize, proof()).prop_map(
            |(actor, org, account, role, proof)| Op::SetOrgRole { actor, org, account, role, proof }
        ),
        2 => (who(), org(), who(), proof())
            .prop_map(|(actor, org, account, proof)| Op::RemoveOrgMember { actor, org, account, proof }),
        3 => (who(), org(), team(), proptest::option::of(team()), any::<bool>()).prop_map(
            |(actor, org, team, parent, open)| Op::CreateTeam { actor, org, team, parent, open }
        ),
        3 => (who(), org(), team(), who(), any::<bool>(), proof()).prop_map(
            |(actor, org, team, account, lead, proof)| Op::SetTeamMember {
                actor, org, team, account, lead, proof
            }
        ),
        1 => (5..8_u64, proof()).prop_map(|(id, proof)| Op::RegisterNode { id, proof }),
        1 => (1..8_u64, proof()).prop_map(|(id, proof)| Op::RemoveNode { id, proof }),
        2 => (who(), 0..3_usize, proof())
            .prop_map(|(actor, key, proof)| Op::CreateApiKey { actor, key, proof }),
        1 => (who(), 0..3_usize, proof())
            .prop_map(|(actor, key, proof)| Op::RevokeApiKey { actor, key, proof }),
        1 => who().prop_map(|actor| Op::RevokeAllSessions { actor }),
        1 => Just(Op::RequireCurrentRules),
        1 => Just(Op::ExpireProofs),
        2 => any::<usize>().prop_map(|back| Op::Replay { back }),
    ]
}

/// Turns ops into entry bytes, holding each account's passkey.
struct Log {
    keys: Vec<SoftAuthenticator>,
    entries: Vec<Vec<u8>>,
}

impl Log {
    /// The genesis entry (the operator's key, nodes 1-4) and a sign-up for
    /// each of [`PEOPLE`].
    fn new() -> Self {
        let mut log = Self {
            keys: Vec::new(),
            entries: Vec::new(),
        };
        log.push(&SystemCmd::Genesis {
            session_proofs: true,
            governance: all(Authority::Owner),
            release: Some("2.0.0".into()),
            operator_key: Some(operator_signer().1),
            nodes: (1..=4).map(node).collect(),
        });
        for who in PEOPLE {
            let mut pk = SoftAuthenticator::new("localhost", "http://localhost:3000");
            let cmd = account_with(who, &mut pk);
            log.keys.push(pk);
            log.push(&cmd);
        }
        log
    }

    fn push(&mut self, cmd: &SystemCmd) {
        self.entries
            .push(serde_json::to_vec(cmd).expect("a system command serializes"));
    }

    /// `actor`'s passkey consent over `(subject, method, args)`, or the proof
    /// `proof` asks for instead.
    fn consent(
        &mut self,
        proof: Proof,
        actor: usize,
        subject: &str,
        method: &str,
        args: &Value,
    ) -> Value {
        match proof {
            Proof::Own => consent(&mut self.keys[actor], subject, method, args),
            Proof::Missing => Value::Null,
            Proof::Other => {
                let other = (actor + 1) % self.keys.len();
                consent(&mut self.keys[other], subject, method, args)
            }
        }
    }

    /// The operator's signature over `(cluster, method, args)`, or none, or
    /// one over other arguments.
    fn signed(proof: Proof, method: &str, args: &Value) -> Value {
        match proof {
            Proof::Own => signed("cluster", method, args),
            Proof::Missing => Value::Null,
            Proof::Other => signed("cluster", method, &json!({ "other": args })),
        }
    }

    fn add(&mut self, op: &Op) {
        let at = 1_000 + self.entries.len() as u64;
        let cmd = match *op {
            Op::Replay { back } => {
                let entry =
                    self.entries[self.entries.len() - 1 - back % self.entries.len()].clone();
                self.entries.push(entry);
                return;
            }
            Op::CreateOrg { actor, org } => SystemCmd::CreateOrg {
                org_id: ORGS[org].into(),
                name: format!("Org {org}"),
                actor: PEOPLE[actor].into(),
                release: Some("2.0.0".into()),
                recovery_key: None,
                at,
                name_fits: true,
                personal: false,
                app_team: None,
                capabilities: None,
            },
            Op::RenameOrg {
                actor,
                org,
                name,
                proof,
            } => {
                let org = org_of(org, actor);
                let name = if name { "Docs" } else { "Labs" };
                let consent = self.consent(
                    proof,
                    actor,
                    &format!("org:{org}"),
                    "__rename_org",
                    &json!({ "name": name }),
                );
                SystemCmd::RenameOrg {
                    org,
                    name: name.into(),
                    actor: PEOPLE[actor].into(),
                    consent,
                }
            }
            Op::SetOrgRole {
                actor,
                org,
                account,
                role,
                proof,
            } => {
                let org = org_of(org, actor);
                let role = [
                    OrgRole::Guest,
                    OrgRole::Member,
                    OrgRole::Admin,
                    OrgRole::Owner,
                ][role];
                let args = json!({ "account": PEOPLE[account], "role": role });
                let consent =
                    self.consent(proof, actor, &format!("org:{org}"), "__set_org_role", &args);
                SystemCmd::SetOrgRole {
                    org,
                    account: PEOPLE[account].into(),
                    role,
                    actor: PEOPLE[actor].into(),
                    consent,
                }
            }
            Op::RemoveOrgMember {
                actor,
                org,
                account,
                proof,
            } => {
                let org = org_of(org, actor);
                let args = json!({ "account": PEOPLE[account] });
                let consent = self.consent(
                    proof,
                    actor,
                    &format!("org:{org}"),
                    "__remove_member",
                    &args,
                );
                SystemCmd::RemoveOrgMember {
                    org,
                    account: PEOPLE[account].into(),
                    actor: PEOPLE[actor].into(),
                    consent,
                }
            }
            Op::CreateTeam {
                actor,
                org,
                team,
                parent,
                open,
            } => SystemCmd::CreateTeam {
                org: org_of(org, actor),
                team_id: TEAMS[team].into(),
                name: format!("Team {team}"),
                parent: parent.map(|t| TEAMS[t].into()),
                join: if open {
                    TeamJoin::Everyone
                } else {
                    TeamJoin::Invite
                },
                actor: PEOPLE[actor].into(),
                member_child_teams: true,
            },
            Op::SetTeamMember {
                actor,
                org,
                team,
                account,
                lead,
                proof,
            } => {
                let org = org_of(org, actor);
                let args = json!({
                    "team": TEAMS[team], "account": PEOPLE[account], "lead": lead,
                    "role": null, "capabilities": null,
                });
                let consent = self.consent(
                    proof,
                    actor,
                    &format!("org:{org}"),
                    "__set_team_member",
                    &args,
                );
                SystemCmd::SetTeamMember {
                    org,
                    team: TEAMS[team].into(),
                    account: PEOPLE[account].into(),
                    lead,
                    role: None,
                    capabilities: None,
                    actor: PEOPLE[actor].into(),
                    consent,
                }
            }
            Op::RegisterNode { id, proof } => {
                let node = node(id);
                let args = json!({ "id": id, "addr": node.addr, "zone": node.zone });
                SystemCmd::RegisterNode {
                    node,
                    proof: Self::signed(proof, "__register_node", &args),
                }
            }
            Op::RemoveNode { id, proof } => SystemCmd::RemoveNode {
                id,
                proof: Self::signed(proof, "__remove_node", &json!({ "id": id })),
            },
            Op::CreateApiKey { actor, key, proof } => {
                let hash = format!("h{key}");
                let record = ApiKeyRecord {
                    key_id: format!("k{key}"),
                    account: PEOPLE[actor].into(),
                    label: "ci".into(),
                    apps: None,
                    methods: None,
                    read_only: true,
                    allow_confirm: false,
                    expires_at: u64::MAX / 2,
                    upgrades: None,
                };
                let args = api_key_args(&hash, &record, &[]);
                let subject = format!("account:{}", PEOPLE[actor]);
                let consent = self.consent(proof, actor, &subject, "__create_api_key", &args);
                SystemCmd::CreateApiKey {
                    hash,
                    record,
                    consent,
                    owner_in_org: true,
                    owner_in_team: true,
                    owner_approves: false,
                    then: Vec::new(),
                }
            }
            Op::RevokeApiKey { actor, key, proof } => {
                let key_id = format!("k{key}");
                let subject = format!("account:{}", PEOPLE[actor]);
                let consent = self.consent(
                    proof,
                    actor,
                    &subject,
                    "__revoke_api_key",
                    &json!({ "keyId": key_id }),
                );
                SystemCmd::RevokeApiKey {
                    key_id,
                    actor: PEOPLE[actor].into(),
                    consent,
                }
            }
            Op::RevokeAllSessions { actor } => SystemCmd::RevokeAllSessions {
                actor: PEOPLE[actor].into(),
                now: at,
            },
            Op::RequireCurrentRules => SystemCmd::RequireCurrentRules,
            Op::ExpireProofs => SystemCmd::ExpireProofs,
        };
        self.push(&cmd);
    }
}

/// The organization index `org` names for `actor`: a shared one, or (past
/// them) the actor's personal one.
fn org_of(org: usize, actor: usize) -> String {
    ORGS.get(org)
        .map_or_else(|| p(PEOPLE[actor]), |o| (*o).to_owned())
}

fn node(id: u64) -> NodeRecord {
    NodeRecord {
        id,
        addr: format!("http://n{id}/"),
        zone: String::new(),
        role: NodeRole::Replica,
        pool: String::new(),
    }
}

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

/// Apply `entries` to `m` from log index `from` on, as the log does: each
/// answer, and a failure if any entry stopped the group.
fn apply(
    m: &mut SystemMachine,
    group: &GroupState,
    from: usize,
    entries: &[Vec<u8>],
) -> Vec<Vec<u8>> {
    entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let mut tx = StateTx::new(group);
            let out = m.apply((from + i + 1) as u64, entry, &mut tx);
            assert!(!tx.failed(), "entry {} stopped the group", from + i + 1);
            out
        })
        .collect()
}

/// Whether an answer is `ok` (the entry applied) rather than a refusal.
fn verdict(out: &[u8]) -> bool {
    serde_json::from_slice::<Value>(out).is_ok_and(|v| v.get("ok").is_some())
}

/// Apply `entries` on a machine straight through and on one restored from a
/// snapshot taken after `cut` entries: the same answers, the same end state.
fn replicas_agree(
    group: &GroupState,
    entries: &[Vec<u8>],
    cut: usize,
) -> Result<usize, TestCaseError> {
    let mut straight = SystemMachine::new(CLUSTER.into(), crate::system::shard::ShardId::Root);
    let answers = apply(&mut straight, group, 0, entries);

    let mut before = SystemMachine::new(CLUSTER.into(), crate::system::shard::ShardId::Root);
    let mut restored_answers = apply(&mut before, group, 0, &entries[..cut]);
    let snapshot = before.snapshot().expect("the system state serializes");
    let mut restored = SystemMachine::new(CLUSTER.into(), crate::system::shard::ShardId::Root);
    restored.restore(&snapshot).expect("a snapshot restores");
    prop_assert_eq!(
        restored.snapshot().expect("the system state serializes"),
        snapshot,
        "restoring a snapshot changed it"
    );
    restored_answers.extend(apply(&mut restored, group, cut, &entries[cut..]));

    for (i, (a, b)) in answers.iter().zip(&restored_answers).enumerate() {
        prop_assert_eq!(
            String::from_utf8_lossy(a),
            String::from_utf8_lossy(b),
            "entry {} answered differently after a snapshot at {}",
            i + 1,
            cut
        );
    }
    let a: Value = serde_json::from_slice(&straight.snapshot().expect("serializes")).expect("JSON");
    let b: Value = serde_json::from_slice(&restored.snapshot().expect("serializes")).expect("JSON");
    prop_assert_eq!(a, b, "the states differ after a snapshot at {}", cut);
    Ok(answers.iter().filter(|o| verdict(o)).count())
}

#[test]
fn a_replica_restored_from_a_snapshot_applies_the_log_as_one_that_never_was() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(
        1,
        dir.path(),
        &[3; 32],
        std::sync::Arc::new(Nowhere),
        calimero_log::Config::default(),
    )
    .unwrap();
    let group = engine.group_state(1);
    let mut runner = TestRunner::new(Config {
        cases: CASES,
        // A failure is reported with its shrunk sequence; nothing is written
        // beside the sources.
        failure_persistence: None,
        ..Config::default()
    });
    // The runner takes an `Fn`: the counts are cells.
    let applied = std::cell::Cell::new(0);
    let total = std::cell::Cell::new(0);
    let result = runner.run(
        &(proptest::collection::vec(op(), 1..=MAX_OPS), any::<Index>()),
        |(ops, cut)| {
            let mut log = Log::new();
            for op in &ops {
                log.add(op);
            }
            let cut = cut.index(log.entries.len() + 1);
            applied.set(applied.get() + replicas_agree(&group, &log.entries, cut)?);
            total.set(total.get() + log.entries.len());
            Ok(())
        },
    );
    match result {
        Ok(()) => {
            println!(
                "{CASES} logs, {} of {} entries applied, the rest refused alike",
                applied.get(),
                total.get()
            )
        }
        Err(TestError::Fail(why, (ops, cut))) => {
            panic!("{why}\nminimal log: {ops:#?}\nsnapshot at {cut:?}")
        }
        Err(e) => panic!("{e}"),
    }
}

/// The ops the property draws from produce entries that apply when proven,
/// so the property compares states that change, not only refusals.
#[test]
fn every_kind_of_generated_entry_applies_when_proven() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(
        1,
        dir.path(),
        &[3; 32],
        std::sync::Arc::new(Nowhere),
        calimero_log::Config::default(),
    )
    .unwrap();
    let group = engine.group_state(1);
    let own = Proof::Own;
    let ops = [
        Op::CreateOrg { actor: 0, org: 0 },
        Op::RenameOrg {
            actor: 0,
            org: 0,
            name: true,
            proof: own,
        },
        Op::SetOrgRole {
            actor: 0,
            org: 0,
            account: 1,
            role: 2,
            proof: own,
        },
        Op::SetOrgRole {
            actor: 0,
            org: 0,
            account: 2,
            role: 1,
            proof: own,
        },
        Op::CreateTeam {
            actor: 1,
            org: 0,
            team: 0,
            parent: None,
            open: false,
        },
        Op::SetTeamMember {
            actor: 0,
            org: 0,
            team: 0,
            account: 2,
            lead: false,
            proof: own,
        },
        Op::RemoveOrgMember {
            actor: 0,
            org: 0,
            account: 1,
            proof: own,
        },
        Op::RegisterNode { id: 5, proof: own },
        Op::RemoveNode { id: 5, proof: own },
        Op::CreateApiKey {
            actor: 2,
            key: 0,
            proof: own,
        },
        Op::RevokeApiKey {
            actor: 2,
            key: 0,
            proof: own,
        },
        Op::RevokeAllSessions { actor: 1 },
        Op::RequireCurrentRules,
        Op::ExpireProofs,
    ];
    let mut log = Log::new();
    for op in &ops {
        log.add(op);
    }
    let mut m = SystemMachine::new(CLUSTER.into(), crate::system::shard::ShardId::Root);
    for (i, out) in apply(&mut m, &group, 0, &log.entries).iter().enumerate() {
        assert!(
            verdict(out),
            "entry {}: {}",
            i + 1,
            String::from_utf8_lossy(out)
        );
    }
    // Each one again: refused (a used proof, an existing name), and the same on a restored replica.
    log.add(&Op::Replay { back: 0 });
    let _applied = replicas_agree(&group, &log.entries, 8).unwrap();
}
