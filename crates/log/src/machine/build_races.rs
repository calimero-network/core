//! A snapshot build that runs beside an install or a replace of the same
//! group, as openraft runs builds beside its state machine worker. The
//! `testing` hooks hold the build at the exact point the race needs.

use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use openraft::CommittedLeaderId;

use super::*;
use crate::testing::{build_waiting, hold_build, release_build, BuildPoint};

const KEY: [u8; 32] = [3; 32];
const GROUP: GroupId = 1;

/// A disk machine: a command `k=v` sets row `k` to `v`.
struct Rows;

impl Machine for Rows {
    fn apply(&mut self, _index: u64, command: &[u8], state: &mut StateTx<'_>) -> Vec<u8> {
        let command = String::from_utf8_lossy(command);
        if let Some((k, v)) = command.split_once('=') {
            state.put(k.as_bytes().to_vec(), v.as_bytes().to_vec());
        }
        Vec::new()
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        Ok(())
    }
}

type State = Vec<(String, String)>;

fn new_state(rows: &[(&str, &str)]) -> State {
    rows.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn entry(index: u64, command: &str) -> Entry {
    Entry {
        log_id: LogId::new(CommittedLeaderId::new(1, 1), index),
        payload: EntryPayload::Normal(Bytes(command.as_bytes().to_vec())),
    }
}

/// A machine at `dir` that has applied `commands` as entries 1, 2, ...
async fn machine_with(dir: &Path, commands: &[&str]) -> StateMachine<Rows> {
    let db = Db::open(dir, &KEY).unwrap();
    let mut sm = StateMachine::open(GROUP, db, Rows, Arc::new(watch::channel(0).0)).unwrap();
    let entries = (1..).zip(commands).map(|(i, c)| entry(i, c));
    let _ = sm.apply(entries.collect::<Vec<_>>()).await.unwrap();
    sm
}

/// The index and rows of a snapshot a build handed to openraft.
async fn built(db: &Db, snapshot: Snapshot<TypeConfig>) -> (u64, State) {
    let file = snapshot.snapshot.into_std().await;
    let mut rows = Vec::new();
    read_frames(
        &db.snapshot_sealer,
        GROUP,
        &snapshot.meta.snapshot_id,
        file,
        |k, sealed| {
            let v = db.open_state(GROUP, k, sealed)?;
            rows.push((
                String::from_utf8_lossy(k).into_owned(),
                String::from_utf8_lossy(&v).into_owned(),
            ));
            Ok(())
        },
    )
    .unwrap();
    (snapshot.meta.last_log_id.map_or(0, |l| l.index), rows)
}

/// The id, index and rows of the snapshot a group serves (to peers, to
/// backups).
fn served(db: &Db) -> (String, u64, State) {
    let (index, id, path) = current_snapshot_file(db, GROUP)
        .unwrap()
        .expect("the group serves a snapshot whose file exists");
    let mut rows = Vec::new();
    read_snapshot_file(&KEY, GROUP, &id, &path, |k, v| {
        rows.push((
            String::from_utf8_lossy(k).into_owned(),
            String::from_utf8_lossy(v).into_owned(),
        ));
        Ok(())
    })
    .unwrap();
    (id, index, rows)
}

/// Start a build of `sm`'s group on its own thread, held at `at`, and wait
/// until it is.
async fn held_build(
    sm: &mut StateMachine<Rows>,
    dir: &Path,
    at: BuildPoint,
) -> JoinHandle<Snapshot<TypeConfig>> {
    hold_build(dir, GROUP, at);
    let mut builder = sm.get_snapshot_builder().await;
    let build = std::thread::spawn(move || builder.inner.build().unwrap());
    while !build_waiting(dir, GROUP) {
        assert!(!build.is_finished(), "the build ran past its hold");
        std::thread::sleep(Duration::from_millis(1));
    }
    build
}

/// A leader at index 10 with `x=new, y=new`, a follower at index 2 with
/// `x=old`, and the leader's snapshot received by the follower (not yet
/// installed): its meta.
async fn follower_receiving(
    leader_dir: &Path,
    follower_dir: &Path,
) -> (StateMachine<Rows>, SnapshotMeta) {
    let mut commands = vec!["x=new"];
    commands.resize(10, "y=new");
    let mut leader = machine_with(leader_dir, &commands).await;
    let snapshot = leader.build().unwrap();
    let mut follower = machine_with(follower_dir, &["x=old", "x=old"]).await;
    let received = follower.snapshot_path(&format!("g{GROUP}-recv-test.snap"));
    let sent = leader.snapshot_path(&format!("g{GROUP}-{}.snap", snapshot.meta.snapshot_id));
    let _bytes = std::fs::copy(sent, &received).unwrap();
    follower.receiving = Some(received);
    (follower, snapshot.meta)
}

/// openraft spawns a build, then installs a snapshot on the same worker; the
/// build reaches the applied state only once the install has switched it.
/// What it builds is the installed state at the installed index, not the
/// rows of the generation before (gone by then: an empty snapshot at index
/// 10) labelled with the new index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_built_across_an_install_holds_the_state_at_its_index() {
    let leader_dir = tempfile::tempdir().unwrap();
    let follower_dir = tempfile::tempdir().unwrap();
    let (mut follower, meta) = follower_receiving(leader_dir.path(), follower_dir.path()).await;
    let build = held_build(&mut follower, follower_dir.path(), BuildPoint::Read).await;

    follower.install(&meta).unwrap();
    release_build(follower_dir.path(), GROUP);
    let snapshot = build.join().unwrap();

    let leader_state = new_state(&[("x", "new"), ("y", "new")]);
    assert_eq!(
        built(&follower.db, snapshot).await,
        (10, leader_state.clone()),
        "the build holds the state at the index it names"
    );
    assert_eq!(served(&follower.db), (meta.snapshot_id, 10, leader_state));
}

/// A build that read the follower's own state (index 2) and finishes writing
/// it after an install of index 10 neither replaces the installed snapshot
/// nor deletes its file: the group keeps serving index 10.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_that_ends_after_an_install_never_replaces_the_installed_snapshot() {
    let leader_dir = tempfile::tempdir().unwrap();
    let follower_dir = tempfile::tempdir().unwrap();
    let (mut follower, meta) = follower_receiving(leader_dir.path(), follower_dir.path()).await;
    let build = held_build(&mut follower, follower_dir.path(), BuildPoint::Record).await;

    follower.install(&meta).unwrap();
    release_build(follower_dir.path(), GROUP);
    let snapshot = build.join().unwrap();

    let leader_state = new_state(&[("x", "new"), ("y", "new")]);
    assert_eq!(
        served(&follower.db),
        (meta.snapshot_id, 10, leader_state.clone()),
        "the installed snapshot is still the one served"
    );
    assert_eq!(
        built(&follower.db, snapshot).await,
        (10, leader_state),
        "the build was made again of the installed state"
    );
}

/// A build that read the state before a backup replaced it, and finishes
/// writing it afterwards, is not served: the group serves the replaced state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_that_ends_after_a_replace_never_serves_the_state_before() {
    let backup_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut source = machine_with(backup_dir.path(), &["x=new", "y=new"]).await;
    let backup = source.build().unwrap();
    let backup_path = source.snapshot_path(&format!("g{GROUP}-{}.snap", backup.meta.snapshot_id));
    let mut sm = machine_with(dir.path(), &["x=old", "x=old", "x=old"]).await;
    let build = held_build(&mut sm, dir.path(), BuildPoint::Record).await;

    let rows = replace_state::<Rows>(
        &sm.state,
        &sm.shared,
        GROUP,
        &backup.meta.snapshot_id,
        &backup_path,
    )
    .unwrap();
    assert_eq!(rows, 2);
    release_build(dir.path(), GROUP);
    let snapshot = build.join().unwrap();

    let replaced = new_state(&[("x", "new"), ("y", "new")]);
    let (_, index, served_rows) = served(&sm.db);
    assert_eq!(
        (index, served_rows),
        (3, replaced.clone()),
        "the group serves the state the backup put in place"
    );
    assert_eq!(built(&sm.db, snapshot).await, (3, replaced));
}
