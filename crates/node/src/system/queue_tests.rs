//! The compute run queue: who is granted a place on a job node, and when.

use std::collections::BTreeMap;

use calimero_log::Machine;
use serde_json::json;

use super::{
    PoolConfig, Quotas, RunSlot, SystemCmd, SystemMachine, SystemState, RUNS_PER_JOB_NODE,
};

/// Apps `a1`, `a2`, `a3` of org `x`, `b1`, `b2` of org `y` and `p` (personal, owner alice).
fn machine() -> SystemMachine {
    let mut m = SystemMachine::default();
    for (id, org) in [
        ("a1", Some("x")),
        ("a2", Some("x")),
        ("a3", Some("x")),
        ("b1", Some("y")),
        ("b2", Some("y")),
        ("p", None),
    ] {
        let app = serde_json::from_value(json!({
            "appId": id, "name": id, "group": 2, "bundle": "b", "owner": "alice",
            "org": org, "replicas": [1], "createdAt": 0
        }))
        .unwrap();
        let _prev = m.state.apps.insert(id.into(), app);
    }
    for org in ["x", "y"] {
        let _prev = m.state.orgs.insert(
            org.into(),
            serde_json::from_value(json!({
                "orgId": org, "name": org, "members": { "alice": "owner" }, "teams": {}
            }))
            .unwrap(),
        );
    }
    m
}

fn job_node(m: &mut SystemMachine, id: u64) {
    let node =
        serde_json::from_value(json!({ "id": id, "addr": format!("http://j{id}"), "role": "job" }))
            .unwrap();
    m.run_authorized(SystemCmd::RegisterNode {
        node,
        proof: serde_json::Value::Null,
    })
    .unwrap();
}

fn acquire(app: &str, run: &str, fuel: u64) -> SystemCmd {
    let payer = if app.starts_with('a') {
        "x"
    } else if app.starts_with('b') {
        "y"
    } else {
        "account:alice"
    };
    SystemCmd::AcquireRun {
        app: app.into(),
        run: run.into(),
        payer: payer.into(),
        fuel,
        memory_mib: 0,
    }
}

fn acquire_mem(app: &str, run: &str, memory_mib: u32) -> SystemCmd {
    match acquire(app, run, 1) {
        SystemCmd::AcquireRun {
            app,
            run,
            payer,
            fuel,
            ..
        } => SystemCmd::AcquireRun {
            app,
            run,
            payer,
            fuel,
            memory_mib,
        },
        other => other,
    }
}

fn pool_node(m: &mut SystemMachine, id: u64, pool: &str) {
    let node = serde_json::from_value(
        json!({ "id": id, "addr": format!("http://j{id}"), "role": "job", "pool": pool }),
    )
    .unwrap();
    m.run_authorized(SystemCmd::RegisterNode {
        node,
        proof: serde_json::Value::Null,
    })
    .unwrap();
}

fn pools(list: &[(&str, u32, u32)]) -> BTreeMap<String, PoolConfig> {
    list.iter()
        .map(|(name, runs, mem)| {
            (
                (*name).to_owned(),
                PoolConfig {
                    runs_per_node: *runs,
                    max_memory_mib: *mem,
                },
            )
        })
        .collect()
}

fn set_pools(
    m: &mut SystemMachine,
    list: &[(&str, u32, u32)],
) -> Result<serde_json::Value, String> {
    m.run_authorized(SystemCmd::SetPools {
        pools: pools(list),
        proof: serde_json::Value::Null,
    })
    .map_err(str::to_owned)
}

/// How many runs each pool holds granted.
fn granted_by_pool(m: &SystemMachine) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for slot in m.state.compute.values().flat_map(BTreeMap::values) {
        if slot.granted {
            *out.entry(slot.pool.clone()).or_default() += 1;
        }
    }
    out
}

/// The `(code, retryable)` of a refused acquire; the answer must be a refusal.
fn refusal(answer: Result<serde_json::Value, String>) -> (String, bool) {
    let answer = answer.expect("a refusal is an answer, not an error");
    assert_eq!(answer["state"], json!("refused"), "{answer}");
    (
        answer["code"].as_str().unwrap().to_owned(),
        answer["retryable"].as_bool().unwrap(),
    )
}

fn release(app: &str, run: &str) -> SystemCmd {
    SystemCmd::ReleaseRun {
        app: app.into(),
        run: run.into(),
    }
}

fn granted(m: &SystemMachine) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (app, runs) in &m.state.compute {
        for (run, slot) in runs {
            if slot.granted {
                out.push((app.clone(), run.clone()));
            }
        }
    }
    out
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, r)| ((*a).to_owned(), (*r).to_owned()))
        .collect()
}

#[test]
fn defaults_are_four_runs_and_ten_times_the_fuel_ceiling_and_read_from_old_state() {
    let q = Quotas::default();
    assert_eq!(q.max_concurrent_runs_per_org, 4);
    assert_eq!(
        q.compute_fuel_per_org,
        10 * calimero_app_host::MAX_COMPUTE_FUEL
    );
    let old: Quotas = serde_json::from_value(json!({ "maxAccounts": 7 })).unwrap();
    assert_eq!(old.max_concurrent_runs_per_org, 4);
    let shown = serde_json::to_value(&old).unwrap();
    assert_eq!(shown["maxConcurrentRunsPerOrg"], json!(4));
    assert_eq!(shown["computeFuelPerOrg"], json!(10_000_000_000_000u64));
    let old_state: SystemState = serde_json::from_value(json!({
        "nodes": {}, "apps": {}, "nextGroup": 1, "bundles": {}, "accounts": {},
        "credentials": {}, "apiKeys": {}, "refresh": {}, "revokedSessions": [],
        "sessionKeys": [], "orgs": {}, "invites": {}
    }))
    .unwrap();
    assert!(old_state.compute.is_empty() && old_state.next_run_seq == 0);
}

#[test]
fn capacity_is_the_job_nodes_places_and_a_removed_node_takes_none_back() {
    let mut m = machine();
    job_node(&mut m, 5);
    for (app, run) in [("a1", "r1"), ("a1", "r2"), ("b1", "r3"), ("b1", "r4")] {
        m.run(acquire(app, run, 1)).unwrap();
    }
    assert_eq!(granted(&m).len(), RUNS_PER_JOB_NODE);
    job_node(&mut m, 6);
    assert_eq!(granted(&m).len(), 2 * RUNS_PER_JOB_NODE);
    m.run_authorized(SystemCmd::RemoveNode {
        id: 6,
        proof: serde_json::Value::Null,
    })
    .unwrap();
    assert_eq!(
        granted(&m).len(),
        2 * RUNS_PER_JOB_NODE,
        "granted runs stay"
    );
    m.run(acquire("p", "r5", 1)).unwrap();
    m.run(release("a1", "r1")).unwrap();
    m.run(release("a1", "r2")).unwrap();
    assert_eq!(
        granted(&m).len(),
        RUNS_PER_JOB_NODE,
        "no new grant while over the capacity left"
    );
}

#[test]
fn a_replica_node_adds_no_capacity() {
    let mut m = machine();
    let node = serde_json::from_value(json!({ "id": 1, "addr": "http://n1" })).unwrap();
    m.run_authorized(SystemCmd::RegisterNode {
        node,
        proof: serde_json::Value::Null,
    })
    .unwrap();
    m.run(acquire("a1", "r1", 1)).unwrap();
    assert!(granted(&m).is_empty());
}

#[test]
fn free_places_go_to_the_orgs_in_turn_not_to_the_one_with_the_oldest_backlog() {
    let mut m = machine();
    job_node(&mut m, 5);
    for run in ["r1", "r2", "r3", "r4", "r5"] {
        m.run(acquire("a1", run, 1)).unwrap();
    }
    m.run(acquire("b1", "w1", 1)).unwrap();
    m.run(acquire("b1", "w2", 1)).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r1"), ("a1", "r2")]));
    // Org x's backlog is older than every run of org y, yet y is served first.
    m.run(release("a1", "r1")).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r2"), ("b1", "w1")]));
    // Then the org served longest ago: x, then y.
    m.run(release("a1", "r2")).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r3"), ("b1", "w1")]));
    m.run(release("b1", "w1")).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r3"), ("b1", "w2")]));
}

#[test]
fn an_org_over_its_concurrency_cap_is_passed_over_not_waited_for() {
    let mut m = machine();
    m.state.quotas.max_concurrent_runs_per_org = 1;
    job_node(&mut m, 5);
    job_node(&mut m, 6);
    for (app, run) in [("a1", "r1"), ("a2", "r2"), ("b1", "r3"), ("b2", "r4")] {
        m.run(acquire(app, run, 1)).unwrap();
    }
    assert_eq!(granted(&m), pairs(&[("a1", "r1"), ("b1", "r3")]));
    m.run(release("a1", "r1")).unwrap();
    assert_eq!(granted(&m), pairs(&[("a2", "r2"), ("b1", "r3")]));
}

#[test]
fn an_org_over_its_fuel_quota_waits_for_a_release_and_others_pass_it() {
    let mut m = machine();
    m.state.quotas.compute_fuel_per_org = 10;
    job_node(&mut m, 5);
    job_node(&mut m, 6);
    m.run(acquire("a1", "r1", 6)).unwrap();
    m.run(acquire("a1", "r2", 6)).unwrap();
    m.run(acquire("a1", "r3", 4)).unwrap();
    m.run(acquire("b1", "r4", 9)).unwrap();
    // r2 does not fit, and r3 behind it waits too; y is served.
    assert_eq!(granted(&m), pairs(&[("a1", "r1"), ("b1", "r4")]));
    m.run(release("a1", "r1")).unwrap();
    assert_eq!(
        granted(&m),
        pairs(&[("a1", "r2"), ("a1", "r3"), ("b1", "r4")])
    );
}

#[test]
fn a_run_is_refused_with_a_code_for_an_unknown_app_a_foreign_payer_or_too_much_fuel() {
    let mut m = machine();
    assert_eq!(
        refusal(m.run(acquire("nobody", "r", 1))),
        ("NO_SUCH_APP".into(), false)
    );
    let foreign = m.run(SystemCmd::AcquireRun {
        app: "a1".into(),
        run: "r".into(),
        payer: "y".into(),
        fuel: 1,
        memory_mib: 0,
    });
    assert_eq!(refusal(foreign), ("NOT_PAYER".into(), true));
    let heavy = m.run(acquire("a1", "r", calimero_app_host::MAX_COMPUTE_FUEL + 1));
    assert_eq!(refusal(heavy), ("FUEL_OVER_RUN_MAX".into(), false));
    assert!(m.state.compute.is_empty() && m.state.next_run_seq == 0);
}

#[test]
fn an_accepted_acquire_answers_its_state_with_no_code() {
    let mut m = machine();
    let queued = m.run(acquire("a1", "r1", 1)).unwrap();
    assert_eq!(
        queued,
        json!({ "state": "queued", "code": null, "retryable": false })
    );
    job_node(&mut m, 5);
    let granted = m.run(acquire("a1", "r1", 1)).unwrap();
    assert_eq!(
        granted,
        json!({ "state": "granted", "code": null, "retryable": false })
    );
}

#[test]
fn acquiring_twice_answers_the_state_and_changes_nothing() {
    let mut m = machine();
    m.run(acquire("a1", "r1", 5)).unwrap();
    let before = m.state.compute.clone();
    let seq = m.state.next_run_seq;
    let again = m.run(acquire("a1", "r1", 99)).unwrap();
    assert_eq!(again["state"], json!("queued"));
    assert_eq!((&m.state.compute, m.state.next_run_seq), (&before, seq));
    job_node(&mut m, 5);
    let granted_now = m.run(acquire("a1", "r1", 99)).unwrap();
    assert_eq!(granted_now["state"], json!("granted"));
    assert_eq!(
        m.state.run_slot("a1", "r1"),
        Some(&RunSlot {
            payer: "x".into(),
            fuel: 5,
            seq: 0,
            granted: true,
            refused: false,
            pool: String::new(),
        })
    );
}

#[test]
fn releasing_frees_the_place_once_and_an_unknown_run_changes_nothing() {
    let mut m = machine();
    job_node(&mut m, 5);
    m.run(acquire("a1", "r1", 1)).unwrap();
    assert_eq!(m.run(release("a1", "r1")).unwrap()["released"], json!(true));
    assert!(
        m.state.compute.is_empty(),
        "an app without runs has no entry"
    );
    assert_eq!(
        m.run(release("a1", "r1")).unwrap()["released"],
        json!(false)
    );
    assert_eq!(m.run(release("zz", "r")).unwrap()["released"], json!(false));
}

#[test]
fn raising_a_quota_grants_the_runs_it_held_back() {
    let mut m = machine();
    m.state.quotas.max_concurrent_runs_per_org = 1;
    job_node(&mut m, 5);
    m.run(acquire("a1", "r1", 1)).unwrap();
    m.run(acquire("a1", "r2", 1)).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r1")]));
    m.run_authorized(SystemCmd::SetQuotas {
        proof: serde_json::Value::Null,
        quotas: Quotas {
            max_concurrent_runs_per_org: 2,
            ..Quotas::default()
        },
    })
    .unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r1"), ("a1", "r2")]));
}

#[test]
fn deleting_an_app_releases_its_runs_and_grants_the_next() {
    let mut m = machine();
    job_node(&mut m, 5);
    m.run(acquire("a1", "r1", 1)).unwrap();
    m.run(acquire("a1", "r2", 1)).unwrap();
    m.run(acquire("b1", "r3", 1)).unwrap();
    m.run(SystemCmd::DeleteApp {
        app_id: "a1".into(),
        actor: "alice".into(),
        owner_in_org: true,
        owner_in_team: true,
    })
    .unwrap();
    assert_eq!(granted(&m), pairs(&[("b1", "r3")]));
    assert!(!m.state.compute.contains_key("a1"));
}

#[test]
fn deleting_an_org_releases_the_runs_of_all_its_apps() {
    let mut m = machine();
    job_node(&mut m, 5);
    m.run(acquire("a1", "r1", 1)).unwrap();
    m.run(acquire("a2", "r2", 1)).unwrap();
    m.run(acquire("b1", "r3", 1)).unwrap();
    let _prev = m.state.orgs.insert(
        "x".into(),
        serde_json::from_value(json!({
            "orgId": "x", "name": "X", "members": { "alice": "owner" }, "teams": {}
        }))
        .unwrap(),
    );
    m.run_unproven(SystemCmd::DeleteOrg {
        org: "x".into(),
        actor: "alice".into(),
        consent: serde_json::Value::Null,
    })
    .unwrap();
    assert_eq!(m.state.compute.keys().collect::<Vec<_>>(), ["b1"]);
    assert_eq!(granted(&m), pairs(&[("b1", "r3")]));
}

#[test]
fn a_transferred_personal_apps_runs_are_billed_to_the_new_owner() {
    let mut m = machine();
    m.run(SystemCmd::Genesis {
        governance: calimero_client::attest::Governance::default(),
        release: Some("2.0.0".into()),
        operator_key: None,
        nodes: Vec::new(),
        session_proofs: false,
    })
    .unwrap();
    m.run(SystemCmd::CreateAccount {
        at: 0,
        account: serde_json::from_value(
            json!({ "accountId": "bob", "name": "bob", "createdAt": 0, "passkeys": [] }),
        )
        .unwrap(),
        invite: None,
        refresh: None,
        consent: serde_json::Value::Null,
    })
    .unwrap();
    m.run(acquire("p", "r1", 1)).unwrap();
    m.run_unproven(SystemCmd::TransferApp {
        app_id: "p".into(),
        to: "bob".into(),
        actor: "alice".into(),
        consent: serde_json::Value::Null,
        owner_in_org: true,
        owner_in_team: true,
    })
    .unwrap();
    assert_eq!(m.state.run_slot("p", "r1").unwrap().payer, "account:bob");
}

#[test]
fn the_queue_survives_a_snapshot() {
    let mut m = machine();
    job_node(&mut m, 5);
    m.run(acquire("a1", "r1", 1)).unwrap();
    m.run(acquire("b1", "r2", 2)).unwrap();
    let mut restored = SystemMachine::default();
    restored.restore(&m.snapshot()).unwrap();
    assert_eq!(restored.state.compute, m.state.compute);
    assert_eq!(restored.state.next_run_seq, 2);
}

#[test]
fn a_run_released_before_cannot_be_acquired_late() {
    let mut m = machine();
    job_node(&mut m, 5);
    m.run(acquire("a1", "r1", 1)).unwrap();
    m.run(release("a1", "r1")).unwrap();
    assert_eq!(
        refusal(m.run(acquire("a1", "r1", 1))),
        ("RELEASED".into(), false)
    );
    // A release that arrives first is remembered too.
    m.run(release("a1", "early")).unwrap();
    assert_eq!(
        refusal(m.run(acquire("a1", "early", 1))),
        ("RELEASED".into(), false)
    );
    assert!(m.state.compute.is_empty());
}

#[test]
fn the_released_ring_keeps_the_newest_ids_per_app() {
    let mut m = machine();
    for i in 0..super::RELEASED_RUNS_KEPT + 1 {
        m.run(release("a1", &format!("r{i}"))).unwrap();
    }
    assert_eq!(m.state.released_runs["a1"].len(), super::RELEASED_RUNS_KEPT);
    let evicted = m.run(acquire("a1", "r0", 1)).unwrap();
    assert_eq!(
        evicted["state"],
        json!("queued"),
        "the oldest id was evicted"
    );
    assert_eq!(refusal(m.run(acquire("a1", "r1", 1))).0, "RELEASED");
    assert!(!m.state.released_runs.contains_key("a2"));
}

#[test]
fn deleting_an_app_forgets_its_released_ids() {
    let mut m = machine();
    m.run(release("a1", "r1")).unwrap();
    m.run(SystemCmd::DeleteApp {
        app_id: "a1".into(),
        actor: "alice".into(),
        owner_in_org: true,
        owner_in_team: true,
    })
    .unwrap();
    assert!(m.state.released_runs.is_empty());
}

#[test]
fn an_app_queues_at_most_the_active_run_cap() {
    let mut m = machine();
    for i in 0..calimero_app_host::MAX_ACTIVE_RUNS {
        m.run(acquire("a1", &format!("r{i}"), 1)).unwrap();
    }
    assert_eq!(
        refusal(m.run(acquire("a1", "one more", 1))),
        ("APP_QUEUE_FULL".into(), true)
    );
    // Another app of the org has its own.
    m.run(acquire("a2", "r", 1)).unwrap();
    // A repeated acquire of a queued run still answers.
    m.run(acquire("a1", "r0", 1)).unwrap();
    m.run(release("a1", "r0")).unwrap();
    m.run(acquire("a1", "one more", 1)).unwrap();
}

#[test]
fn a_run_whose_fuel_alone_exceeds_the_quota_is_refused_not_queued() {
    let mut m = machine();
    m.state.quotas.compute_fuel_per_org = 10;
    assert_eq!(
        refusal(m.run(acquire("a1", "r", 11))),
        ("FUEL_OVER_QUOTA".into(), false)
    );
    assert!(m.state.compute.is_empty() && m.state.next_run_seq == 0);
    m.run(acquire("a1", "r", 10)).unwrap();
}

#[test]
fn lowering_the_fuel_quota_marks_the_queued_runs_it_can_never_grant_refused() {
    let mut m = machine();
    m.state.quotas.compute_fuel_per_org = 10;
    job_node(&mut m, 5);
    m.run(acquire("a1", "granted", 8)).unwrap();
    m.run(acquire("a1", "big", 8)).unwrap();
    m.run(acquire("a1", "small", 2)).unwrap();
    m.run_authorized(SystemCmd::SetQuotas {
        proof: serde_json::Value::Null,
        quotas: Quotas {
            compute_fuel_per_org: 5,
            ..Quotas::default()
        },
    })
    .unwrap();
    let slot = |run| m.state.run_slot("a1", run).unwrap().clone();
    assert!(slot("big").refused && !slot("big").granted);
    assert!(!slot("granted").refused, "a granted run keeps its place");
    assert!(!slot("small").refused);
    // The answer a repeated acquire gets is the permanent refusal.
    assert_eq!(
        refusal(m.run(acquire("a1", "big", 8))),
        ("FUEL_OVER_QUOTA".into(), false)
    );
    // A refused run does not hold a place or block the payer's next run.
    m.run(release("a1", "granted")).unwrap();
    assert!(m.state.run_slot("a1", "small").unwrap().granted);
    assert!(!m.state.run_slot("a1", "big").unwrap().granted);
}

const TWO_POOLS: [(&str, u32, u32); 2] = [("standard", 2, 4096), ("highmem", 1, 65536)];

#[test]
fn no_pools_is_the_legacy_single_pool() {
    let mut m = machine();
    job_node(&mut m, 5);
    assert_eq!(m.state.pool_for(1_000_000), Some(String::new()));
    assert_eq!(m.state.pool_capacity(""), RUNS_PER_JOB_NODE);
    for (app, run) in [("a1", "r1"), ("b1", "r2"), ("p", "r3")] {
        m.run(acquire_mem(app, run, 999_999)).unwrap();
    }
    assert_eq!(granted(&m).len(), RUNS_PER_JOB_NODE);
}

#[test]
fn a_highmem_run_waits_for_highmem_capacity() {
    let mut m = machine();
    set_pools(&mut m, &TWO_POOLS).unwrap();
    pool_node(&mut m, 5, "standard");
    pool_node(&mut m, 6, "highmem");
    m.run(acquire_mem("a1", "r1", 8192)).unwrap();
    m.run(acquire_mem("b1", "r2", 8192)).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r1")]));
    m.run(acquire_mem("b2", "r3", 1024)).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "r1"), ("b2", "r3")]));
    m.run(release("a1", "r1")).unwrap();
    assert_eq!(granted(&m), pairs(&[("b1", "r2"), ("b2", "r3")]));
}

#[test]
fn capacity_is_counted_per_pool() {
    let mut m = machine();
    m.state.quotas.max_concurrent_runs_per_org = 100;
    set_pools(&mut m, &TWO_POOLS).unwrap();
    pool_node(&mut m, 5, "standard");
    pool_node(&mut m, 6, "standard");
    pool_node(&mut m, 7, "highmem");
    for i in 0..6 {
        m.run(acquire_mem("a1", &format!("s{i}"), 1024)).unwrap();
    }
    for i in 0..2 {
        m.run(acquire_mem("a2", &format!("h{i}"), 8192)).unwrap();
    }
    assert_eq!(granted(&m).len(), 5);
    assert_eq!(
        granted_by_pool(&m),
        BTreeMap::from([("standard".to_owned(), 4), ("highmem".to_owned(), 1)])
    );
}

#[test]
fn memory_no_pool_fits_is_refused() {
    let mut m = machine();
    set_pools(&mut m, &TWO_POOLS).unwrap();
    pool_node(&mut m, 5, "standard");
    assert_eq!(
        refusal(m.run(acquire_mem("a1", "r", 65537))),
        ("RUN_NO_POOL".into(), false)
    );
    assert!(m.state.compute.is_empty());
}

#[test]
fn a_run_queued_before_pools_is_dispatched_to_the_default_pool() {
    let mut m = machine();
    m.run(acquire("a1", "r1", 1)).unwrap();
    set_pools(&mut m, &[("standard", 2, 4096)]).unwrap();
    job_node(&mut m, 5);
    assert_eq!(granted(&m), pairs(&[("a1", "r1")]));
    let pool = m.state.dispatch_pool("a1", "r1").unwrap();
    assert_eq!(pool, "standard");
    assert!(m.state.node_in_pool(&m.state.nodes[&5], &pool));
}

#[test]
fn pools_are_bounded_in_number_and_name() {
    let mut m = machine();
    let many: Vec<(String, u32, u32)> = (0..=super::MAX_POOLS)
        .map(|i| (format!("p{i}"), 1, 1024))
        .collect();
    let many: Vec<(&str, u32, u32)> = many
        .iter()
        .map(|(n, r, mem)| (n.as_str(), *r, *mem))
        .collect();
    assert!(set_pools(&mut m, &many)
        .unwrap_err()
        .starts_with("BAD_POOLS"));
    let long = "x".repeat(super::MAX_POOL_NAME_BYTES + 1);
    assert!(set_pools(&mut m, &[(long.as_str(), 1, 1024)])
        .unwrap_err()
        .starts_with("BAD_POOLS"));
    assert!(set_pools(&mut m, &[("", 1, 1024)])
        .unwrap_err()
        .starts_with("BAD_POOLS"));
    assert!(m.state.pools.is_empty());
    set_pools(&mut m, &many[..super::MAX_POOLS]).unwrap();
}

#[test]
fn a_node_in_an_unknown_pool_adds_no_capacity() {
    let mut m = machine();
    set_pools(&mut m, &[("standard", 2, 4096)]).unwrap();
    pool_node(&mut m, 5, "ghost");
    pool_node(&mut m, 6, "");
    assert_eq!(m.state.pool_capacity("ghost"), 0);
    assert_eq!(m.state.pool_capacity("standard"), 2);
    m.run(acquire_mem("a1", "r1", 1)).unwrap();
    m.run(acquire_mem("a1", "r2", 1)).unwrap();
    m.run(acquire_mem("b1", "r3", 1)).unwrap();
    assert_eq!(granted(&m).len(), 2);
    m.run_authorized(SystemCmd::RemoveNode {
        id: 6,
        proof: serde_json::Value::Null,
    })
    .unwrap();
    assert_eq!(m.state.pool_capacity("standard"), 0);
}

#[test]
fn removing_a_pool_in_use_is_refused() {
    let mut m = machine();
    set_pools(&mut m, &TWO_POOLS).unwrap();
    pool_node(&mut m, 6, "highmem");
    let err = set_pools(&mut m, &[("standard", 2, 4096)]).unwrap_err();
    assert!(err.starts_with("POOL_IN_USE"), "{err}");
    assert_eq!(m.state.pools, pools(&TWO_POOLS));
    // Without the node, a granted run still keeps its pool.
    let mut m = machine();
    set_pools(&mut m, &TWO_POOLS).unwrap();
    pool_node(&mut m, 5, "standard");
    m.run(acquire_mem("a1", "r1", 1024)).unwrap();
    let err = set_pools(&mut m, &[("highmem", 1, 65536)]).unwrap_err();
    assert!(err.starts_with("POOL_IN_USE"), "{err}");
    m.run(release("a1", "r1")).unwrap();
    pool_node(&mut m, 7, "highmem");
    m.run_authorized(SystemCmd::RemoveNode {
        id: 5,
        proof: serde_json::Value::Null,
    })
    .unwrap();
    set_pools(&mut m, &[("highmem", 1, 65536)]).unwrap();
}

#[test]
fn set_pools_twice_is_idempotent() {
    let mut m = machine();
    set_pools(&mut m, &TWO_POOLS).unwrap();
    pool_node(&mut m, 5, "standard");
    m.run(acquire_mem("a1", "r1", 1024)).unwrap();
    let before = granted(&m);
    let first = set_pools(&mut m, &TWO_POOLS).unwrap();
    let second = set_pools(&mut m, &TWO_POOLS).unwrap();
    assert_eq!(first, second);
    assert_eq!(granted(&m), before);
    assert_eq!(m.state.pools, pools(&TWO_POOLS));
}
