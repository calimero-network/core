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
- `Group::propose_led` is `propose` that also hands the builder the `Leadership`
  (the term the same leadership check found: the metrics' term when they say this
  node leads, else the Raft core's after a confirmation; no extra round). Once per
  term this node leads in, before its first proposal there, it waits (up to
  `CATCH_UP`, then `Unavailable`) until it has applied its whole log: a leader
  restored into its old term (a restart, no election) can hold entries it
  appended past the commit index it saved, which is not synced. The node's
  component apps take their unique ids (D52, `term.seq.n`) from it.
- Test hooks (feature `testing`, never in a release build):
  `Engine::lose_unsynced_writes(group)` drops a group's commit index and applied
  batches from then on, as a crash before they reach the disk would;
  `testing::hold_apply(dir, group)` / `release_apply` hold every apply of a group
  on the node opened at `dir` (from its next start), so a test decides when a
  restarted leader catches up. Used by the node's
  `unique_ids_survive_a_crash_before_the_commit_index_reaches_the_disk`;
  `testing::slow_close(dir, delay)` makes closing that node's store wait first,
  as a stalled last holder would (`tests/store_close.rs`).
- `Engine::store_watch` reports the store open until RocksDB has closed and
  released its lock (`Db::closed`, the last field), not merely until the last
  `Arc<Db>` starts dropping.
- Storage (`store.rs`): one RocksDB per node (`<data-dir>/store`), opened with
  `tuning`'s column and table options (one bounded block cache,
  `BLOCK_CACHE_BYTES`). Column families `log`, `meta`, `state`, `blobs`. Every value
  is sealed with AES-256-GCM under the storage key, bound to its column, group and
  key. A machine can hide a key (`StateTx::put_hidden`): the row is keyed by an HMAC
  and the key travels inside the sealed value. Log appends, votes and blobs are
  synced writes; RocksDB shares one WAL sync between concurrent groups. The
  commit index (`save_committed`) is not synced: a node that loses it, or keeps
  an older one, learns it again and applies each entry once (disk and
  in-memory machines: `a_lost_or_older_commit_index_is_learned_again`,
  `an_in_memory_machine_applies_each_entry_once_after_losing_its_commit_index`).
- State machines (`machine.rs`): a `Machine` writes through `StateTx` in `apply`;
  each applied batch commits its writes and the applied index in one batch, so a
  restart resumes instead of replaying. `Machine::IN_MEMORY` machines (the system
  group) keep state in memory and are checkpointed with each snapshot instead.
- Snapshots: files of sealed frames under `<data-dir>/snapshots`, streamed from a
  RocksDB snapshot (never the whole state in memory), sent in `SNAPSHOT_CHUNK_BYTES`
  chunks. Every node of a cluster shares K_r, so a follower opens a leader's file.
  Installing writes the next state generation (`group ‖ generation ‖ key`) and
  switches in one batch: a crash mid-install keeps the old state.
- Migration (`legacy.rs`): a data dir holding the previous redb file (`log.redb`) is
  migrated on `Engine::open` (entries, vote, commit and purge marks, blobs; the old
  whole-state snapshot is imported by the group's machine via `Machine::import`) and
  the file renamed `log.redb.migrated`.
- Entries are capped at `MAX_ENTRY_BYTES` (1 MiB; `WriteError::TooLarge`): openraft
  gives each append RPC one heartbeat interval, so a huge entry would never reach a
  follower. Large content goes to the sealed blob store (`Engine::put_blob`/`get_blob`).
- Network: `Transport` trait; the node supplies an attested channel, tests an in-memory one.
  openraft takes the sender an RPC names (`vote.leader_id`) on trust and a
  follower accepts an append from any node with a higher term:
  `RpcRequest::sender` names it so the node checks it against the identified
  caller and the group's members before `handle_rpc`.

Tests: `cargo test -p calimero-log` runs a 3-node cluster in one process
(replication, failover, restart from disk, a lost or older commit index, learner catch-up, no plaintext on disk,
a disk machine restarting without replay, a ~6 MiB state streamed to a new node) and
the redb migration (`migration_tests.rs`).

Bench: `cargo bench -p calimero-log --bench log` (`benches/log.rs`): a proposal
appended, committed and applied on one group, on 1 node and on a 3-node group
over the in-memory transport, 1 and 64 in flight (64 only queue behind the
propose lock; an append takes no leadership round); and one RocksDB write synced and unsynced on the same disk, the
unit a commit's latency is made of.
