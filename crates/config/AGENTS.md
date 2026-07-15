# calimero-config - Node Configuration Model

Defines `ConfigFile`, the typed schema for a merod node's `config.toml`, plus atomic load/save.

## Package Identity

- **Crate**: `calimero-config`
- **Entry**: `src/lib.rs` (single file, no submodules)
- **Key deps**: `toml` + `serde` (parsing), `camino` (UTF-8 paths), `tempfile` (atomic write), `tokio` (`fs`, `rt` - async load/save), `libp2p-identity` (node keypair), `multiaddr`, `url`, `bs58`; workspace types from `calimero-context`, `calimero-network-primitives`, `calimero-node-primitives`, `calimero-runtime`, `calimero-server`, `mero-auth`

## Commands

```bash
# Build
cargo build -p calimero-config

# Test (all)
cargo test -p calimero-config

# Test a single case
cargo test -p calimero-config write_atomic_creates_file_mode_0600 -- --nocapture
```

## File Location and Format

- File name: `CONFIG_FILE = "config.toml"` (constant in `src/lib.rs`).
- Full path is `<node_dir>/config.toml`, where `<node_dir>` is chosen by the caller (this crate never hardcodes it). merod resolves `<node_dir>` as `--home/<node_name>`, and `--home` defaults to `~/.calimero` (`defaults::default_node_dir()` in `crates/merod/src/defaults.rs`, overridable via `CALIMERO_HOME`).
- Format is TOML, (de)serialized with `serde` + the `toml` crate.
- `ConfigFile` is `#[non_exhaustive]` at every level (`ConfigFile`, `TeeConfig`, `KmsConfig`, `PhalaKmsConfig`, `SyncConfig`, `SpecializedNodeConfig`, `NetworkConfig`, `ServerConfig`, `DataStoreConfig`, `BlobStoreConfig`) - external crates cannot build these with a struct literal; use the provided `::new`/`::with_*` constructors or field access, so adding a field here never breaks downstream compilation.

## Config Field Inventory (`ConfigFile`)

| Field | TOML section | Type | Default when absent |
| --- | --- | --- | --- |
| `identity` | `[identity]` | `IdentityConfig` (`peer_id` + base58 protobuf `keypair`) | generates a fresh Ed25519 libp2p keypair |
| `mode` | `mode = ...` | `NodeMode` (re-exported from `calimero-node-primitives`) | `NodeMode::default()` |
| `network` | flattened (`[swarm]`, `[server]`, `[bootstrap]`, `[discovery]`, `[server.specialized_node]`) | `NetworkConfig` | no default - required unless every nested field has one |
| `sync` | `[sync]` | `SyncConfig` | required (no `#[serde(default)]` on the field itself) |
| `datastore` | `[datastore]` | `DataStoreConfig` (just `path: Utf8PathBuf`) | required |
| `blobstore` | `[blobstore]` | `BlobStoreConfig` (just `path: Utf8PathBuf`) | required |
| `context` | `[context]` | `ContextConfig` (from `calimero-context`) | required |
| `runtime` | `[runtime]` | `RuntimeConfig` (from `calimero-runtime`) | `RuntimeConfig::default()` |
| `tee` | `[tee]` | `Option<TeeConfig>` | `None` |
| `dag_compaction` | `[dag_compaction]` | `DagCompactionConfig` (re-exported from `calimero-node-primitives`) | `DagCompactionConfig::default()` |

Nested structs of note:

| Struct | Fields | Notes |
| --- | --- | --- |
| `NetworkConfig` | `swarm`, `server`, `bootstrap` (default), `discovery` (default), `specialized_node` (default) | flattened into the top-level TOML, not a `[network]` section |
| `ServerConfig` | `listen: Vec<Multiaddr>`, `admin`/`jsonrpc`/`websocket`/`sse: Option<..>`, `auth_mode: AuthMode` (default `Proxy`), `embedded_auth: Option<AuthConfig>` | |
| `SyncConfig` | `timeout` (`timeout_ms`), `session_deadline` (`session_deadline_ms`, defaults to 30s), `interval` (`interval_ms`), `frequency` (`frequency_ms`) | all four are millisecond integers on the wire via `serde_duration` |
| `SpecializedNodeConfig` | `invite_topic` (default `"mero_specialized_node_invites"`), `accept_mock_tee` (default `false`) | read-only-node support |
| `TeeConfig` | `kms: KmsConfig` | |
| `KmsConfig` | `phala: Option<PhalaKmsConfig>` | only Phala is supported today |
| `PhalaKmsConfig` | `url: Url`, `tls: KmsTlsConfig` (default), `attestation: KmsAttestationConfig` (default) | |
| `KmsTlsConfig` | `ca_cert_path`, `client_cert_path`, `client_key_path: Option<Utf8PathBuf>` | mTLS needs cert+key together, not enforced by this struct |
| `KmsAttestationConfig` | `enabled`, `accept_mock` (both default `false`), `allowed_tcb_statuses` (default `["UpToDate"]`), `allowed_mrtd`, `allowed_rtmr0..3` (default empty), `binding_b64`, `policy_json_path` | see `validate_enabled_policy()` below |

## Mental Model

`ConfigFile::load`/`save` are thin TOML (de)serialization wrappers; the interesting logic is elsewhere in the file:

- `IdentityConfig` is not `Serialize`/`Deserialize` directly - it round-trips through the `serde_identity` module, which writes `peer_id` (base58 `PeerId`) and `keypair` (base58 protobuf-encoded `Keypair`) as a two-key map, and on read cross-checks that the derived `peer_id` matches the decoded keypair's public key, rejecting a tampered/mismatched pair at parse time.
- `write_atomic` (used by `ConfigFile::save`) writes to a `NamedTempFile` in the same directory (chmod 0600 on Unix), fsyncs the file, renames it over the target, then fsyncs the containing directory - because `config.toml` holds the node's private key, a torn write is unacceptable data loss, not just corruption.
- `KmsAttestationConfig::validate_enabled_policy()` and `TeeConfig::has_real_attestation()` encode the same production-safety rule from two angles: attestation `enabled=true` with `accept_mock=false` requires non-empty TCB status/MRTD/RTMR0-3 allowlists (checked at config-validation time), and `has_real_attestation()` destructures `KmsConfig` field-by-field (no `..` rest pattern) so a new KMS provider fails to compile here until folded into the predicate - this is the gate merod uses to refuse `--mock-tee` when real attestation is configured.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Everything: all config structs, `serde_duration`/`serde_identity` (de)serialization modules, `write_atomic`, and unit tests |

## Relationship to merod

`calimero-config` only models and (de)serializes the schema; it has no notion of a default home directory or CLI flags. `crates/merod` owns that:

- `crates/merod/src/defaults.rs` resolves the node home (`--home`, env `CALIMERO_HOME`, else `~/.calimero`).
- `crates/merod/src/cli/init.rs` builds the path (`home.join(node_name)`), calls `ConfigFile::exists`/`::load`/`::new` to create or load a node's config.
- `crates/merod/src/cli/run.rs` loads the config at startup and calls `TeeConfig::has_real_attestation` to gate the `--mock-tee` flag.
- `crates/merod/src/cli/config.rs` and `crates/merod/src/cli/kms.rs` mutate and re-save an existing config (`merod config key=value`, KMS subcommands).
- `crates/meroctl` also depends on this crate (`crates/meroctl/src/common.rs`) for the same `ConfigFile` schema when acting on a node's data directory.

## Invariants and Gotchas

- **Never build these structs with a bare literal from outside the crate** - they are `#[non_exhaustive]`; use `ConfigFile::new`, `NetworkConfig::new`/`with_specialized_node`, `ServerConfig::new`/`with_auth`, `SyncConfig::new`, `DataStoreConfig::new`, `BlobStoreConfig::new`, or field access on an existing value.
- **Duration fields are milliseconds on the wire**: `SyncConfig`'s four `Duration` fields serialize/deserialize as bare `u64` millis via the private `serde_duration` module (`timeout_ms`, `session_deadline_ms`, `interval_ms`, `frequency_ms`) - do not add a human-readable duration format without also handling old integer configs.
- **`session_deadline` defaults independently of `timeout`**: if absent from TOML it defaults to a hardcoded 30s (`default_sync_session_deadline`), not to whatever `timeout_ms` was set to - the two can silently diverge in older configs that only set `timeout_ms`.
- **Identity round-trip is self-verifying**: `serde_identity::deserialize` rejects a config where the stored `peer_id` doesn't match the decoded `keypair`'s public key. Hand-editing `config.toml`'s `[identity]` section (e.g. swapping just `peer_id`) will fail to load, by design.
- **`write_atomic` is the only sanctioned way to persist `config.toml`**: it exists specifically to avoid a truncated file destroying the node's only copy of its private key; don't replace `ConfigFile::save`'s use of it with a plain `fs::write`.
- **`KmsAttestationConfig::validate_enabled_policy` is not called automatically by `load`/`save`** - callers (merod's init/run/config paths) must invoke it explicitly after loading if they want to enforce the production-attestation policy; a config that fails this check still parses and loads fine.
- **`TeeConfig::has_real_attestation` destructures `KmsConfig` without `..`** on purpose - adding a new KMS provider field to `KmsConfig` will fail to compile in this function until the new provider is accounted for in the mock-tee guard.

Part of [crates/](../AGENTS.md).
