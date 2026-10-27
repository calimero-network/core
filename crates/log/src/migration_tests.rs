//! A node data directory in the previous format (`log.redb`) is migrated when
//! the engine opens it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use openraft::{CommittedLeaderId, EntryPayload, LogId, Membership, StoredMembership};

use crate::legacy::LegacyWriter;
use crate::machine::LegacySnapshot;
use crate::types::{Entry, SnapshotMeta};
use crate::{
    BoxFuture, Bytes, Config, Engine, GroupId, GroupState, Machine, NodeId, Proposal, RpcRequest,
    RpcResponse, StateTx, Transport,
};

const KEY: [u8; 32] = [9; 32];

struct Nowhere;

impl Transport for Nowhere {
    fn send(
        &self,
        _target: NodeId,
        _addr: String,
        _group: GroupId,
        _request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>> {
        Box::pin(async { Err("single node".to_owned()) })
    }
}

/// In memory, as the system group's machine is.
#[derive(Default)]
struct Mem(BTreeMap<String, String>);

impl Machine for Mem {
    const IN_MEMORY: bool = true;

    fn apply(&mut self, _i: u64, command: &[u8], _s: &mut StateTx<'_>) -> Vec<u8> {
        let s = String::from_utf8_lossy(command);
        let (k, v) = s.split_once('=').unwrap_or((&s, ""));
        let _prev = self.0.insert(k.to_owned(), v.to_owned());
        Vec::new()
    }

    fn snapshot(&self) -> Vec<u8> {
        serde_json::to_vec(&self.0).unwrap()
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        self.0 = serde_json::from_slice(data)?;
        Ok(())
    }
}

/// On disk, as an app's machine is: it imports the old JSON state.
#[derive(Default)]
struct Disk(Option<GroupState>);

impl Disk {
    fn get(&self, k: &str) -> Option<String> {
        String::from_utf8(self.0.as_ref()?.get_hidden(b'k', k.as_bytes())?).ok()
    }
}

impl Machine for Disk {
    fn attach(&mut self, state: GroupState) {
        self.0 = Some(state);
    }

    fn apply(&mut self, _i: u64, command: &[u8], s: &mut StateTx<'_>) -> Vec<u8> {
        let c = String::from_utf8_lossy(command);
        let (k, v) = c.split_once('=').unwrap_or((&c, ""));
        s.put_hidden(b'k', k.as_bytes(), v.as_bytes());
        Vec::new()
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        Ok(())
    }

    fn import(&mut self, data: &[u8], s: &mut StateTx<'_>) -> eyre::Result<()> {
        let old: BTreeMap<String, String> = serde_json::from_slice(data)?;
        for (k, v) in old {
            s.put_hidden(b'k', k.as_bytes(), v.as_bytes());
        }
        Ok(())
    }
}

fn log_id(index: u64) -> LogId<NodeId> {
    LogId::new(CommittedLeaderId::new(1, 1), index)
}

/// What the previous format held for one group of one node: a membership
/// entry, two writes covered by a snapshot, and two after it.
fn write_legacy(w: &LegacyWriter, group: GroupId) {
    let membership = Membership::new(
        vec![[1_u64].into_iter().collect()],
        [(1_u64, openraft::BasicNode { addr: "n1".into() })]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
    );
    let entries = vec![
        Entry {
            log_id: log_id(0),
            payload: EntryPayload::Membership(membership.clone()),
        },
        Entry {
            log_id: log_id(1),
            payload: EntryPayload::Normal(Bytes(b"a=1".to_vec())),
        },
        Entry {
            log_id: log_id(2),
            payload: EntryPayload::Normal(Bytes(b"b=2".to_vec())),
        },
        Entry {
            log_id: log_id(3),
            payload: EntryPayload::Normal(Bytes(b"c=3".to_vec())),
        },
        Entry {
            log_id: log_id(4),
            payload: EntryPayload::Normal(Bytes(b"a=4".to_vec())),
        },
    ];
    for e in &entries {
        w.put_entry(group, e).unwrap();
    }
    w.put_meta(group, "vote", &openraft::Vote::new_committed(1, 1))
        .unwrap();
    w.put_meta(group, "committed", &Some(log_id(4))).unwrap();
    let state: BTreeMap<String, String> =
        [("a".into(), "1".into()), ("b".into(), "2".into())].into();
    w.put_meta(
        group,
        "snapshot",
        &LegacySnapshot {
            meta: SnapshotMeta {
                last_log_id: Some(log_id(2)),
                last_membership: StoredMembership::new(Some(log_id(0)), membership),
                snapshot_id: "2-1".into(),
            },
            data: Bytes(serde_json::to_vec(&state).unwrap()),
        },
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redb_data_dir_is_migrated_on_open() {
    let dir = tempfile::tempdir().unwrap();
    {
        let w = LegacyWriter::create(&dir.path().join(crate::legacy::FILE), &KEY).unwrap();
        write_legacy(&w, 0);
        write_legacy(&w, 7);
        w.put_blob("bundle-1", b"wasm bytes").unwrap();
    }
    let config = Config {
        heartbeat_ms: 50,
        election_min_ms: 150,
        election_max_ms: 300,
        snapshot_every: 1000,
    };
    let engine = Engine::open(1, dir.path(), &KEY, Arc::new(Nowhere), config.clone()).unwrap();
    // The old file is kept aside, never read again.
    assert!(!dir.path().join("log.redb").exists());
    assert!(dir.path().join("log.redb.migrated").exists());
    assert_eq!(
        engine.get_blob("bundle-1").unwrap().as_deref(),
        Some(&b"wasm bytes"[..])
    );

    let mem = engine.start_group(0, Mem::default()).await.unwrap();
    let disk = engine.start_group(7, Disk::default()).await.unwrap();
    // The snapshot (a=1, b=2 at 2), then the entries after it (c=3, a=4).
    for g in [mem.applied(), disk.applied()] {
        assert!(g >= 2);
    }
    assert!(mem.wait_applied(4, Duration::from_secs(10)).await);
    assert!(disk.wait_applied(4, Duration::from_secs(10)).await);
    let m = mem.read_local(|m, _| m.0.clone());
    assert_eq!(m.get("a").map(String::as_str), Some("4"));
    assert_eq!(m.get("b").map(String::as_str), Some("2"));
    assert_eq!(m.get("c").map(String::as_str), Some("3"));
    assert_eq!(disk.read_local(|m, _| m.get("a")).as_deref(), Some("4"));
    assert_eq!(disk.read_local(|m, _| m.get("b")).as_deref(), Some("2"));
    assert_eq!(disk.read_local(|m, _| m.get("c")).as_deref(), Some("3"));

    // The migrated groups keep their history and take new writes.
    let p = disk
        .propose(|_m, _| Proposal::Append(b"d=5".to_vec()))
        .await
        .unwrap();
    assert!(p.index > 4);
    assert_eq!(disk.read_local(|m, _| m.get("d")).as_deref(), Some("5"));

    // Reopened, nothing is migrated again and the state is still there.
    engine.shutdown().await;
    drop((mem, disk, engine));
    let engine = Engine::open(1, dir.path(), &KEY, Arc::new(Nowhere), config).unwrap();
    let disk = engine.start_group(7, Disk::default()).await.unwrap();
    assert_eq!(disk.read_local(|m, _| m.get("d")).as_deref(), Some("5"));
    assert_eq!(disk.read_local(|m, _| m.get("a")).as_deref(), Some("4"));
}
