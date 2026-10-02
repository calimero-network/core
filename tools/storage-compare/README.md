# storage-compare

Measures the same workloads against whichever `calimero-storage` it is built
in, so that two commits can be compared directly:

- **State**: rows, logical key+value bytes, and compacted RocksDB bytes per kv-store
  entry (`UnorderedMap<String, LwwRegister<String>>`) and per chat message
  (`AuthoredVector<Message>`). The on-disk figures use 4 contexts under the node's
  `context_id ‖ state key` layout, once uncompressed and once with the node's own `State`
  options (`src/config.rs`).
- **Calls**: one commit per call, 500 calls after a prefill of 1,000. For each call it
  records the delta artifact's bytes and its wall time against an in-memory store.

- **Delta rows**: the node's `Column::Delta` row (`calimero_store::types::ContextDagDelta`) for 10,000
  calls of each workload, built the way `execute` persists a self-authored delta (real actions and
  HLC, one parent, author, one-head governance position, signature). Bytes per field in the borsh
  layout rows had before and in the current one, and compacted RocksDB bytes for the 10,000 rows
  under one context, uncompressed and with the node's options (`src/delta_row.rs`).

```bash
cargo run -p storage-compare --release
cargo run -p storage-compare --release -- --deltas   # the delta-row section only
```

The delta-row section needs `ContextDagDelta::to_row_bytes`, so it builds only at commits that have it;
the old layout it compares against is written by the probe itself.

To measure an older commit, check it out in a worktree. Copy this directory into that
worktree and add it to the workspace `members`. Then replace `src/config.rs` with
whatever options that commit's node gave the `State` family. Before #4200 that was
`Options::default()`, with no compression codec compiled in.

The results from 2026-10-02, before #4200 compared with `master`, are in
[`RESULTS.md`](RESULTS.md).
