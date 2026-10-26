# calimero-log

Replicated log for the cloud-only (release-v2) architecture: one Raft group per
app instance plus a system group per cluster, on [openraft](https://docs.rs/openraft) 0.9.

- `Engine` hosts every group of one node; `Group<M>` is one group with state machine `M: Machine`.
- `Group::propose` serializes proposals per group and confirms leadership, so a
  command built from the applied state applies to exactly that state.
- Storage: one redb file per node; every value sealed with AES-256-GCM under the node's storage key (`store.rs`).
- Entries are capped at `MAX_ENTRY_BYTES` (1 MiB; `WriteError::TooLarge`): openraft
  gives each append RPC one heartbeat interval, so a huge entry would never reach a
  follower. Large content goes to the sealed blob store (`Engine::put_blob`/`get_blob`).
- Network: `Transport` trait; the node supplies an attested channel, tests an in-memory one.
- State machines live in memory and are rebuilt from the last snapshot plus the log on restart.

Tests: `cargo test -p calimero-log` runs a 3-node cluster in one process
(replication, failover, restart from disk, learner catch-up, no plaintext on disk).
