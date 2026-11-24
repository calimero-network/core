# calimero-log

Replicated log for the node: one Raft group per
app instance plus a system group per cluster, on [openraft](https://docs.rs/openraft) 0.9.

- `Engine` hosts every group of one node; `Group<M>` is one group with state machine `M: Machine`.
- `Group::propose` serializes proposals per group and builds on the applied state
  without a majority round first (its metrics say it leads, or openraft's core
  refuses a follower at once). The machine must refuse at apply a command built
  on other state (the node's `AppMachine` records its own last entry as
  `base_index`, checks it at apply and answers `conflict`; the builder's
  `u64` is the log's applied index, which counts blank and membership entries), so a deposed leader's stale build commits as a no-op or not at all. A `Reply` is a read: it is confirmed with
  `ensure_linearizable` and built again if entries committed meanwhile, so the
  builder may run twice (`FnMut`). Regression:
  `a_stale_build_by_a_deposed_leader_never_applies`.
- Every leadership confirmation (`confirm_leader`: `read_strong`, a `Reply`, a build's term)
  retries a round that missed a majority once per heartbeat, up to `election_max_ms`: one
  slow heartbeat round (each waits one `heartbeat_ms`) is not a lost leader. Regression:
  `a_strong_read_outlasts_a_heartbeat_round_its_followers_answer_late`.
- `Group::propose_led` is `propose` that also hands the builder the `Leadership`
  (the term the same leadership check found: the metrics' term when they say this
  node leads, else the Raft core's after a confirmation; no extra round). Once per
  term this node leads in, before its first proposal there, it waits (up to
  `CATCH_UP`, then `Unavailable`) until it has applied its whole log: a leader
  restored into its old term (a restart, no election) can hold entries it
  appended past the commit index it saved, which is not synced. The node's
  component apps take their unique ids (D52, `term.seq.n`) from it.
- A stopped Raft core restarts (`supervise`, one task per group, ended with it):
  openraft stops a group's core for good on a storage error, a failed apply or a
  panic, so as it stops `StateMachine::apply` drops what it applied in memory and
  reads the state and applied index back from the disk (`machine::reload`; the
  machine's `restore` rereads everything, an in-memory one from its checkpoint, or
  from nothing when `data` is empty), and a panic in `Machine::apply` is caught and
  fails the batch like a refused write. After `RESTART_FIRST` (1 s, doubled per stop
  within `RESTART_MAX`, 5 min, ±25 % jitter) `Group::restart` starts a new core on
  the same `Shared`, `GroupState` and applied watch, so readers and subscribers keep
  their handles; `Group::raft()` answers the current core. `Group::stopped`,
  `Status::stopped`/`restarts` (not serialized). The engine's `shutdown` and
  `remove_group` close a group first, so it is never restarted after. Regressions
  (`tests/cluster.rs`): `a_batch_that_fails_to_commit_stops_the_group_which_reloads_and_restarts`,
  `a_machine_that_panics_stops_the_group_which_reloads_and_restarts`,
  `a_fault_that_outlasts_a_restart_backs_off_and_shows_in_the_status`.
- Test hooks (feature `testing`, never in a release build):
  `testing::fail_applies(dir, group, n)` makes the next `n` applied batches of a
  group fail to commit, as a disk answering EIO would;
  `Engine::lose_unsynced_writes(group)` drops a group's commit index and applied
  batches from then on, as a crash before they reach the disk would;
  `testing::hold_apply(dir, group)` / `release_apply` hold every apply of a group
  on the node opened at `dir` (from its next start), so a test decides when a
  restarted leader catches up. Used by the node's
  `unique_ids_survive_a_crash_before_the_commit_index_reaches_the_disk`;
  `testing::slow_close(dir, delay)` makes closing that node's store wait first,
  as a stalled last holder would (`tests/store_close.rs`);
  `testing::hold_build(dir, group, BuildPoint)` / `build_waiting` /
  `release_build` hold a snapshot build before it reads the applied state or
  before it records its file (`machine::build_races`);
  `testing::slow_synced_writes(dir, group, delay)` / `heal_synced_writes` make
  a group's synced writes (appends, votes, purges) block for `delay` longer
  holding the node's write path, as a slow fsync holds RocksDB's write group
  (every other write of that node waits meanwhile; a heal lets one waiting now
  go on), and `synced_write_waiting` names the write waiting there, so a test
  holds one and checks what runs meanwhile (`tests/slow_disk.rs`).
- Disk work never runs on an async worker: every group of a node shares them, so
  a slow fsync or a long apply there would hold up the other groups' heartbeats
  and start elections across the cluster. `RaftLogStorage`'s append, `save_vote`,
  `truncate`, `purge` and `save_committed`, and the state machine's `apply` (the
  machine, its commit, the `shared` write lock that waits for a proposal's build),
  snapshot build and install, `applied_state`, `StateMachine::open` in
  `start_group` and a restart's `reload` run on tokio's blocking pool
  (`store::off_workers`); each is still awaited, so an append is durable before
  its callback, a vote before the reply, and the commit index saved before the
  entries apply. An unsynced write waits for the disk too: RocksDB writes in
  groups, and one that arrives while another group's synced write is in its
  fsync joins or queues behind it (`save_committed` left inline stalled CI's
  one worker 357 ms). Log reads and the small snapshot file opens stay inline
  (a read takes no part in the write path). Regression:
  `a_slow_disk_under_one_group_does_not_stall_the_others` (one worker thread; a
  group's synced writes slowed 250 ms stalled it 750 ms before; then the healthy
  group's commit reaches a node while a synced write of the slow group holds that
  node's write path, and the worker must run while the same write still waits:
  told by order, not latency, since a loaded runner's own fsync can take 300 ms).
- A read of the applied state waits off the async workers too (`machine::read_applied`:
  `Group::read_local`, a proposal's build, `digests`). An apply holds the state's write
  lock while its batch commits, so readers never see a state the disk does not hold,
  and that commit queues behind any other write's fsync; a reader that waited for the
  lock on a worker stopped it for as long as the disk took (the node's `Node::sys` is
  `read_local`, so one handler or background task did). A free lock is taken at once,
  a held one is waited for through `store::wait_off_workers` (`block_in_place` on a
  multi-threaded runtime, inline elsewhere). `testing::writes_queued(dir)` counts the
  writes waiting behind a held synced write. Regression:
  `a_state_read_waiting_for_an_apply_behind_the_disk_leaves_the_worker_free`
  (`tests/slow_disk.rs`, one worker: it ticked 0 times in 300 ms before, as CI's node
  `a_blob_put_waiting_for_the_disk_leaves_the_async_worker_free` once did).
- `Engine::store_watch` reports the store open until RocksDB has closed and
  released its lock (`Db::closed`, the last field), not merely until the last
  `Arc<Db>` starts dropping.
- Storage (`store.rs`): one RocksDB per node (`<data-dir>/store`), opened with
  `tuning`'s column and table options (one bounded block cache,
  `BLOCK_CACHE_BYTES`). Column families `log`, `meta`, `state`, `blobs`. Rows are
  stored as they are (store format 2): the data directory is on the data disk the TD
  encrypts itself (mero-tee's LUKS2 volume with integrity, keyed by mero-kms), which
  merod requires of a node whose storage key comes from mero-kms (`--kms-disk-evidence`,
  else a data directory on tmpfs). Sealed with AES-256-GCM under the storage key, bound
  to column, group and key, are only the node's own rows (`NODE_ROWS`: format, rollback
  counter; a store opened under another key is refused by them) and a snapshot file's
  rows (`Db::seal_state` as it is built, `open_state` as it is installed), so snapshots
  keep the shape every release reads. A machine can hide a key (`StateTx::put_hidden`):
  the row is keyed by an HMAC and the key travels inside the value. Log appends, votes and blobs are
  synced writes; RocksDB shares one WAL sync between concurrent groups. The
  commit index (`save_committed`) is not synced: a node that loses it, or keeps
  an older one, learns it again and applies each entry once (disk and
  in-memory machines: `a_lost_or_older_commit_index_is_learned_again`,
  `an_in_memory_machine_applies_each_entry_once_after_losing_its_commit_index`).
- State machines (`machine.rs`): a `Machine` writes through `StateTx` in `apply`;
  each applied batch commits its writes and the applied index in one batch, so a
  restart resumes instead of replaying. A machine that cannot apply an entry (one
  it cannot decode, a row it read but cannot decode) calls `StateTx::fail`, and a
  `StateTx` read that fails (a storage fault, a malformed hidden row) fails the batch
  itself and reads as absent: no later entry of the batch is applied,
  the applied index stays where it was, and the batch's error stops the group's
  Raft core there (stop rather than diverge); restarted, it stops there again, with backoff. Regression:
  `a_failed_batch_stops_that_nodes_group_on_the_entry`,
  `a_row_a_replica_cannot_read_stops_it_rather_than_applying_differently` (B3; the
  `testing::fail_state_reads` hook makes a node's reads of a group fail).
  `GroupState::get` (views, routes) still reads a fault as absent and logs it; a
  machine's `restore` reads with `GroupState::try_get`, and `scan_hidden` fails on a
  malformed row. `Machine::IN_MEMORY` machines (the system
  group) keep state in memory and are checkpointed with each snapshot instead.
  `Machine::snapshot` answers a `Result`: a state that does not serialize fails the
  build (logged with the group) and keeps the last checkpoint, never an empty one that
  a receiving node fails to restore. Regression:
  `a_state_that_does_not_serialize_builds_no_snapshot_and_keeps_the_checkpoint`.
- Snapshots: files of sealed frames under `<data-dir>/snapshots`, streamed from a
  RocksDB snapshot (never the whole state in memory), sent in `SNAPSHOT_CHUNK_BYTES`
  chunks. Every node of a cluster shares K_r, so a follower opens a leader's file.
  Installing writes the next state generation (`group ‖ generation ‖ key`) and
  switches in one batch: a crash mid-install keeps the old state. openraft runs
  a build beside installs: a build reads its generation, index and view under
  the `shared` read lock, and records its file under `GroupState`'s
  snapshot-record lock only if the generation is unchanged (an install or
  `replace_state*` switches and records under that lock); otherwise it builds
  again, never failing (a failed build is fatal to openraft's core), and a
  record no newer than the served one is never kept. Lock order: the
  snapshot-record lock, then `shared`. Regressions: `machine::build_races`.
  openraft does not wait for a build when a core shuts down, so
  `Engine::remove_group` drops a group's data under both locks and marks its
  `GroupState` removed (`Group::drop_data`); a build still running then fails
  (`refuse_if_removed`) and writes no checkpoint, record or file for it.
  `Engine::open` deletes every `g<group>-*.snap` file no group's `snapshot`
  record names (a receive or build a crash cut short, a file a crash kept from
  being deleted, a removed group's), before any group starts
  (`machine::clean_snapshot_dir`). Regressions: `tests/snapshot_files.rs`.
  A group builds a full snapshot every `Config::snapshot_every` (1,000) applied
  entries whatever the state's size: openraft 0.9's `SnapshotPolicy` knows only
  `LogsSinceLast(n)` and `Never`, so a size-aware policy would mean `Never` and
  triggering builds from the engine (not done).
- State digests (`digest.rs`): a disk machine's state has a digest, the sum mod 2^256 of
  `SHA-256(label ‖ len(key) ‖ key ‖ value)` over its rows (`StateDigest`), so it depends on
  the rows alone. `StateTx::put`/`delete` move it by the row written less the row replaced
  (`StateTx::replaced`, a failed read fails the batch); `apply` records it after every entry
  at a multiple of `DIGEST_EVERY` (`Applied::recorded`, the last `DIGEST_KEPT`) and commits it
  with the applied index (meta row `digest`, `DigestAt`). `StateMachine::open` takes it again
  from the rows when the stored one is not at the applied index (`StateTx::commit` drops it);
  installs and `replace_state*` sum the rows they write. `Group::digests` answers them (none
  for an in-memory machine) and `Digests::compare` finds the latest index two replicas share.
  The node compares them across replicas. Tests: `digest::tests`,
  `replicas_that_applied_the_same_entries_hold_the_same_digest`,
  `a_replica_that_applied_differently_is_told_apart`,
  `a_row_changed_under_a_replica_shows_once_an_entry_rewrites_it`.
- Node rows: `Engine::put_node_meta`/`get_node_meta` keep rows of the node's own in
  `meta` under group id `NODE_ROWS` (`u64::MAX`, which no group takes), sealed and
  synced like a vote: the node's rollback counter lives in the same store as the
  data, so a copy of the store carries the counter it had.
- Format versions (`format.rs`, `machine::SNAPSHOT_FORMAT`): the store keeps its format in
  the sealed node row `format` (`STORE_FORMAT`, 2). `Engine::open` settles it before anything
  else: an empty one is marked current, any other format is refused.
  This release reads format 2 only, and refuses any other (`OtherStore`, the node does not
  start); there is no migration step, and no reader of the redb store before RocksDB. A
  rollout never meets an older store: a new release's nodes start on empty data directories
  (each release has its own K_r) and receive their data by snapshot, whose format is
  unchanged; only a node restarted in place on an older directory (development) is refused. Tests: `format::tests`
  (`any_other_store_format_is_refused_and_left_as_it_was`).
  A snapshot file's first line is `CALIMERO-SNAPSHOT-<n>`; a later format than
  `SNAPSHOT_FORMAT` (2) is refused by it, not by its frames. Format 2 lets a row span frames
  (pieces of `FRAME_BYTES`, the value length's top bit marking "continued"): a reader refuses
  a frame over `MAX_FRAME_BYTES` (64 MiB), and an in-memory machine's checkpoint (the whole
  system state) is one row. A file with no such row is written as format 1, which the
  release before reads, so a rollout needs no gate; the first line is patched to 2 only when
  a row was split. Regression: `machine::large_rows`. A change to any row's shape raises `STORE_FORMAT` (and
  `SNAPSHOT_FORMAT`, since snapshots carry rows) and appends its step. Tests:
  `format::tests`, `machine::snapshot_format`.
- Entries are capped at `MAX_ENTRY_BYTES` (1 MiB; `WriteError::TooLarge`): openraft
  gives each append RPC one heartbeat interval, so a huge entry would never reach a
  follower. Large content goes to the sealed blob store (`Engine::put_blob`/`get_blob`).
- Heartbeats batched per peer (B6; `network::Batcher`), while
  `Engine::set_batch_heartbeats(true)` (the node turns it on once the cluster version
  brings `batchedHeartbeats`; off is the release before's behaviour): one engine ticker
  (`heartbeat_ticker`, every `heartbeat_ms`) turns openraft's own heartbeat timer off in
  each group (`RpcTarget::tick`, also on start and restart) and triggers every group's
  heartbeats at once; each empty append to a peer then waits up to
  `HEARTBEAT_BATCH_WINDOW` (5 ms) and goes in one `Transport::send_batch` of at most
  `MAX_HEARTBEATS_PER_BATCH` (1024). A batch not answered as a whole (unreachable,
  refused, the wrong number of answers) is sent again item by item, so each group gets
  its own answer. `Engine::handle_rpc_batch` takes a batch whatever the switch. Cadence:
  openraft's timer fires every 300 ms (it checks `heartbeat_ms` only on its tick,
  1.5 × `heartbeat_ms`); the ticker every 200 ms, against `election_min_ms` 1200: more
  often, never less. Regression (`tests/heartbeats.rs`):
  `a_mixed_cluster_keeps_its_leaders_through_the_batching_rollout` (a node refusing
  batches, batching on node by node, a peer refusing them again, off again: no group
  changes leader or term; without the item-by-item fallback it fails).
- Network: `Transport` trait; the node supplies an attested channel, tests an in-memory one.
  openraft takes the sender an RPC names (`vote.leader_id`) on trust and a
  follower accepts an append from any node with a higher term:
  `RpcRequest::sender` names it so the node checks it against the identified
  caller and the group's members before `handle_rpc`.

Tests: `cargo test -p calimero-log` runs a 3-node cluster in one process
(replication, failover, restart from disk, a lost or older commit index, learner catch-up, no plaintext in snapshot files,
a disk machine restarting without replay, a ~6 MiB state streamed to a new node),
the store format (`format::tests`), and a seeded fault simulator (`tests/sim.rs`:
writes, crashes losing unsynced writes, restarts, partitions, dropped and delayed
messages, snapshot builds and log purges; then no acknowledged write lost, strong
reads see them, every replica's digest equal at one index). One test per seed in
`ci_seeds!`; `SIM_SEED`, `SIM_RUNS` and `SIM_TRACE` replay, widen and print
(docs: contribute/testing, "Seeded faults").

Bench: `cargo bench -p calimero-log --bench log` (`benches/log.rs`): a proposal
appended, committed and applied on one group, on 1 node and on a 3-node group
over the in-memory transport, 1 and 64 in flight (64 only queue behind the
propose lock; an append takes no leadership round); 64 `Group::write`s in flight
(the system group's path, straight to openraft: openraft 0.9 appends each
`client_write` as its own synced RocksDB write on the leader's core task, so
these too commit no faster than one sync each); and one RocksDB write synced and unsynced on the same disk, the
unit a commit's latency is made of.

Bench: `cargo bench -p calimero-log --bench heartbeats` (`benches/heartbeats.rs`,
B6): 1, 100 and 1,000 idle groups (`HEARTBEAT_GROUPS`) on 3 engines, all led by
node 1, on `Config::default()`, heartbeats per group and then batched; it prints the
messages node 1 sends one follower per second and per heartbeat interval, the
heartbeats each carries, and the process CPU per Raft message. Per group a peer gets
about one message per shared group per 300 ms (671 per 200 ms at 1,000 groups);
batched, about one per 200 ms tick (1.6 at 1,000 groups, ~610 heartbeats each).
Routing a message is a hash lookup here (`Engine::handle_rpc`) and in the node
(`SystemMachine::app_of_group`, #5163), so the CPU per message stays flat with the
groups.
