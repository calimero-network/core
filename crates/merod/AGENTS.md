# merod

The Calimero node binary: v1's command line over the node
(`calimero-node`, which holds everything the node does). This crate only
turns a configuration into `calimero_node::Config` and calls `run`.

- `merod --node NAME init FLAGS` writes `{home}/{NAME}/config.toml`
  (`--home`/`CALIMERO_HOME`, default `~/.calimero`; mode 0600, it may hold the
  ops token); a second `init` needs `--force`.
- `merod --node NAME config FLAGS` changes it; `merod --node NAME run [FLAGS]`
  starts from it, flags and `MEROD_*` variables overriding it for that run. A
  container may `run` with flags only (no `--node`, no file).
- One struct (`node_config::NodeArgs`) is both the flags and the file: every
  field optional, `over` merges, kebab-case keys, unknown keys refused, the
  `[outbound]` table is the outbound policy (`--settings FILE` replaces it).
  Defaults are applied in `start`, not in the struct, so a file never records
  a default as if chosen.
- `merod local` (a development cluster in process), `merod inspect` (a stopped
  node's store, `calimero_node::inspect`), `merod kms disk-key` (for a
  TEE boot script, `calimero-kms-client`).

```bash
cargo test -p merod          # NodeArgs merging; init/config/run/inspect through the binary
cargo run -p merod -- --help
cargo run -p merod -- local
```
