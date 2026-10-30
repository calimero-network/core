//! Starting and cancelling compute runs from app code on a cluster, through the `compute`
//! fixture (`apps/v2-fixtures/compute`). No job node is registered, so a run stays queued.

mod common;

use calimero_node::component::{RunRecord, RunState};
use calimero_node::Node;
use common::{signed_up, Cluster};
use serde_json::json;

fn leader_record(c: &Cluster, app: &str, run: &str) -> (RunRecord, Vec<String>) {
    let leader = c.app_leader(app).unwrap();
    let node: &Node = &c.nodes[leader].as_ref().unwrap().node;
    let g = node.hosted(app).unwrap();
    g.read_local(|m, _| (m.run_record(run).unwrap(), m.active_runs()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_starts_a_queued_run_and_an_identical_call_answers_the_same_one() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .upload_bundle(common::component("compute"))
        .await
        .unwrap();
    let app = alice
        .create_app("compute", &bundle, json!({}))
        .await
        .unwrap();
    let up = alice.upload_blob(&app, vec![7u8; 5000]).await.unwrap();
    let file = up["id"].as_str().unwrap().to_owned();

    let first = alice
        .call(&app, "startDigest", json!({ "inputs": [file] }), None)
        .await
        .unwrap();
    let run = first.result.as_str().unwrap().to_owned();
    let again = alice
        .call(&app, "startDigest", json!({ "inputs": [file] }), None)
        .await
        .unwrap();
    assert_eq!(again.result, json!(run), "the same job is the same run");

    let (record, active) = leader_record(&c, &app, &run);
    assert_eq!(record.state, RunState::Queued);
    assert_eq!(record.method, "digest");
    assert_eq!(record.inputs.len(), 1);
    assert_eq!(
        (record.inputs[0].id.as_str(), record.inputs[0].size),
        (file.as_str(), 5000)
    );
    assert_eq!(record.result_method, "onDone");
    assert!(!record.owner.is_empty());
    assert_eq!(active, std::slice::from_ref(&run));

    // A follower has applied the same record.
    let follower = (0..3).find(|i| Some(*i) != c.app_leader(&app)).unwrap();
    let mut seen = None;
    for _ in 0..50 {
        let node: &Node = &c.nodes[follower].as_ref().unwrap().node;
        seen = node
            .hosted(&app)
            .and_then(|g| g.read_local(|m, _| m.run_record(&run)));
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(seen.as_ref(), Some(&record), "the follower's record");

    // Other inputs are another run, and a file that is not the app's is refused.
    let other = alice
        .call(&app, "startDigest", json!({ "inputs": [] }), None)
        .await
        .unwrap();
    assert_ne!(other.result, json!(run));
    let missing = alice
        .call(
            &app,
            "startDigest",
            json!({ "inputs": ["00".repeat(32)] }),
            None,
        )
        .await;
    assert!(missing.is_err(), "{missing:?}");

    // A cancel is recorded on the active run; nothing grants it, so it stays queued.
    let _ok = alice
        .call(&app, "cancel", json!({ "run": run }), None)
        .await
        .unwrap();
    let (record, _) = leader_record(&c, &app, &run);
    assert!(record.cancel_requested);
    assert_eq!(record.state, RunState::Queued);
    let unknown = alice
        .call(&app, "cancel", json!({ "run": "nobody" }), None)
        .await;
    assert!(unknown.is_err(), "{unknown:?}");
}
