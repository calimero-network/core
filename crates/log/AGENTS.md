# calimero-log

Replicated log for the cloud-only (release-v2) architecture: one Raft group per
app instance plus a system group per cluster, on [openraft](https://docs.rs/openraft) 0.9.

- `Engine` hosts every group of one node; `Group<M>` is one group with state machine `M: Machine`.
- `Group::propose` serializes proposals per group and confirms leadership, so a
  command built from the applied state applies to exactly that state.
- Storage (`store.rs`): one RocksDB per node (`<data-dir>/store`), opened with
  `calimero-store-rocksdb`'s column and table options (one bounded block cache,
  `BLOCK_CACHE_BYTES`). Column families `log`, `meta`, `state`, `blobs`. Every value
  is sealed with AES-256-GCM under the storage key, bound to its column, group and
  key. A machine can hide a key (`StateTx::put_hidden`): the row is keyed by an HMAC
  and the key travels inside the sealed value. Log appends, votes and blobs are
  synced writes; RocksDB shares one WAL sync between concurrent groups.
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

Tests: `cargo test -p calimero-log` runs a 3-node cluster in one process
(replication, failover, restart from disk, learner catch-up, no plaintext on disk,
a disk machine restarting without replay, a ~6 MiB state streamed to a new node) and
the redb migration (`migration_tests.rs`).
