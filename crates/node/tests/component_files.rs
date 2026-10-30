//! Files of a component app on a cluster, through the client and the `files` fixture
//! (`apps/v2-fixtures/files`): stored on the replicas before the entry, read back, deleted.

mod common;

use std::time::Duration;

use calimero_client::{Client, Consistency};
use calimero_node::component::FileMark;
use calimero_node::files::{unwrap_key, FileOpener};
use calimero_node::node::{CallRequest, PeerBlobs};
use calimero_node::types::Caller;
use common::{signed_up, Cluster};
use serde_json::{json, Value};

const MIB: usize = 1024 * 1024;
/// Not a multiple of the 64 KiB chunk, so the last chunk is a short one.
const LEN: usize = 3 * MIB + MIB / 2 + 1000;

/// The `len` bytes `putFile(len, seed)` writes: splitmix64 words, little endian.
fn stream(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out.truncate(len);
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(calimero_identity::sha256(bytes))
}

struct Fixture {
    c: Cluster,
    /// One signed-in client per node, alice's.
    clients: Vec<Client>,
    app: String,
}

/// Hashing megabytes in the guest costs more fuel than the default call budget.
fn more_fuel(cfg: &mut calimero_node::Config) {
    cfg.budget = Some(calimero_app_host::Budget {
        fuel: u64::MAX,
        ..calimero_app_host::Budget::call()
    });
}

/// A copy fetched from a replica inside one call can outlast the default deadline on a loaded runner.
fn more_time(cfg: &mut calimero_node::Config) {
    more_fuel(cfg);
    if let Some(b) = cfg.budget.as_mut() {
        b.wall_clock = Duration::from_secs(30);
    }
}

async fn start() -> Fixture {
    start_with(Cluster::start_with(3, more_fuel).await).await
}

async fn start_with(c: Cluster) -> Fixture {
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .upload_bundle(common::component("files"))
        .await
        .unwrap();
    let app = alice.create_app("files", &bundle, json!({})).await.unwrap();
    let mut clients = vec![alice];
    for i in 1..c.nodes.len() {
        let client = c.client(i);
        let _s = client.sign_in(&mut pk).await.unwrap();
        clients.push(client);
    }
    Fixture { c, clients, app }
}

impl Fixture {
    /// Writes a file through node 0; answers its id, the plaintext SHA-256 and the
    /// entry's index.
    async fn put(&self, len: usize, seed: u64) -> (String, String, u64) {
        let r = self.clients[0]
            .call(
                &self.app,
                "putFile",
                json!({ "len": len.to_string(), "seed": seed.to_string() }),
                None,
            )
            .await
            .unwrap();
        let id = r.result["id"].as_str().unwrap().to_owned();
        (id, r.result["sha256"].as_str().unwrap().to_owned(), r.index)
    }

    /// `readFile` as a view on node `i` that applied entry `index`.
    async fn read_on(
        &self,
        i: usize,
        id: &str,
        index: u64,
    ) -> Result<Value, Box<calimero_client::Error>> {
        let v = self.clients[i]
            .view(
                &self.app,
                "readFile",
                json!({ "id": id }),
                Consistency::Stale { min_index: index },
            )
            .await
            .map_err(Box::new)?;
        Ok(v.result)
    }

    fn node(&self, i: usize) -> &calimero_node::Node {
        &self.c.nodes[i].as_ref().unwrap().node
    }

    fn mark(&self, i: usize, id: &str) -> Option<FileMark> {
        let g = self.node(i).hosted(&self.app)?;
        g.read_local(|m, _| m.state.component.files.get(id).cloned())
    }

    fn holders(&self, id: &str) -> Vec<usize> {
        (0..3)
            .filter(|i| self.c.nodes[*i].is_some())
            .filter(|i| self.node(*i).engine.get_blob(id).unwrap().is_some())
            .collect()
    }

    fn blob_counts(&self) -> Vec<u64> {
        (0..3)
            .map(|i| self.node(i).engine.blob_count().unwrap())
            .collect()
    }

    fn leader(&self) -> usize {
        self.c.app_leader(&self.app).unwrap()
    }

    fn applied(&self) -> u64 {
        self.c.applied_on(self.leader(), &self.app)
    }

    /// Waits until `holds` is true, or fails after a few seconds.
    async fn until(&self, what: &str, holds: impl Fn() -> bool) {
        for _ in 0..200 {
            if holds() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{what}");
    }

    async fn wait_gone(&self, id: &str) {
        for _ in 0..100 {
            if self.holders(id).is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the bytes of {id} are still on {:?}", self.holders(id));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_file_reads_back_identically_on_every_replica() {
    let f = start().await;
    let (id, sha, index) = f.put(LEN, 7).await;
    assert_eq!(sha, sha256_hex(&stream(LEN, 7)));

    // Before the call answered, a majority held the bytes.
    assert!(f.holders(&id).len() >= 2, "held by {:?}", f.holders(&id));

    let strong = f.clients[0]
        .view(&f.app, "readFile", json!({ "id": id }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(
        strong.result,
        json!({ "size": LEN.to_string(), "sha256": sha })
    );
    for i in 0..3 {
        let stale = f.read_on(i, &id, index).await.unwrap();
        assert_eq!(stale, strong.result, "node {i}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stored_bytes_are_ciphertext_that_only_the_marks_key_opens() {
    let f = start().await;
    let (id, sha, index) = f.put(LEN, 11).await;
    let plain = stream(LEN, 11);
    let window = &plain[LEN / 2..LEN / 2 + 32];
    f.read_on(2, &id, index).await.unwrap();

    for i in f.holders(&id) {
        let stored = f.node(i).engine.get_blob(&id).unwrap().unwrap();
        assert_eq!(
            sha256_hex(&stored),
            id,
            "the id is the hash of the ciphertext"
        );
        assert!(stored.len() > LEN);
        assert!(
            !stored.windows(window.len()).any(|w| w == window),
            "node {i} stores plaintext"
        );

        let mark = f.mark(i, &id).unwrap();
        assert_eq!(mark.size, LEN as u64);
        assert_eq!(hex::encode(&mark.sha256.0), sha);
        let file_key = f.node(i).app_file_key(&f.app);
        let data_key = unwrap_key(&file_key, &f.app, &mark.wrapped_key.0).unwrap();
        let mut opener = FileOpener::new(&data_key);
        let mut opened = opener.push(&stored).unwrap();
        opened.extend(
            opener
                .finish_expecting(mark.size, mark.sha256.0.as_slice().try_into().unwrap())
                .unwrap(),
        );
        assert_eq!(opened, plain, "node {i}");

        assert!(unwrap_key(&[0; 32], &f.app, &mark.wrapped_key.0).is_err());
        assert!(unwrap_key(&file_key, "another-app", &mark.wrapped_key.0).is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_call_stores_no_bytes_and_no_mark() {
    let f = start().await;
    let before = f.blob_counts();
    let e = f.clients[0]
        .call(&f.app, "putAndFail", json!({ "len": "100000" }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert_eq!(f.blob_counts(), before);
    for i in 0..3 {
        let g = f.node(i).hosted(&f.app).unwrap();
        assert!(g.read_local(|m, _| m.state.component.files.is_empty()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_file_removes_its_mark_and_then_its_bytes() {
    let f = start().await;
    let (id, _sha, index) = f.put(200_000, 3).await;
    f.read_on(1, &id, index).await.unwrap();
    let dropped = f.clients[0]
        .call(&f.app, "dropFile", json!({ "id": id }), None)
        .await
        .unwrap();
    for i in 0..3 {
        let _ = f.read_on(i, &id, dropped.index).await.unwrap_err();
        assert!(f.mark(i, &id).is_none(), "node {i} keeps the mark");
    }
    f.wait_gone(&id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_on_a_replica_that_misses_the_bytes_fetches_them() {
    let f = start().await;
    let (id, sha, index) = f.put(500_000, 5).await;
    f.read_on(2, &id, index).await.unwrap();
    f.node(2).engine.delete_blob(&id).unwrap();
    let read = f.read_on(2, &id, index).await.unwrap();
    assert_eq!(read["sha256"], sha);
    assert!(f.node(2).engine.get_blob(&id).unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_no_replica_holds_cannot_be_read() {
    let f = start().await;
    let (id, _sha, index) = f.put(100_000, 5).await;
    f.read_on(0, &id, index).await.unwrap();
    for i in 0..3 {
        f.node(i).engine.delete_blob(&id).unwrap();
    }
    let applied = f.applied();
    let e = f.clients[0]
        .call(&f.app, "touchFile", json!({ "id": id }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TIMEOUT"), "{e}");
    assert_eq!(f.applied(), applied, "the call was logged");
    let e = f.read_on(1, &id, index).await.unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert!(
        e.details().unwrap().to_string().contains("not available"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corrupt_copy_is_dropped_and_healed_from_a_replica() {
    let f = start_with(Cluster::start_with(3, more_time).await).await;
    let (id, sha, index) = f.put(LEN, 9).await;
    let leader = f.leader();
    let follower = (leader + 1) % 3;
    for i in [follower, leader] {
        let mut stored = f.node(i).engine.get_blob(&id).unwrap().unwrap();
        let at = stored.len() / 2;
        stored[at] ^= 1;
        f.node(i).engine.put_blob(&id, &stored).unwrap();
    }
    // A view on the follower and a logged call on the leader each find their copy is not
    // the file, and read it from the replica that still holds it.
    assert_eq!(
        f.read_on(follower, &id, index).await.unwrap()["sha256"],
        sha
    );
    let touched = f.clients[0]
        .call(&f.app, "touchFile", json!({ "id": id }), None)
        .await
        .unwrap();
    assert_eq!(touched.result["sha256"], sha);
    for i in [follower, leader] {
        let healed = f.node(i).engine.get_blob(&id).unwrap().unwrap();
        assert_eq!(sha256_hex(&healed), id, "node {i} keeps a corrupt copy");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_fails_retryably_when_a_majority_cannot_take_the_files() {
    let f = start().await;
    let leader = f.leader();
    let before = f.blob_counts();
    let applied = f.applied();
    for i in (0..3).filter(|i| *i != leader) {
        f.node(i).set_peer_blobs(PeerBlobs::StoreThenFail);
    }
    let args = json!({ "len": "100000", "seed": "1" });
    let e = f.clients[0]
        .call(&f.app, "putFile", args.clone(), Some("k1"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FILES_UNAVAILABLE"), "{e}");
    assert_eq!(f.applied(), applied, "nothing was logged");
    // The bytes the peers took are removed again: no mark names them.
    f.until("bytes of a failed call stay", || f.blob_counts() == before)
        .await;

    // The key was not spent: the same call goes through once the peers take blobs.
    for i in 0..3 {
        f.node(i).set_peer_blobs(PeerBlobs::Accept);
    }
    let r = f.clients[0]
        .call(&f.app, "putFile", args, Some("k1"))
        .await
        .unwrap();
    assert!(r.index > applied);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forwarded_call_that_cannot_store_its_files_is_attempted_once() {
    let f = start().await;
    let leader = f.leader();
    let follower = (leader + 1) % 3;
    for i in (0..3).filter(|i| *i != leader) {
        f.node(i).set_peer_blobs(PeerBlobs::Refuse);
    }
    let account = f.clients[0].me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let req = CallRequest::new(
        f.app.clone(),
        "putFile".to_owned(),
        json!({ "len": "1000", "seed": "1" }),
        Caller::Account { account },
    );
    let pushes = f.node(leader).file_pushes();
    let e = f.node(follower).call_component(&req).await.unwrap_err();
    assert_eq!(e.code, "FILES_UNAVAILABLE", "{e}");
    assert_eq!(f.node(leader).file_pushes(), pushes + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entry_replaced_as_too_large_stores_no_files() {
    let f = start().await;
    let before = f.blob_counts();
    let pushes = f.node(f.leader()).file_pushes();
    let e = f.clients[0]
        .call(
            &f.app,
            "putPadded",
            json!({ "len": "100000", "seed": "2", "pad": 800 * 1024 }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["detail"], "entry too large", "{e}");
    assert_eq!(f.blob_counts(), before);
    assert_eq!(f.node(f.leader()).file_pushes(), pushes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delete_in_an_entry_replaced_as_too_large_keeps_the_file() {
    let f = start().await;
    let (id, sha, index) = f.put(100_000, 6).await;
    let e = f.clients[0]
        .call(
            &f.app,
            "dropPadded",
            json!({ "id": id, "pad": 800 * 1024 }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["detail"], "entry too large", "{e}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(f.holders(&id).len() >= 2, "the bytes were deleted");
    for i in 0..3 {
        assert!(f.mark(i, &id).is_some(), "node {i} lost the mark");
    }
    assert_eq!(
        f.read_on(f.leader(), &id, index).await.unwrap()["sha256"],
        sha
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_file_bytes_are_bounded_across_runs_and_freed_with_the_reader() {
    let f = start().await;
    let (id, sha, index) = f.put(500_000, 8).await;
    let leader = f.leader();
    let applied = f.applied();
    // Room for the stored file only once at a time (500_000 bytes and 8 chunk tags).
    f.node(leader).limit_open_file_bytes(500_128);
    for _ in 0..2 {
        let r = f.clients[0]
            .call(&f.app, "touchFile", json!({ "id": id }), None)
            .await
            .unwrap();
        assert_eq!(r.result["sha256"], sha);
    }
    f.node(leader).limit_open_file_bytes(100_000);
    let e = f.clients[0]
        .call(&f.app, "touchFile", json!({ "id": id }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TIMEOUT"), "{e}");
    assert_eq!(f.applied(), applied + 2, "the refused call was logged");
    let e = f.read_on(leader, &id, index).await.unwrap_err();
    assert!(
        e.details().unwrap().to_string().contains("not available"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_drops_the_bytes_of_a_file_when_it_applies_the_delete() {
    let f = start().await;
    let (id, _sha, index) = f.put(100_000, 12).await;
    let follower = (1..3).find(|i| *i != f.leader()).unwrap();
    f.read_on(follower, &id, index).await.unwrap();
    assert!(f.node(follower).engine.get_blob(&id).unwrap().is_some());
    // The leader's request to delete never takes effect there: only applying the entry can.
    f.node(follower).set_peer_blobs(PeerBlobs::IgnoreDeletes);
    let _r = f.clients[0]
        .call(&f.app, "dropFile", json!({ "id": id }), None)
        .await
        .unwrap();
    f.until("the follower keeps the bytes of a deleted file", || {
        f.node(follower).engine.get_blob(&id).unwrap().is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blob_swapped_for_another_files_ciphertext_is_never_read() {
    let f = start().await;
    let (a, _sha, index) = f.put(100_000, 20).await;
    let (b, _sha, index_b) = f.put(100_000, 21).await;
    f.read_on(2, &a, index).await.unwrap();
    let other = f.node(0).engine.get_blob(&b).unwrap().unwrap();
    for i in 0..3 {
        f.node(i).engine.put_blob(&a, &other).unwrap();
    }
    let e = f.read_on(2, &a, index_b).await.unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert!(
        e.details().unwrap().to_string().contains("not available"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_run_cannot_hold_more_than_its_cap_open() {
    let f = start().await;
    let (id, sha, _index) = f.put(500_000, 8).await;
    let leader = f.leader();
    let applied = f.applied();
    // Room for the file once (500_000 bytes and 8 chunk tags) in one run, not twice.
    f.node(leader).limit_run_open_bytes(600_000);
    let once = f.clients[0]
        .call(&f.app, "touchFile", json!({ "id": id }), None)
        .await
        .unwrap();
    assert_eq!(once.result["sha256"], sha);
    let e = f.clients[0]
        .call(&f.app, "touchTwice", json!({ "id": id }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TIMEOUT"), "{e}");
    assert_eq!(f.applied(), applied + 1, "the refused call was logged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflicting_entry_removes_the_files_stored_for_it() {
    let f = start().await;
    let leader = f.leader();
    let before = f.node(leader).engine.blob_count().unwrap();
    f.node(leader).conflict_next_call();
    // The first run conflicts and its file is dropped; the call runs again.
    let (id, _sha, _index) = f.put(100_000, 40).await;
    f.until("the conflicted run left its bytes behind", || {
        (0..3).all(|i| f.node(i).engine.blob_count().unwrap() <= before + 1)
            && f.node(leader).engine.get_blob(&id).unwrap().is_some()
            && f.node(leader).engine.blob_count().unwrap() == before + 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bytes_being_pushed_are_bounded_across_calls() {
    let f = start().await;
    let leader = f.leader();
    f.node(leader).limit_push_bytes(1_000);
    let account = f.clients[0].me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let req = CallRequest::new(
        f.app.clone(),
        "putFile".to_owned(),
        json!({ "len": "100000", "seed": "1" }),
        Caller::Account { account },
    );
    let e = f.node(leader).call_component(&req).await.unwrap_err();
    assert_eq!(e.code, "FILES_UNAVAILABLE", "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_through_a_node_that_hosts_no_replica_keeps_the_leaders_answer() {
    let c = Cluster::start_with(4, more_fuel).await;
    let f = start_with(c).await;
    let app = f.c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.apps[&f.app].clone());
    let outsider = (1..=4).find(|n| !app.replicas.contains(n)).unwrap() as usize - 1;
    // The outsider reaches the app through the first replica: make that one a follower.
    let first = app.replicas[0] as usize - 1;
    let second = app.replicas[1] as usize - 1;
    for _ in 0..30 {
        if f.c.app_leader(&f.app).is_some_and(|l| l != first) {
            break;
        }
        let g = f.node(second).hosted(&f.app).unwrap();
        let _r = g.raft().trigger().elect().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert_ne!(f.leader(), first, "the first replica still leads");
    let leader = f.leader();
    for i in (0..4).filter(|i| *i != leader && *i != outsider) {
        f.node(i).set_peer_blobs(PeerBlobs::Refuse);
    }
    let account = f.clients[0].me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let caller = Caller::Account { account };
    let keyed = |method: &str| CallRequest {
        idempotency_key: Some("k".to_owned()),
        ..CallRequest::new(
            f.app.clone(),
            method.to_owned(),
            json!({ "len": "1000", "seed": "1" }),
            caller.clone(),
        )
    };
    let pushes = f.node(leader).file_pushes();
    let e = f
        .node(outsider)
        .call_component(&keyed("putFile"))
        .await
        .unwrap_err();
    assert_eq!(e.code, "FILES_UNAVAILABLE", "{e}");
    assert_eq!(
        f.node(leader).file_pushes(),
        pushes + 1,
        "attempted more than once"
    );
    let e = f
        .node(outsider)
        .call_component(&keyed("noSuchMethod"))
        .await
        .unwrap_err();
    assert_eq!(e.code, "APP_ERROR", "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_move_hands_the_files_to_the_joining_node() {
    let c = Cluster::start_with(4, more_fuel).await;
    let f = start_with(c).await;
    let (id, _sha, _index) = f.put(150_000, 30).await;
    let app = f.c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.apps[&f.app].clone());
    let spare = (1..=4).find(|n| !app.replicas.contains(n)).unwrap();
    let leader = f.leader() as u64 + 1;
    let target: Vec<u64> = app
        .replicas
        .iter()
        .copied()
        .filter(|r| *r != leader)
        .chain([spare])
        .collect();
    let ops = f.c.client(leader as usize - 1);
    ops.use_api_key("ops-secret").await;
    let _moved = ops
        .post(
            &format!("ops/apps/{}/replicas", f.app),
            json!({ "replicas": target }),
        )
        .await
        .unwrap();
    let joined = f.c.nodes[spare as usize - 1].as_ref().unwrap();
    assert!(joined.node.engine.get_blob(&id).unwrap().is_some());
    // The bundle at upload, then the bundle and the file handed over once: the second
    // hand-over sends nothing the node already holds.
    assert_eq!(joined.node.blob_puts_seen(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_snapshot_install_carries_the_marks() {
    let mut f = start_with(
        Cluster::start_with(3, |cfg| {
            more_fuel(cfg);
            cfg.raft.snapshot_every = 20;
        })
        .await,
    )
    .await;
    let leader = f.c.app_leader(&f.app).unwrap();
    let lagging = (leader + 1) % 3;
    let lagging_applied = f.c.applied_on(lagging, &f.app);
    f.c.stop(lagging).await;

    let writer = &f.clients[(leader + 2) % 3];
    let r = writer
        .call(
            &f.app,
            "putFile",
            json!({ "len": "70000", "seed": "4" }),
            None,
        )
        .await
        .unwrap();
    let id = r.result["id"].as_str().unwrap().to_owned();
    let index = r.index;
    for _ in 0..300 {
        let _r = writer
            .call(&f.app, "putFile", json!({ "len": "1", "seed": "1" }), None)
            .await
            .unwrap();
    }
    let g = f.node(leader).hosted(&f.app).unwrap();
    g.snapshot_now().await.unwrap();
    drop(g);

    f.c.restart(lagging, |cfg| cfg.raft.snapshot_every = 1_000_000)
        .await;
    let target = f.c.applied_on(leader, &f.app);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let group = f.node(lagging).hosted(&f.app);
        let installed = group.as_ref().is_some_and(|g| {
            g.applied() >= target
                && f.node(lagging)
                    .engine
                    .snapshot_file(g.id())
                    .unwrap()
                    .is_some_and(|(index, _, _)| index > lagging_applied)
        });
        if installed {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no snapshot installed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mark = f
        .mark(lagging, &id)
        .expect("the mark came with the snapshot");
    assert_eq!(mark.size, 70_000);
    assert_eq!(f.mark(leader, &id), Some(mark));
    // The bytes were pushed while it was down: it fetches them.
    let read = f.read_on(lagging, &id, index).await.unwrap();
    assert_eq!(read["size"], "70000");
}
