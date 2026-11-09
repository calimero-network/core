//! Files of a component app on a cluster, through the client and the `files` fixture
//! (`apps/v2-fixtures/files`): stored on the replicas before the entry, retained by `BlobRef` and
//! collected when nothing refers to them. A guest reads a sealed file only inside a compute run
//! (`compute_runs.rs`), so these tests read through the provider such a run uses.

mod common;

use std::time::Duration;

use calimero_client::{Client, Consistency};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::component::blobs::BlobRecord;
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
    /// Alice's passkey, which approves her API keys.
    passkey: tokio::sync::Mutex<SoftAuthenticator>,
    /// The segments of each file a test saw, which outlive the file's record.
    seen: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
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
    let passkey = tokio::sync::Mutex::new(pk);
    Fixture {
        c,
        clients,
        app,
        passkey,
        seen: std::sync::Mutex::default(),
    }
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
        let _cached = self.segments_of(&id);
        (id, r.result["sha256"].as_str().unwrap().to_owned(), r.index)
    }

    /// The hashes of file `id`'s segments, from any node's record, or as a test saw them before
    /// the file was forgotten.
    fn segments_of(&self, id: &str) -> Vec<String> {
        self.find_segments(id).expect("a file this test never saw")
    }

    fn find_segments(&self, id: &str) -> Option<Vec<String>> {
        let found = (0..self.c.nodes.len())
            .filter(|i| self.c.nodes[*i].is_some())
            .find_map(|i| self.mark(i, id))
            .and_then(|r| r.file)
            .map(|f| f.segments);
        let mut seen = self.seen.lock().unwrap();
        match found {
            Some(segments) => {
                let _prev = seen.insert(id.to_owned(), segments.clone());
                Some(segments)
            }
            None => seen.get(id).cloned(),
        }
    }

    /// The ciphertext of file `id` as node `i` stores it: its segments, joined.
    fn stored(&self, i: usize, id: &str) -> Vec<u8> {
        (self.segments_of(id).iter())
            .flat_map(|h| self.node(i).engine.get_segment(h).unwrap().unwrap())
            .collect()
    }

    /// The size and SHA-256 of file `id` as node `i` reads it once it applied entry `index`.
    async fn read_on(&self, i: usize, id: &str, index: u64) -> Result<Value, String> {
        applied_on(self, i, index).await;
        let bytes = self.node(i).read_sealed_file(&self.app, id, 1)?;
        Ok(json!({ "size": bytes.len().to_string(), "sha256": sha256_hex(&bytes) }))
    }

    fn node(&self, i: usize) -> &calimero_node::Node {
        &self.c.nodes[i].as_ref().unwrap().node
    }

    /// The app's blob record of file `id` on node `i`.
    fn mark(&self, i: usize, id: &str) -> Option<BlobRecord> {
        let raw = <[u8; 32]>::try_from(hex::decode(id).unwrap()).unwrap();
        let g = self.node(i).hosted(&self.app)?;
        g.read_local(|m, _| m.state.component.blobs.records.get(&raw).cloned())
    }

    /// The nodes that store every segment of file `id`.
    fn holders(&self, id: &str) -> Vec<usize> {
        let segments = self.segments_of(id);
        (0..self.c.nodes.len())
            .filter(|i| self.c.nodes[*i].is_some())
            .filter(|i| {
                (segments.iter()).all(|h| self.node(*i).engine.get_segment(h).unwrap().is_some())
            })
            .collect()
    }

    /// The nodes that store any segment of file `id`.
    fn holders_of_any(&self, id: &str) -> Vec<usize> {
        let segments = self.segments_of(id);
        (0..self.c.nodes.len())
            .filter(|i| self.c.nodes[*i].is_some())
            .filter(|i| {
                (segments.iter()).any(|h| self.node(*i).engine.get_segment(h).unwrap().is_some())
            })
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
            if self.holders_of_any(id).is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "the bytes of {id} are still on {:?}",
            self.holders_of_any(id)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_file_reads_back_identically_on_every_replica() {
    let f = start().await;
    let (id, sha, index) = f.put(LEN, 7).await;
    assert_eq!(sha, sha256_hex(&stream(LEN, 7)));

    // Before the call answered, a majority held the bytes.
    assert!(f.holders(&id).len() >= 2, "held by {:?}", f.holders(&id));

    for i in 0..3 {
        let read = f.read_on(i, &id, index).await.unwrap();
        assert_eq!(
            read,
            json!({ "size": LEN.to_string(), "sha256": sha }),
            "node {i}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forty_mib_file_is_stored_as_three_segments_and_no_whole_file_row() {
    let f = start_with(Cluster::start_with(3, more_time).await).await;
    let rows_before = f.blob_counts();
    let (id, sha, index) = f.put(40 * MIB, 61).await;
    assert_eq!(sha, sha256_hex(&stream(40 * MIB, 61)));
    let segments = f.segments_of(&id);
    assert_eq!(segments.len(), 3, "{segments:?}");

    let holders = f.holders(&id);
    assert!(holders.len() >= 2, "held by {holders:?}");
    for i in holders {
        let node = f.node(i);
        let mut index_rows = node.engine.list_segments().unwrap();
        index_rows.sort();
        let mut want = segments.clone();
        want.sort();
        assert_eq!(index_rows, want, "node {i}: one index row per segment");
        assert_eq!(
            node.engine.blob_count().unwrap(),
            rows_before[i] + 3,
            "node {i}: three segment rows and nothing else"
        );
        assert!(node.engine.get_blob(&id).unwrap().is_none(), "node {i}");
        for hash in &segments {
            let segment = node.engine.get_segment(hash).unwrap().unwrap();
            assert_eq!(&sha256_hex(&segment), hash);
            assert!(segment.len() <= calimero_node::files::SEGMENT_BYTES);
        }
    }

    // A replica that lost a segment reads the file by fetching it again.
    let lost = (f.leader() + 1) % 3;
    f.node(lost).engine.delete_segment(&segments[1]).unwrap();
    assert_eq!(f.read_on(lost, &id, index).await.unwrap()["sha256"], sha);
    assert!(f
        .node(lost)
        .engine
        .get_segment(&segments[1])
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_writers_hold_one_segment_each_and_store_the_rest() {
    use calimero_node::files::{CHUNK_BYTES, SEGMENT_BYTES};

    let f = start().await;
    let node = f.node(f.leader());
    let stored_before = node.engine.list_segments().unwrap().len();
    node.write_test_files(&f.app, 3, 20 * MIB).unwrap();
    let (peak, bound) = (
        node.pending_file_bytes_peak(),
        3 * (SEGMENT_BYTES + 2 * CHUNK_BYTES),
    );
    assert!(
        peak <= bound,
        "writers held {peak} bytes, more than {bound}"
    );
    // Two segments per file, each stored the moment it was complete.
    let stored = node.engine.list_segments().unwrap().len();
    assert_eq!(stored - stored_before, 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stored_bytes_are_ciphertext_that_only_the_marks_key_opens() {
    let f = start().await;
    let (id, sha, index) = f.put(LEN, 11).await;
    let plain = stream(LEN, 11);
    let window = &plain[LEN / 2..LEN / 2 + 32];
    f.read_on(2, &id, index).await.unwrap();

    for i in f.holders(&id) {
        let stored = f.stored(i, &id);
        let segments = f.segments_of(&id);
        let hashes: Vec<[u8; 32]> = segments
            .iter()
            .map(|h| hex::decode(h).unwrap().try_into().unwrap())
            .collect();
        assert_eq!(
            id,
            hex::encode(calimero_node::files::file_id(&hashes)),
            "the id is the hash of the segment list"
        );
        assert!(stored.len() > LEN);
        assert!(
            !stored.windows(window.len()).any(|w| w == window),
            "node {i} stores plaintext"
        );

        let record = f.mark(i, &id).unwrap();
        let mark = record.file.as_ref().unwrap();
        assert_eq!(record.size, LEN as u64);
        assert_eq!(hex::encode(&mark.sha256.0), sha);
        let file_key = f.node(i).app_file_key(&f.app);
        let data_key = unwrap_key(&file_key, &f.app, &mark.wrapped_key.0).unwrap();
        let mut opener = FileOpener::new(&data_key);
        let mut opened = opener.push(&stored).unwrap();
        opened.extend(
            opener
                .finish_expecting(record.size, mark.sha256.0.as_slice().try_into().unwrap())
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
        assert!(g.read_local(|m, _| m.state.component.blobs.records.is_empty()));
    }
}

/// One hour and a little: past the idle time of a blob nothing refers to.
const PAST_IDLE_MS: i64 = 3_700_000;

impl Fixture {
    /// Advances every clock past the idle time and logs an entry, which forgets what is due.
    async fn pass_the_idle_time(&self) -> u64 {
        self.c.advance_all(PAST_IDLE_MS);
        let r = self.clients[0]
            .call(
                &self.app,
                "putUnkept",
                json!({ "len": "1", "seed": "99" }),
                None,
            )
            .await
            .unwrap();
        r.index
    }

    fn recorded_anywhere(&self, id: &str) -> bool {
        (0..3).any(|i| self.mark(i, id).is_some())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_kept_by_a_blob_ref_survives_the_idle_time() {
    let f = start().await;
    let (id, sha, _index) = f.put(200_000, 3).await;
    let index = f.pass_the_idle_time().await;
    for i in 0..3 {
        assert!(
            f.mark(i, &id).is_some(),
            "node {i} forgot a referenced file"
        );
    }
    assert!(f.holders(&id).len() >= 2);
    assert_eq!(
        f.read_on(f.leader(), &id, index).await.unwrap()["sha256"],
        sha
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_finished_but_never_stored_is_collected_on_every_replica() {
    let f = start().await;
    let r = f.clients[0]
        .call(
            &f.app,
            "putUnkept",
            json!({ "len": "100000", "seed": "5" }),
            None,
        )
        .await
        .unwrap();
    let id = r.result["id"].as_str().unwrap().to_owned();
    // Recorded by the entry that made it, so it cannot be collected before anything stores it.
    for i in 0..3 {
        applied_on(&f, i, r.index).await;
        assert!(f.mark(i, &id).is_some(), "node {i} has no record");
    }
    assert!(f.holders(&id).len() >= 2);
    let _index = f.pass_the_idle_time().await;
    f.until("a replica keeps the record", || !f.recorded_anywhere(&id))
        .await;
    f.wait_gone(&id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_whose_last_reference_is_removed_is_collected_on_every_replica() {
    let f = start().await;
    let (id, _sha, index) = f.put(200_000, 3).await;
    f.read_on(1, &id, index).await.unwrap();
    f.read_on(2, &id, index).await.unwrap();
    let _forgot = f.clients[0]
        .call(&f.app, "forgetFile", json!({ "id": id }), None)
        .await
        .unwrap();
    // Still held until its idle time passes, then forgotten in the same entry everywhere.
    assert!(f.mark(0, &id).is_some());
    let index = f.pass_the_idle_time().await;
    for i in 0..3 {
        applied_on(&f, i, index).await;
        assert!(f.mark(i, &id).is_none(), "node {i} keeps the record");
    }
    f.wait_gone(&id).await;
}

async fn applied_on(f: &Fixture, i: usize, index: u64) {
    f.until("a replica did not apply the entry", || {
        f.c.applied_on(i, &f.app) >= index
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_that_misses_the_bytes_fetches_them() {
    let f = start().await;
    let (id, sha, index) = f.put(500_000, 5).await;
    f.read_on(2, &id, index).await.unwrap();
    for h in f.segments_of(&id) {
        f.node(2).engine.delete_segment(&h).unwrap();
    }
    assert!(!f.holders(&id).contains(&2));
    let read = f.read_on(2, &id, index).await.unwrap();
    assert_eq!(read["sha256"], sha);
    assert!(f.holders(&id).contains(&2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_no_replica_holds_cannot_be_read() {
    let f = start().await;
    let (id, _sha, index) = f.put(100_000, 5).await;
    f.read_on(0, &id, index).await.unwrap();
    for i in 0..3 {
        for h in f.segments_of(&id) {
            f.node(i).engine.delete_segment(&h).unwrap();
        }
    }
    let e = f.read_on(1, &id, index).await.unwrap_err();
    assert!(e.contains("not available"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_and_a_view_cannot_open_a_sealed_file() {
    let f = start().await;
    let (id, _sha, _index) = f.put(100_000, 5).await;
    let e = f.clients[0]
        .call(&f.app, "touchFile", json!({ "id": id }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");
    assert!(format!("{:?}", e.details()).contains("compute run"), "{e}");
    let e = f.clients[0]
        .view(&f.app, "readFile", json!({ "id": id }), Consistency::Strong)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");
    assert!(format!("{:?}", e.details()).contains("compute run"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corrupt_copy_is_dropped_and_healed_from_a_replica() {
    let f = start_with(Cluster::start_with(3, more_time).await).await;
    let (id, sha, index) = f.put(LEN, 9).await;
    let leader = f.leader();
    let follower = (leader + 1) % 3;
    let segments = f.segments_of(&id);
    for i in [follower, leader] {
        let mut stored = f.node(i).engine.get_segment(&segments[0]).unwrap().unwrap();
        let at = stored.len() / 2;
        stored[at] ^= 1;
        f.node(i).engine.put_segment(&segments[0], &stored).unwrap();
    }
    // The follower and the leader each find their copy is not the file, and read it from the
    // replica that still holds it.
    for i in [follower, leader] {
        assert_eq!(f.read_on(i, &id, index).await.unwrap()["sha256"], sha);
    }
    for i in [follower, leader] {
        let healed = f.node(i).engine.get_segment(&segments[0]).unwrap().unwrap();
        assert_eq!(
            sha256_hex(&healed),
            segments[0],
            "node {i} keeps a corrupt copy"
        );
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
async fn a_dropped_reference_in_an_entry_replaced_as_too_large_keeps_the_file() {
    let f = start().await;
    let (id, sha, index) = f.put(100_000, 6).await;
    let e = f.clients[0]
        .call(
            &f.app,
            "forgetPadded",
            json!({ "id": id, "pad": 800 * 1024 }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["detail"], "entry too large", "{e}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(f.holders(&id).len() >= 2, "the bytes were collected");
    for i in 0..3 {
        assert!(f.mark(i, &id).is_some(), "node {i} lost the record");
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
    // Room for the stored file only once at a time (500_000 bytes and 8 chunk tags).
    f.node(leader).limit_open_file_bytes(500_128);
    for _ in 0..2 {
        assert_eq!(f.read_on(leader, &id, index).await.unwrap()["sha256"], sha);
    }
    f.node(leader).limit_open_file_bytes(100_000);
    let e = f.read_on(leader, &id, index).await.unwrap_err();
    assert!(e.contains("not available"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blob_swapped_for_another_files_ciphertext_is_never_read() {
    let f = start().await;
    let (a, _sha, index) = f.put(100_000, 20).await;
    let (b, _sha, index_b) = f.put(100_000, 21).await;
    f.read_on(2, &a, index).await.unwrap();
    let other = f.stored(0, &b);
    let a_segments = f.segments_of(&a);
    for i in 0..3 {
        f.node(i)
            .engine
            .put_segment(&a_segments[0], &other)
            .unwrap();
    }
    let e = f.read_on(2, &a, index_b).await.unwrap_err();
    assert!(e.contains("not available"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_of_three_segments_to_two_replicas_holds_two_segments_at_most() {
    use calimero_node::files::SEGMENT_BYTES;

    let f = start_with(Cluster::start_with(3, more_time).await).await;
    let _file = f.put(40 * MIB, 63).await;
    let peak = f.node(f.leader()).push_bytes_peak();
    assert!(peak > 0, "the push took no budget");
    assert!(peak <= 2 * SEGMENT_BYTES, "a push held {peak} bytes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_holds_one_segment_so_a_larger_file_opens_under_a_small_cap() {
    let f = start_with(Cluster::start_with(3, more_time).await).await;
    let (id, sha, index) = f.put(40 * MIB, 62).await;
    let leader = f.leader();
    // A segment and its plaintext fit in 34 MiB, the whole file does not.
    f.node(leader).limit_run_open_bytes(34 * MIB);
    assert_eq!(f.read_on(leader, &id, index).await.unwrap()["sha256"], sha);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_run_cannot_hold_more_than_its_cap_open() {
    let f = start().await;
    let (id, _sha, _index) = f.put(500_000, 8).await;
    let leader = f.leader();
    // Room for the file once (500_000 bytes and 8 chunk tags) in one run, not twice.
    f.node(leader).limit_run_open_bytes(600_000);
    assert!(f.node(leader).read_sealed_file(&f.app, &id, 1).is_ok());
    let e = f.node(leader).read_sealed_file(&f.app, &id, 2).unwrap_err();
    assert!(e.contains("not available"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflicting_entry_removes_the_files_stored_for_it() {
    let f = start().await;
    let leader = f.leader();
    let before = f.node(leader).engine.blob_count().unwrap();
    f.node(leader).conflict_next_call();
    // The first run conflicts and its file is dropped; the call runs again.
    let (_id, _sha, _index) = f.put(100_000, 40).await;
    f.until("the conflicted run left its bytes behind", || {
        (0..3).all(|i| f.node(i).engine.blob_count().unwrap() <= before + 1)
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
        .post_operator_signed(
            &format!("ops/apps/{}/replicas", f.app),
            json!({ "replicas": target }),
            &common::operator_sign,
        )
        .await
        .unwrap();
    let joined = f.c.nodes[spare as usize - 1].as_ref().unwrap();
    for h in f.segments_of(&id) {
        assert!(joined.node.engine.get_segment(&h).unwrap().is_some());
    }
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
    assert!(mark.file.is_some());
    assert_eq!(f.mark(leader, &id), Some(mark));
    // The bytes were pushed while it was down: it fetches them.
    let read = f.read_on(lagging, &id, index).await.unwrap();
    assert_eq!(read["size"], "70000");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blobs_info_answers_for_a_file_and_blobs_grant_refuses_it() {
    let f = start().await;
    let (id, sha, index) = f.put(100_000, 14).await;
    for i in 0..3 {
        let info = f.clients[i]
            .view(
                &f.app,
                "blobInfo",
                json!({ "id": id }),
                Consistency::Stale { min_index: index },
            )
            .await
            .unwrap();
        assert_eq!(
            info.result,
            json!({ "size": "100000", "sha256": sha, "mime": "" }),
            "node {i}"
        );
    }
    let e = f.clients[0]
        .view(
            &f.app,
            "shareFile",
            json!({ "id": id }),
            Consistency::Strong,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");
    let none = f.clients[0]
        .view(
            &f.app,
            "blobInfo",
            json!({ "id": "11".repeat(32) }),
            Consistency::Strong,
        )
        .await
        .unwrap();
    assert_eq!(none.result, Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_blob_routes_refuse_a_file_id() {
    let f = start().await;
    let (id, _sha, _index) = f.put(100_000, 15).await;
    let alice = &f.clients[0];
    let refused = |e: calimero_client::Error, what: &str| {
        assert_eq!(e.code(), Some("NOT_SUPPORTED"), "{what}: {e}");
    };
    refused(
        alice.download_blob(&f.app, &id).await.unwrap_err(),
        "download",
    );
    // Another node's route answers the same.
    refused(
        f.clients[2].download_blob(&f.app, &id).await.unwrap_err(),
        "download",
    );
    refused(alice.delete_blob(&f.app, &id).await.unwrap_err(), "delete");
    // The preimage of the id, uploaded as a blob, would claim the file's id.
    let mut preimage = b"calimero.file.segments.v1\0".to_vec();
    let hashes = f.segments_of(&id);
    preimage.extend_from_slice(&u32::try_from(hashes.len()).unwrap().to_be_bytes());
    for h in &hashes {
        preimage.extend_from_slice(&hex::decode(h).unwrap());
    }
    assert_eq!(sha256_hex(&preimage), id);
    refused(
        alice.upload_blob(&f.app, preimage).await.unwrap_err(),
        "upload",
    );
    // The owner sees the file in the listing, as a sealed file and not as a blob.
    let listed = alice.list_blobs(&f.app).await.unwrap();
    assert_eq!(listed["blobs"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["blobs"][0]["sealed"], json!(true));
    for i in 0..3 {
        assert!(f.mark(i, &id).is_some_and(|r| r.file.is_some()), "node {i}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ciphertext_runs_hold_before_their_entries_is_bounded_and_returned() {
    let f = start().await;
    let leader = f.leader();
    let node = f.node(leader);
    let full = node.pending_file_bytes_left();
    let (_id, _sha, _index) = f.put(100_000, 50).await;
    assert_eq!(
        node.pending_file_bytes_left(),
        full,
        "a finished call kept bytes"
    );

    // Other runs hold all but 150_000 bytes: a bigger file cannot be held, and the call is
    // aborted and retried, not logged.
    node.limit_pending_file_bytes(150_000);
    let applied = f.applied();
    let before = f.blob_counts();
    let e = f.clients[0]
        .call(
            &f.app,
            "putFile",
            json!({ "len": "400000", "seed": "51" }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TIMEOUT"), "{e}");
    assert_eq!(f.applied(), applied, "the refused call was logged");
    assert_eq!(f.blob_counts(), before);
    assert_eq!(
        node.pending_file_bytes_left(),
        150_000,
        "a failed run kept bytes"
    );

    // What was left still takes a smaller file, and gives the bytes back.
    let (_id, _sha, _index) = f.put(100_000, 52).await;
    assert_eq!(node.pending_file_bytes_left(), 150_000);
}

/// The file bytes `usage` shows for `payer`.
fn used(usage: &Value, payer: &str) -> u64 {
    usage["fileBytes"][payer].as_u64().unwrap_or(0)
}

fn status(e: calimero_client::Error) -> (u16, String) {
    match e {
        calimero_client::Error::Api { status, code, .. } => (status, code),
        other => panic!("{other:?}"),
    }
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
            .upload_file(&self.app, stream(len, seed))
            .await;
        if let Ok(up) = &sent {
            let _cached = self.find_segments(up["id"].as_str().unwrap());
        }
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
            .post_operator_signed(
                "ops/quotas",
                json!({ "maxFileBytesPerOrg": bytes }),
                &common::operator_sign,
            )
            .await
            .unwrap();
    }

    /// The file records the replicas hold, summed.
    fn marks(&self) -> usize {
        (0..3)
            .map(|i| {
                let g = self.node(i).hosted(&self.app).unwrap();
                g.read_local(|m, _| {
                    (m.state.component.blobs.records.values())
                        .filter(|r| r.file.is_some())
                        .count()
                })
            })
            .sum()
    }

    /// Waits for the quota's count of alice's file bytes to reach `bytes`.
    async fn until_charged(&self, bytes: u64) {
        for _ in 0..200 {
            if self.charged().await == bytes {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("charged {} instead of {bytes}", self.charged().await);
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
        let stored = f.stored(i, &id);
        assert!(
            !stored.windows(window.len()).any(|w| w == window),
            "node {i}"
        );
        assert!(f.mark(i, &id).is_some_and(|r| r.file.is_some()));
    }

    let listed = f.clients[0].list_blobs(&f.app).await.unwrap();
    assert_eq!(listed["blobs"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["blobs"][0]["id"], json!(id));
    assert_eq!(listed["blobs"][0]["sha256"], json!(sha256_hex(&plain)));
    assert_eq!(f.charged().await, LEN as u64);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_uploaded_file_answers_its_uploader_through_every_node_and_a_made_file_none() {
    let f = start().await;
    let alice = f.account().await;
    let who = |id: String| {
        let f = &f;
        async move {
            let r = f.clients[0]
                .view(
                    &f.app,
                    "uploaderOf",
                    json!({ "id": id }),
                    Consistency::Strong,
                )
                .await
                .unwrap();
            r.result
        }
    };
    let mut ids = Vec::new();
    for via in 0..3 {
        let up = f.upload(via, 5_000, 1).await.unwrap();
        ids.push(up["id"].as_str().unwrap().to_owned());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        3,
        "each upload is sealed under a key of its own: three files"
    );
    for id in ids {
        assert_eq!(who(id).await, json!(alice));
    }
    let (made, ..) = f.put(5_000, 2).await;
    assert_eq!(who(made).await, Value::Null);
    assert_eq!(who("ab".repeat(32)).await, Value::Null);
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
async fn a_read_only_key_cannot_upload() {
    let f = start().await;
    let (_id, key) = f.clients[0]
        .create_api_key(
            json!({ "readOnly": true, "apps": [f.app] }),
            &mut *f.passkey.lock().await,
        )
        .await
        .unwrap();
    let agent = f.c.client(2);
    agent.use_api_key(&key).await;
    let before = (f.blob_counts(), f.marks(), f.charged().await);
    let e = agent
        .upload_file(&f.app, stream(5000, 1))
        .await
        .unwrap_err();
    assert_eq!(status(e), (403, "FORBIDDEN".to_owned()));
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
async fn an_unreferenced_file_is_forgotten_after_the_idle_time_and_releases_its_quota() {
    let f = start().await;
    let up = f.upload(0, 30_000, 8).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let (kept, _sha, _index) = f.put(2_000, 9).await;
    assert_eq!(f.charged().await, 32_000);
    let _index = f.pass_the_idle_time().await;
    // The unreferenced upload is collected and released; the file a value refers to stays
    // charged, and the run's own unreferenced file (1 byte) is not due yet.
    f.until("a replica keeps the upload's record", || {
        !f.recorded_anywhere(&id)
    })
    .await;
    f.wait_gone(&id).await;
    f.until_charged(2_000 + 1).await;
    assert!(f.mark(0, &kept).is_some());
    assert!(f.holders(&kept).len() >= 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_collection_releases_only_what_it_collected() {
    let f = start().await;
    let first = f.upload(0, 30_000, 1).await.unwrap();
    let _index = f.pass_the_idle_time().await;
    f.until_charged(1).await;
    assert!(!f.recorded_anywhere(first["id"].as_str().unwrap()));
    // A later upload and collection leave the one-byte file of the run that is not due.
    let second = f.upload(0, 20_000, 2).await.unwrap();
    f.until_charged(20_001).await;
    let _index = f.pass_the_idle_time().await;
    f.until_charged(1).await;
    assert!(!f.recorded_anywhere(second["id"].as_str().unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_file_commands_are_not_methods_anyone_can_call() {
    let f = start().await;
    let _up = f.upload(0, 1_000, 9).await.unwrap();
    let before = (f.applied(), f.charged().await);
    let mark = json!({ "id": "ab".repeat(32), "size": 5, "sha256": "", "wrappedKey": "" });
    let args = json!({ "mark": mark });
    let e = f.clients[0]
        .call(&f.app, "__file", args.clone(), None)
        .await
        .unwrap_err();
    assert_ne!(e.code(), None, "{e}");
    let e = f.clients[0]
        .view(&f.app, "__file", args, Consistency::Strong)
        .await
        .unwrap_err();
    assert_ne!(e.code(), None, "as a view: {e}");
    assert_eq!((f.applied(), f.charged().await), before);
    f.until("the mark reaches every replica", || f.marks() == 3)
        .await;
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
async fn an_upload_that_meets_a_recorded_answer_for_its_key_returns_it_and_undoes_its_own() {
    let f = start().await;
    f.node(f.leader()).fail_next_mark_commit(3);
    let before: u64 = f.blob_counts().iter().sum();
    let up = f.upload(f.leader(), 20_000, 1).await.unwrap();
    let recorded = hex::encode(calimero_node::files::file_id(&[[0xab; 32]]));
    assert_eq!(up["id"], json!(recorded), "the recorded answer");
    f.until_charged(0).await;
    f.until("the new file's bytes are removed", || {
        f.blob_counts().iter().sum::<u64>() == before
    })
    .await;
}

/// An organization with a public app of its own, and a signed-up client of each role.
struct Org {
    app: String,
    guest: Client,
    member: Client,
    stranger: Client,
}

async fn org_with_a_public_app(f: &Fixture) -> Org {
    let alice = &f.clients[0];
    let org = alice.post("orgs", json!({ "name": "Docs" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice
        .upload_bundle(common::component("files"))
        .await
        .unwrap();
    let app = alice
        .create_app(
            "open",
            &bundle,
            json!({ "org": org, "access": { "kind": "public" } }),
        )
        .await
        .unwrap();
    let join = |role: &'static str, node: usize, name: &'static str| {
        let org = org.clone();
        async move {
            let (c, _k) = signed_up(&f.c, node, name).await;
            if role != "stranger" {
                let token = alice
                    .post_confirmed(
                        &format!("orgs/{org}/invites"),
                        json!({ "role": role }),
                        &mut *f.passkey.lock().await,
                    )
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
    Org {
        app,
        guest: join("guest", 1, "gus").await,
        member: join("member", 2, "mia").await,
        stranger: join("stranger", 1, "sam").await,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_members_and_above_upload_to_a_public_app_of_an_organization() {
    let f = start().await;
    let o = org_with_a_public_app(&f).await;
    for (who, c) in [("guest", &o.guest), ("stranger", &o.stranger)] {
        let e = c.upload_file(&o.app, stream(5_000, 1)).await.unwrap_err();
        assert_eq!(status(e), (403, "FORBIDDEN".to_owned()), "{who}");
    }
    let _ok = o
        .member
        .upload_file(&o.app, stream(5_000, 2))
        .await
        .unwrap();
    let _ok = f.clients[0]
        .upload_file(&o.app, stream(5_000, 3))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_members_and_above_list_the_files_of_a_public_app_of_an_organization() {
    let f = start().await;
    let o = org_with_a_public_app(&f).await;
    let plain = stream(5_000, 3);
    let up = f.clients[0]
        .upload_file(&o.app, plain.clone())
        .await
        .unwrap();
    let id = up["id"].as_str().unwrap();

    // Others may list the app's blobs, but a file's hash is the plaintext's: they see none.
    for (who, c) in [("guest", &o.guest), ("stranger", &o.stranger)] {
        assert_eq!(
            c.list_blobs(&o.app).await.unwrap()["blobs"],
            json!([]),
            "{who}"
        );
    }
    for (who, c) in [("member", &o.member), ("owner", &f.clients[0])] {
        let listed = c.list_blobs(&o.app).await.unwrap();
        assert_eq!(listed["blobs"][0]["id"], json!(id), "{who}");
        assert_eq!(
            listed["blobs"][0]["sha256"],
            json!(sha256_hex(&plain)),
            "{who}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_info_of_a_file_is_its_size_and_hash_for_those_who_list_files_and_unknown_to_the_rest()
{
    let f = start().await;
    let o = org_with_a_public_app(&f).await;
    let plain = stream(5_000, 3);
    let up = f.clients[0]
        .upload_file(&o.app, plain.clone())
        .await
        .unwrap();
    let id = up["id"].as_str().unwrap();
    let raw = <[u8; 32]>::try_from(hex::decode(id).unwrap()).unwrap();
    f.until("every replica applied the upload", || {
        (0..f.c.nodes.len())
            .filter_map(|i| f.node(i).hosted(&o.app))
            .all(|g| g.read_local(|m, _| m.state.component.blobs.records.contains_key(&raw)))
    })
    .await;

    for (who, c) in [("member", &o.member), ("owner", &f.clients[0])] {
        let info = c.blob_info(&o.app, id).await.unwrap();
        assert_eq!(info["size"], json!(5_000), "{who}");
        assert_eq!(info["sha256"], json!(sha256_hex(&plain)), "{who}");
        // Never the key or any content.
        let mut keys: Vec<_> = info.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["blobId", "id", "sealed", "sha256", "size"], "{who}");
    }
    // A caller who may call the app but not list its files learns nothing of the file.
    for (who, c) in [("guest", &o.guest), ("stranger", &o.stranger)] {
        let e = c.blob_info(&o.app, id).await.unwrap_err();
        assert_eq!(status(e), (404, "NOT_FOUND".to_owned()), "{who}");
    }
    // Downloading it stays refused to everyone.
    for c in [&o.member, &f.clients[0]] {
        let e = c.download_blob(&o.app, id).await.unwrap_err();
        assert_eq!(status(e), (409, "NOT_SUPPORTED".to_owned()));
    }
}

/// A replica restarted after an upload committed has not applied it yet; what blob info and the
/// blob list read must still hold the upload, as the leader has it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_info_and_the_list_of_an_upload_are_read_from_the_leader() {
    let mut f = start_with(
        Cluster::start_with(3, |cfg| {
            more_fuel(cfg);
            cfg.raft.heartbeat_ms = 2_000;
            cfg.raft.election_min_ms = 8_000;
            cfg.raft.election_max_ms = 12_000;
        })
        .await,
    )
    .await;
    let leader = f.leader();
    let follower = (leader + 1) % 3;
    f.c.stop(follower).await;
    let owner = &f.clients[leader];
    let plain = owner.upload_blob(&f.app, stream(2_000, 7)).await.unwrap();
    let file = owner.upload_file(&f.app, stream(4_000, 9)).await.unwrap();
    let (plain, file) = (plain["id"].as_str().unwrap(), file["id"].as_str().unwrap());
    f.c.restart(follower, |_| {}).await;
    while f.node(follower).hosted(&f.app).is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let app = f.node(follower).sys(|s| s.apps[&f.app].clone());
    let (plain, file) = (hex_id(plain), hex_id(file));
    let local = f.node(follower).storage_stats(&app).await.unwrap();
    assert!(
        local.blobs.iter().all(|(id, _)| *id != plain),
        "the replica already applied the blob"
    );
    let stats = f.node(follower).storage_stats_strong(&app).await.unwrap();
    assert!(
        stats.blobs.iter().any(|(id, _)| *id == plain),
        "blob unseen"
    );
    assert!(
        stats.files.iter().any(|(id, ..)| *id == file),
        "file unseen"
    );
}

/// Waits until node `i` hosts the app, then answers its group.
async fn hosted_after_restart(f: &Fixture, i: usize) {
    while f.node(i).hosted(&f.app).is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn slow_raft(cfg: &mut calimero_node::Config) {
    more_fuel(cfg);
    cfg.raft.heartbeat_ms = 2_000;
    cfg.raft.election_min_ms = 8_000;
    cfg.raft.election_max_ms = 12_000;
}

fn listed(stats: &Value, id: &[u8; 32]) -> bool {
    let id = hex::encode(id);
    stats["blobs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b[0] == json!(id))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn internal_storage_answers_strongly_only_as_the_leader() {
    let mut f = start_with(Cluster::start_with(3, slow_raft).await).await;
    let leader = f.leader();
    let follower = (leader + 1) % 3;
    f.c.stop(follower).await;
    let up = f.clients[leader]
        .upload_blob(&f.app, stream(2_000, 7))
        .await
        .unwrap();
    let id = hex_id(up["id"].as_str().unwrap());
    f.c.restart(follower, |_| {}).await;
    hosted_after_restart(&f, follower).await;

    let addr = |i: usize| f.node(0).sys(|s| s.nodes[&(i as u64 + 1)].addr.clone());
    let ask = |strong: Option<bool>| {
        let mut body = json!({ "app": f.app });
        if let Some(s) = strong {
            body["strong"] = json!(s);
        }
        body
    };
    let from = f.node(leader);
    let local = from
        .peer_post(&addr(follower), "internal/storage", &ask(None))
        .await
        .unwrap();
    assert!(!listed(&local, &id), "the replica already applied the blob");
    let refused = from
        .peer_post(&addr(follower), "internal/storage", &ask(Some(true)))
        .await;
    assert!(refused.is_err(), "a follower answered a strong read");
    let strong = f
        .node(follower)
        .peer_post(&addr(leader), "internal/storage", &ask(Some(true)))
        .await
        .unwrap();
    assert!(listed(&strong, &id), "{strong}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_that_hosts_no_replica_reads_the_upload_from_the_leader() {
    let mut f = start_with(Cluster::start_with(4, slow_raft).await).await;
    let app = f.node(0).sys(|s| s.apps[&f.app].clone());
    let outsider = (1..=4).find(|n| !app.replicas.contains(n)).unwrap() as usize - 1;
    // The outsider asks the replicas in order: make the first a lagging follower.
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
    let leader = f.leader();
    assert_ne!(leader, first, "the first replica still leads");
    f.c.stop(first).await;
    let up = f.clients[leader]
        .upload_blob(&f.app, stream(2_000, 7))
        .await
        .unwrap();
    let id = hex_id(up["id"].as_str().unwrap());
    f.c.restart(first, |_| {}).await;
    hosted_after_restart(&f, first).await;

    let stats = f.node(outsider).storage_stats_strong(&app).await.unwrap();
    assert!(stats.blobs.iter().any(|(b, _)| *b == id), "blob unseen");
}

fn hex_id(id: &str) -> [u8; 32] {
    <[u8; 32]>::try_from(hex::decode(id).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deleted_apps_sealed_files_leave_every_replica_and_the_quota() {
    let f = start().await;
    let up = f.upload(0, 30_000, 8).await.unwrap();
    let uploaded = up["id"].as_str().unwrap().to_owned();
    let (made, _sha, _index) = f.put(2_000, 9).await;
    assert_eq!(f.charged().await, 32_000);
    assert!(!f.holders(&uploaded).is_empty() && !f.holders(&made).is_empty());

    let _ok = f.clients[0]
        .delete(&format!("apps/{}", f.app))
        .await
        .unwrap();
    f.wait_gone(&uploaded).await;
    f.wait_gone(&made).await;
    assert_eq!(f.charged().await, 0);
}
