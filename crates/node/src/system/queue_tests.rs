//! The compute run queue: who is granted a place on a job node, and when.

use calimero_log::Machine;
use serde_json::json;

use super::{Quotas, RunSlot, SystemCmd, SystemMachine, SystemState, RUNS_PER_JOB_NODE};

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
    m
}

fn job_node(m: &mut SystemMachine, id: u64) {
    let node =
        serde_json::from_value(json!({ "id": id, "addr": format!("http://j{id}"), "role": "job" }))
            .unwrap();
    m.run(SystemCmd::RegisterNode { node }).unwrap();
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
    }
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
fn without_a_job_node_a_run_waits_and_a_registered_node_grants_it() {
    let mut m = machine();
    let out = m.run(acquire("a1", "r1", 10)).unwrap();
    assert_eq!(out["state"], json!("queued"));
    assert!(granted(&m).is_empty());
    job_node(&mut m, 5);
    assert_eq!(granted(&m), pairs(&[("a1", "r1")]));
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
    m.run(SystemCmd::RemoveNode { id: 6 }).unwrap();
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
    m.run(SystemCmd::RegisterNode { node }).unwrap();
    m.run(acquire("a1", "r1", 1)).unwrap();
    assert!(granted(&m).is_empty());
}

#[test]
fn the_longest_waiting_org_is_served_first() {
    let mut m = machine();
    job_node(&mut m, 5);
    m.run(acquire("a1", "g1", 1)).unwrap();
    m.run(acquire("a1", "g2", 1)).unwrap();
    // Waiting, in arrival order: y, x, y.
    m.run(acquire("b1", "w1", 1)).unwrap();
    m.run(acquire("a1", "w2", 1)).unwrap();
    m.run(acquire("b1", "w3", 1)).unwrap();
    m.run(release("a1", "g1")).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "g2"), ("b1", "w1")]));
    m.run(release("a1", "g2")).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "w2"), ("b1", "w1")]));
    m.run(release("b1", "w1")).unwrap();
    assert_eq!(granted(&m), pairs(&[("a1", "w2"), ("b1", "w3")]));
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
            refused: false
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
    m.run(SystemCmd::SetQuotas {
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
    })
    .unwrap();
    m.run(SystemCmd::CreateAccount {
        account: serde_json::from_value(
            json!({ "accountId": "bob", "name": "bob", "createdAt": 0, "passkeys": [] }),
        )
        .unwrap(),
    })
    .unwrap();
    m.run(acquire("p", "r1", 1)).unwrap();
    m.run_unproven(SystemCmd::TransferApp {
        app_id: "p".into(),
        to: "bob".into(),
        actor: "alice".into(),
        consent: serde_json::Value::Null,
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
fn many_waiting_runs_are_granted_in_arrival_order_per_payer() {
    let mut m = machine();
    m.state.quotas.max_concurrent_runs_per_org = 3;
    for i in 0..4 {
        m.run(acquire("a1", &format!("x{i}"), 1)).unwrap();
        m.run(acquire("b1", &format!("y{i}"), 1)).unwrap();
    }
    for n in 5..9 {
        job_node(&mut m, n);
    }
    // 8 places, 3 per org: the oldest three of each.
    let got = granted(&m);
    let want: Vec<(String, String)> = (0..3)
        .map(|i| ("a1".to_owned(), format!("x{i}")))
        .chain((0..3).map(|i| ("b1".to_owned(), format!("y{i}"))))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn lowering_the_fuel_quota_marks_the_queued_runs_it_can_never_grant_refused() {
    let mut m = machine();
    m.state.quotas.compute_fuel_per_org = 10;
    job_node(&mut m, 5);
    m.run(acquire("a1", "granted", 8)).unwrap();
    m.run(acquire("a1", "big", 8)).unwrap();
    m.run(acquire("a1", "small", 2)).unwrap();
    m.run(SystemCmd::SetQuotas {
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
