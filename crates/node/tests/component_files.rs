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
        (0..self.c.nodes.len())
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

/// The file bytes `usage` shows for `payer`.
fn used(usage: &Value, payer: &str) -> u64 {
    usage["fileBytes"][payer].as_u64().unwrap_or(0)
}

impl Fixture {
    async fn account(&self) -> String {
        self.clients[0].me().await.unwrap()["accountId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn upload(
        &self,
        via: usize,
        len: usize,
        seed: u64,
    ) -> Result<Value, Box<calimero_client::Error>> {
        let sent = self.clients[via]
            .upload_blob(&self.app, stream(len, seed))
            .await;
        sent.map_err(Box::new)
    }

    /// Alice's file bytes, as `/auth/usage` shows them.
    async fn charged(&self) -> u64 {
        let payer = format!("account:{}", self.account().await);
        used(&self.clients[0].usage().await.unwrap(), &payer)
    }

    async fn set_file_quota(&self, bytes: u64) {
        let ops = self.c.client(1);
        ops.use_api_key("ops-secret").await;
        let _ok = ops
            .post("ops/quotas", json!({ "maxFileBytesPerOrg": bytes }))
            .await
            .unwrap();
    }

    fn marks(&self) -> usize {
        (0..3)
            .map(|i| {
                let g = self.node(i).hosted(&self.app).unwrap();
                g.read_local(|m, _| m.state.component.files.len())
            })
            .sum()
    }
}

fn status(e: calimero_client::Error) -> (u16, String) {
    match e {
        calimero_client::Error::Api { status, code, .. } => (status, code),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_uploaded_file_is_read_by_the_app_and_stored_sealed() {
    let f = start().await;
    let plain = stream(LEN, 21);
    let up = f.upload(1, LEN, 21).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    assert_eq!(up["size"], json!(LEN));
    assert_eq!(up["sha256"], json!(sha256_hex(&plain)));
    let index = up["index"].as_u64().unwrap();

    for i in 0..3 {
        let seen = f.read_on(i, &id, index).await.unwrap();
        assert_eq!(
            seen,
            json!({ "size": LEN.to_string(), "sha256": sha256_hex(&plain) })
        );
    }
    assert!(f.holders(&id).len() >= 2, "held by {:?}", f.holders(&id));
    let window = &plain[LEN / 2..LEN / 2 + 32];
    for i in f.holders(&id) {
        let stored = f.node(i).engine.get_blob(&id).unwrap().unwrap();
        assert_eq!(sha256_hex(&stored), id);
        assert!(
            !stored.windows(window.len()).any(|w| w == window),
            "node {i}"
        );
    }

    let info = f.clients[2].blob_info(&f.app, &id).await.unwrap();
    assert_eq!(info["size"], json!(LEN));
    assert_eq!(info["sha256"], json!(sha256_hex(&plain)));
    let listed = f.clients[0].list_blobs(&f.app).await.unwrap();
    assert_eq!(listed["blobs"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["blobs"][0]["id"], json!(id));
    assert_eq!(listed["blobs"][0]["sha256"], json!(sha256_hex(&plain)));
    assert_eq!(f.charged().await, LEN as u64);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_is_not_downloadable() {
    let f = start().await;
    let up = f.upload(0, 1000, 1).await.unwrap();
    let e = f.clients[0]
        .download_blob(&f.app, up["id"].as_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(status(e), (409, "NOT_SUPPORTED".to_owned()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_through_a_node_that_hosts_no_replica_is_stored_on_the_replicas() {
    let c = Cluster::start_with(4, more_fuel).await;
    let f = start_with(c).await;
    let app = f.node(0).sys(|s| s.apps[&f.app].clone());
    let outsider = (1..=4).find(|n| !app.replicas.contains(n)).unwrap() as usize - 1;
    assert!(f.node(outsider).hosted(&f.app).is_none());
    // The outsider asks the first replica: make that one a follower, which names the leader.
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
    let outsider_blobs = f.node(outsider).engine.blob_count().unwrap();
    let up = f.upload(outsider, 100_000, 2).await.unwrap();
    let id = up["id"].as_str().unwrap();
    assert!(f.holders(id).len() >= 2, "held by {:?}", f.holders(id));
    assert!(
        !f.holders(id).contains(&outsider),
        "the outsider stores nothing"
    );
    assert_eq!(
        f.node(outsider).engine.blob_count().unwrap(),
        outsider_blobs
    );
    let seen = f
        .read_on(
            app.replicas[0] as usize - 1,
            id,
            up["index"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(seen["size"], "100000");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_only_key_and_a_read_only_member_cannot_upload() {
    let f = start().await;
    let (_id, key) = f.clients[0]
        .create_api_key(json!({ "readOnly": true, "apps": [f.app] }))
        .await
        .unwrap();
    let agent = f.c.client(2);
    agent.use_api_key(&key).await;
    let before = (f.blob_counts(), f.marks(), f.charged().await);
    let e = agent
        .upload_blob(&f.app, stream(5000, 1))
        .await
        .unwrap_err();
    assert_eq!(status(e), (403, "FORBIDDEN".to_owned()));

    let alice = &f.clients[0];
    let (bob, _pk) = signed_up(&f.c, 1, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = alice.post("orgs", json!({ "name": "Docs" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .post(&format!("orgs/{org}/invites"), json!({ "role": "member" }))
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
    let team = alice
        .post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "T", "join": "everyone" }),
        )
        .await
        .unwrap()["teamId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{team}/members"),
            json!({ "account": bob_id, "role": "readOnly" }),
        )
        .await
        .unwrap();
    let bundle = alice
        .upload_bundle(common::component("files"))
        .await
        .unwrap();
    let shared = alice
        .create_app(
            "shared",
            &bundle,
            json!({ "org": org, "access": { "kind": "team", "team": team } }),
        )
        .await
        .unwrap();
    let e = bob.upload_blob(&shared, stream(5000, 1)).await.unwrap_err();
    assert_eq!(status(e), (403, "FORBIDDEN".to_owned()));
    assert_eq!(bob.usage().await.unwrap()["fileBytes"][&org], json!(0));

    assert_eq!((f.blob_counts(), f.marks(), f.charged().await), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_past_the_quota_is_refused_and_leaves_nothing() {
    let f = start().await;
    f.set_file_quota(10_000).await;
    let before = f.blob_counts();
    let e = f.upload(1, 20_000, 1).await.unwrap_err();
    assert_eq!(status(*e), (429, "QUOTA_EXCEEDED".to_owned()));
    assert_eq!(f.blob_counts(), before);
    f.until("no replica keeps a mark", || f.marks() == 0).await;
    assert_eq!(f.charged().await, 0);

    let _ok = f.upload(0, 6_000, 2).await.unwrap();
    let e = f.upload(2, 6_000, 3).await.unwrap_err();
    assert_eq!(status(*e), (429, "QUOTA_EXCEEDED".to_owned()));
    assert_eq!(f.charged().await, 6_000);
    f.until("one file, marked on three replicas", || f.marks() == 3)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_creating_a_file_past_the_quota_fails_and_logs_nothing() {
    let f = start().await;
    f.set_file_quota(10_000).await;
    let (_id, _sha, _index) = f.put(6_000, 1).await;
    assert_eq!(f.charged().await, 6_000);
    let (before, applied) = (f.blob_counts(), f.applied());
    let pushes = f.node(f.leader()).file_pushes();
    let e = f.clients[0]
        .call(
            &f.app,
            "putFile",
            json!({ "len": "6000", "seed": "2" }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("QUOTA_EXCEEDED"), "{e}");
    assert_eq!(
        f.node(f.leader()).file_pushes(),
        pushes,
        "a refused charge pushed bytes"
    );
    assert_eq!(f.blob_counts(), before, "nothing was stored");
    assert_eq!(f.applied(), applied, "nothing was logged");
    f.until("the mark reaches every replica", || f.marks() == 3)
        .await;
    assert_eq!(f.charged().await, 6_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_file_refunds_it_and_removes_its_bytes() {
    let f = start().await;
    let up = f.upload(0, 50_000, 5).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let (_other, _sha, index) = f.put(1_000, 6).await;
    assert_eq!(f.charged().await, 51_000);
    f.read_on(2, &id, index).await.unwrap();

    let gone = f.clients[1].delete_blob(&f.app, &id).await.unwrap();
    for i in 0..3 {
        f.read_on(i, &id, gone["index"].as_u64().unwrap())
            .await
            .unwrap_err();
        assert!(f.mark(i, &id).is_none(), "node {i} keeps the mark");
    }
    f.wait_gone(&id).await;
    assert_eq!(f.charged().await, 1_000);
    let e = f.clients[0].delete_blob(&f.app, &id).await.unwrap_err();
    assert_eq!(status(e), (404, "NOT_FOUND".to_owned()));
    assert_eq!(f.charged().await, 1_000, "a second delete refunds nothing");

    // A call that deletes a file refunds it too.
    let (kept, _sha, _i) = f.put(2_000, 7).await;
    assert_eq!(f.charged().await, 3_000);
    let _r = f.clients[0]
        .call(&f.app, "dropFile", json!({ "id": kept }), None)
        .await
        .unwrap();
    assert_eq!(f.charged().await, 1_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_the_app_refunds_its_files_and_drops_their_bytes() {
    let f = start().await;
    let up = f.upload(0, 30_000, 8).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let payer = format!("account:{}", f.account().await);
    assert_eq!(f.charged().await, 30_000);
    let _ok = f.clients[0]
        .delete(&format!("apps/{}", f.app))
        .await
        .unwrap();
    assert_eq!(f.charged().await, 0);
    assert_eq!(f.node(0).sys(|s| s.file_bytes(&payer)), 0);
    f.wait_gone(&id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_file_commands_are_not_methods_anyone_can_call() {
    let f = start().await;
    let up = f.upload(0, 1_000, 9).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let before = (f.applied(), f.charged().await);
    let mark = json!({ "id": "ab".repeat(32), "size": 5, "sha256": "", "wrappedKey": "" });
    for (method, args) in [
        ("__file", json!({ "mark": mark })),
        ("__file_forget", json!({ "id": id })),
    ] {
        let e = f.clients[0]
            .call(&f.app, method, args.clone(), None)
            .await
            .unwrap_err();
        assert_ne!(e.code(), None, "{method}: {e}");
        let e = f.clients[0]
            .view(&f.app, method, args, Consistency::Strong)
            .await
            .unwrap_err();
        assert_ne!(e.code(), None, "{method} as a view: {e}");
    }
    assert_eq!((f.applied(), f.charged().await), before);
    f.until("the mark reaches every replica", || f.marks() == 3)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delete_racing_a_call_that_deletes_the_same_file_refunds_once() {
    let f = start().await;
    let up = f.upload(0, 50_000, 3).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let _kept = f.upload(0, 1_000, 4).await.unwrap();
    let (call, delete) = tokio::join!(
        f.clients[0].call(&f.app, "dropFile", json!({ "id": id }), None),
        f.clients[1].delete_blob(&f.app, &id),
    );
    assert!(call.is_ok() || delete.is_ok());
    assert_eq!(f.charged().await, 1_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retried_forget_replays_its_first_answer_and_refunds_once() {
    let f = start().await;
    let up = f.upload(0, 4_000, 5).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let caller = Caller::Account {
        account: f.account().await,
    };
    let req = CallRequest {
        idempotency_key: Some("node:forget-1".to_owned()),
        ..CallRequest::host(f.app.clone(), "__file_forget", json!({ "id": id }), caller)
    };
    let node = f.node(0);
    let first = node.call(&req).await.unwrap();
    let again = node.call(&req).await.unwrap();
    assert_eq!(first.result["size"], json!(4_000));
    assert_eq!((again.index, &again.result), (first.index, &first.result));
    f.until("no replica keeps a mark", || f.marks() == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_whose_commit_failed_before_it_applied_is_retried_without_leaking() {
    let f = start().await;
    f.node(f.leader()).fail_next_mark_commit(1);
    let before: u64 = f.blob_counts().iter().sum();
    let up = f.upload(f.leader(), 20_000, 1).await.unwrap();
    let id = up["id"].as_str().unwrap();
    // The first attempt's bytes and charge were undone; only the answered file remains.
    assert_eq!(f.charged().await, 20_000);
    f.until("the mark reaches every replica", || f.marks() == 3)
        .await;
    let held = f.holders(id).len() as u64;
    f.until("the first attempt's bytes are removed", || {
        f.blob_counts().iter().sum::<u64>() == before + held
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_whose_commit_failed_after_it_applied_succeeds() {
    let f = start().await;
    f.node(f.leader()).fail_next_mark_commit(2);
    let up = f.upload(f.leader(), 20_000, 1).await.unwrap();
    let id = up["id"].as_str().unwrap();
    f.until("the mark reaches every replica", || f.marks() == 3)
        .await;
    assert_eq!(f.charged().await, 20_000);
    assert!(f.mark(f.leader(), id).is_some());
    assert!(up["index"].as_u64().unwrap() > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stranger_cannot_upload_to_a_public_app() {
    let f = start().await;
    let alice = &f.clients[0];
    let bundle = alice
        .upload_bundle(common::component("files"))
        .await
        .unwrap();
    let open = alice
        .create_app("open", &bundle, json!({ "access": { "kind": "public" } }))
        .await
        .unwrap();
    let (bob, _pk) = signed_up(&f.c, 1, "bob").await;
    let e = bob.upload_blob(&open, stream(5_000, 1)).await.unwrap_err();
    assert_eq!(status(e), (403, "FORBIDDEN".to_owned()));
    let _ok = alice.upload_blob(&open, stream(5_000, 1)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_that_meets_a_recorded_answer_for_its_key_returns_it_and_undoes_its_own() {
    let f = start().await;
    f.node(f.leader()).fail_next_mark_commit(3);
    let before: u64 = f.blob_counts().iter().sum();
    let up = f.upload(f.leader(), 20_000, 1).await.unwrap();
    assert_eq!(up["id"], json!("ab".repeat(32)), "the recorded answer");
    assert_eq!(f.charged().await, 0, "the new file's charge was given back");
    f.until("the new file's bytes are removed", || {
        f.blob_counts().iter().sum::<u64>() == before
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_members_and_above_upload_to_a_public_app_of_an_organization() {
    let f = start().await;
    let alice = &f.clients[0];
    let org = alice.post("orgs", json!({ "name": "Docs" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice
        .upload_bundle(common::component("files"))
        .await
        .unwrap();
    let open = alice
        .create_app(
            "open",
            &bundle,
            json!({ "org": org, "access": { "kind": "public" } }),
        )
        .await
        .unwrap();
    let invite = |role: &'static str| {
        let org = org.clone();
        async move {
            alice
                .post(&format!("orgs/{org}/invites"), json!({ "role": role }))
                .await
                .unwrap()["token"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let (guest, _g) = signed_up(&f.c, 1, "gus").await;
    let _r = guest
        .post(
            &format!("invites/{}/redeem", invite("guest").await),
            json!({}),
        )
        .await
        .unwrap();
    let (member, _m) = signed_up(&f.c, 2, "mia").await;
    let _r = member
        .post(
            &format!("invites/{}/redeem", invite("member").await),
            json!({}),
        )
        .await
        .unwrap();
    let e = guest
        .upload_blob(&open, stream(5_000, 1))
        .await
        .unwrap_err();
    assert_eq!(status(e), (403, "FORBIDDEN".to_owned()));
    let _ok = member.upload_blob(&open, stream(5_000, 2)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_members_and_above_list_the_files_of_a_public_app_of_an_organization() {
    let f = start().await;
    let alice = &f.clients[0];
    let org = alice.post("orgs", json!({ "name": "Docs" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice
        .upload_bundle(common::component("files"))
        .await
        .unwrap();
    let open = alice
        .create_app(
            "open",
            &bundle,
            json!({ "org": org, "access": { "kind": "public" } }),
        )
        .await
        .unwrap();
    let cluster = &f.c;
    let redeem = |role: &'static str, node: usize, name: &'static str| {
        let org = org.clone();
        async move {
            let (c, _k) = signed_up(cluster, node, name).await;
            if role != "stranger" {
                let token = alice
                    .post(&format!("orgs/{org}/invites"), json!({ "role": role }))
                    .await
                    .unwrap()["token"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let _r = c
                    .post(&format!("invites/{token}/redeem"), json!({}))
                    .await
                    .unwrap();
            }
            c
        }
    };
    let plain = stream(5_000, 3);
    let up = alice.upload_blob(&open, plain.clone()).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();

    for (role, node, name) in [("stranger", 1, "sam"), ("guest", 1, "gus")] {
        let c = redeem(role, node, name).await;
        let e = c.list_blobs(&open).await.unwrap_err();
        assert_eq!(status(e), (403, "FORBIDDEN".to_owned()), "{role} list");
        let e = c.blob_info(&open, &id).await.unwrap_err();
        assert_eq!(status(e), (403, "FORBIDDEN".to_owned()), "{role} info");
    }
    let member = redeem("member", 2, "mia").await;
    for c in [&member, alice] {
        let listed = c.list_blobs(&open).await.unwrap();
        assert_eq!(listed["blobs"][0]["sha256"], json!(sha256_hex(&plain)));
        let info = c.blob_info(&open, &id).await.unwrap();
        assert_eq!(info["sha256"], json!(sha256_hex(&plain)));
    }
}
