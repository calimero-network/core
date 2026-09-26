# merod - Node Daemon

The Calimero node daemon that orchestrates WASM apps, storage, networking, and RPC.

## Package Identity

- **Binary**: `merod`
- **Entry**: `src/main.rs`
- **Framework**: clap (CLI), tokio (async), actix (actors)

## Commands

```bash
# Build
cargo build -p merod

# Build release
cargo build -p merod --release

# Run
cargo run -p merod -- --node node1 run

# Test
cargo test -p merod
```

## CLI Structure

```
merod [--node <name>] <subcommand>     # --node only where a store is opened
├── account       # Account root: export, import, revoke-proof.
│                 # export/import open the store directly — node must be STOPPED.
│                 # warrant + login-statement never need --node; sign-cert,
│                 # revoke-proof and sign-with-root need none with --from.
├── init          # Initialize node configuration (mints the embedded-auth
│                 # admin root key from --admin-user + password via
│                 # file/stdin/env; --no-admin defers)
├── run           # Start the node daemon (alias: up)
├── config        # Modify node configuration
├── auth          # Embedded-auth accounts (set-admin: offline admin-key mint)
└── kms           # Key management service (probe, disk-key)
```

## Account root: backup, restore, offline revocation (`merod account`)

**`--node` is optional, and which commands need it is the whole rule.** It names
the node home a command opens, so everything that reads a config or a store
requires it — `run`, `init`, `config`, `auth`, `kms`, `tee`, and the account
commands that take their root from the store. The offline signers require
nothing: `account warrant` and `account login-statement` are pure functions of
their flags, and `sign-cert` / `revoke-proof` / `sign-with-root` reach the root
through `--from <PHRASE>` without opening anything. Those five run with no
`--node`, no `--home` and no init — from any directory, on a machine that has
never held a node.

It was required until recently, which made the cold-storage path name a node
that need not exist; a nonexistent name was accepted precisely because nothing
read it. `RootArgs::node_home()` is now the one place `--node` becomes a path, so
a command that needs a node fails on the missing name rather than silently
addressing `$CALIMERO_HOME/` — the parent of every node home — and `open_store`
adds the `--from` hint, since reaching it without `--node` means the caller
wanted the offline path.


The account root is the only key that can certify a replacement device after every
device is lost, so this family of commands is the whole recovery story. `export`
and `import` open the datastore directly, which means the node must be **stopped**
(RocksDB's lock is exclusive) and a KMS-encrypted store is refused rather than
misread; `revoke-proof` needs no store at all when given `--from`.

**Where a root comes from — exactly two places.** `merod init` provisions one, and
`merod account import` restores one. Nothing mints a root lazily: everything that
needs one calls `require_account_root`, which errors when there is none. So a node
holds a root because it was provisioned, because it was imported, or not at all —
and the third state is real rather than quietly repaired.

`merod init --no-account-root` is how you ask for that third state: the root stays
in cold storage and signs certificates, and the node's device is enabled by a
certificate signed elsewhere. Such a node cannot certify a device (its own or
anyone's) and cannot be the holder half of a pairing, so `ensure_enrolled` fails
on it by design. It *can* still name its account, because a certified device row
answers before the root fallback does.

**The signing key is provisioned at init too.** `merod init` mints the keypair the
node signs ops with, not just the account root. That used to happen on first
namespace join, which is invisible for an ordinary node — it self-signs its device
certificate at that same moment — and fatal for a node whose account root lives
elsewhere: the certificate must be signed over that key BEFORE the join, and the key
did not exist until the join it was meant to enable. `participate_in` still mints for
nodes initialised by older binaries, and reuses the provisioned key otherwise.

**On a TEE node the store is encrypted from `init`, or not at all.** `merod init
--kms-url <URL>` fetches the storage key from mero-kms-phala *before* writing the
signing identity and account root, opens the store encrypted, and saves
`[tee.kms.phala]` so `run` fetches the same key (the KMS derives it from the peer
id). Adding `[tee]` to a node that was initialised without it does not work, and not
only because those keys already sit in plaintext. The encrypted store decrypts every
read, so the plaintext rows `init` wrote cannot be read back and `run` fails. The KMS
is verified against the signed release policy, so `--kms-url` requires
`MERO_TEE_VERSION` (or `MERO_KMS_VERSION` / `MERO_KMS_RELEASE_TAG`) and refuses
without it; `run` would accept an unverified KMS when neither the release nor config
allowlists are set, and `init` deliberately does not. The key is fetched before the
`--force` wipe and before anything is written, so a failed fetch leaves the home as
it was.

**Every key the KMS releases is sealed to the TD** (`src/kms/sealed.rs`). TLS ends
wherever `kms-phala-url` points, and that URL is operator-written metadata, so a
plain-hex key was readable by any proxy in front of the genuine KMS. `/attest`
commits to the KMS's X25519 transport key, the get-key quote commits to a one-time
key, and the reply is AES-256-GCM under X25519+HKDF of the two. merod refuses a KMS
with no transport key and refuses an unsealed key. The wire format is repeated in
mero-kms (`mero-kms/src/sealed.rs`); the fixed vectors in both test modules are
what keep the two implementations the same format — change both or neither.

The release policy is read from its `kms_allowed_*` lists (never `node_allowed_*`),
and its `role`, `tag` and (with `MERO_TEE_PROFILE`) `profile` must match what was
asked for. `MERO_TEE_MIN_VERSION` refuses a release older than the floor, so
naming an old but validly signed release is not a downgrade.

**The KMS's compose hash is pinned too** (`src/kms/event_log.rs`). Node keys are
derived from the KMS's dstack *app* key, so anything running under that app can
derive them, and the app owner can upgrade it to another compose file whose
registers may still be allowlisted (mero-tee#338). merod replays the `eventLog`
`/attest` returns, recomputing each RTMR3 event digest from its contents, and
trusts the `compose-hash` event only if the replay reproduces the quote's RTMR3.
That hash must be in the policy's `kms_allowed_event_payload`; a release policy
without one is refused. The config-policy path checks
`tee.kms.phala.attestation.allowed_compose_hashes` when it is set, and warns when
it is not. Mock quotes carry no event log and skip the check.

**A TDX cluster KMS is pinned by its registers instead** (`KmsBackend::Tdx`,
`kms.backend = "tdx"` in the release policy, `tee.kms.phala.attestation.backend`
in config). mero-kms with `MERO_KMS_BACKEND=tdx` runs as a locked GCP TDX image
with no dstack; its RTMR3 is the image's own boot measurement, fixed per image,
so the five registers pin its code and there is no compose file. A `tdx` policy
must not name a compose hash (refused), and a policy without `kms.backend` is a
dstack policy, so files published before the field existed keep their check.

**`merod kms disk-key`** fetches the key that unlocks the image's LUKS2 data disk,
before that disk (and so this node's home) exists. It needs no `--node`. It uses a
dedicated identity (`--identity`, created with `--create-identity`), not the
node's libp2p key, which lives on the disk being unlocked; the identity may sit
where the host can read it, since it gets nothing without a genuine TD. The key is
written only to a new 0600 file on tmpfs/ramfs (`--key-out`), never to stdout,
because merod's logs go to stdout and are shipped off the machine.

```bash
# Print the 24-word phrase to stdout.
merod --node node1 account export

# Write it to a file instead. Refused without the second flag; created 0600.
merod --node node1 account export --out backup.txt --allow-plaintext-file

# Restore. Reads stdin by default, or --from PATH. Refuses to replace an
# existing root without --force.
merod --node node1 account import [--from backup.txt] [--force]
```

Export prints the phrase on the **first line** (so `head -1` is the secret),
then the root's public key, then the account id — the same one in every
namespace, since the id carries no per-namespace nonce. Only the first line is
sensitive.

`--force` **drops the device rows belonging to the root it replaces**, and reports
which namespaces they were in. Not housekeeping: a device row is keyed by namespace
alone and enrolment refuses to replace a *linked* row naming a different account, so
leaving them made the node refuse enrolment under the root it had just recovered —
telling the operator to revoke first, which needs the key they replaced. Rows naming
an account this root never owned (a device paired into somebody else's account) are
kept and reported separately. An import onto an empty store needs no flag and drops
nothing, and neither does re-importing the root that is already installed — the
root write and the row removals are one atomic write, so there is no half-applied
state where a new root sits beside the old root's rows.

`--out` refuses an existing file (`O_CREAT | O_EXCL`): the 0600 mode only applies on
creation, so reusing a path would write the phrase into whatever permissions were
already there, and a pre-planted symlink could redirect it. On a platform with no
mode to set, the command says so instead of claiming owner-only.

### Revoking a device with only the root

```bash
# Sign a device revocation offline. --from reads the phrase, so this needs no
# node, no home and no init — the lost-device case. Prints a hex proof.
# No --namespace: the proof names a device and the account that owns it, and one
# root owns one account everywhere (see below).
merod account revoke-proof --device <DEVICE_ID> --from phrase.txt

# Without --from, the root comes from this node's store (so: stopped), and
# --node is required to say which store.
merod --node node1 account revoke-proof --device <DEVICE_ID>
```

The proof is self-certifying, so any member node can publish it and needs no
authority of its own: `meroctl account revoke <NS> --device-id <D> --proof @file`.
It is not a secret — it authorises exactly one revocation of one device. What it
cannot do is name a device the account does not own (the stored binding is checked
before publishing and again on every replica) or rotate the scope key (admin only).

The proof names a device and the account it belongs to, and one root owns one
account everywhere — so there is no namespace to supply. Publication is still
per-DAG: it takes effect in a group once published there.

Full model, the recovery procedure, and what does *not* come back:
[protocol/accounts](../../docs/src/content/docs/protocol/accounts.mdx#backing-up-and-recovering-an-account).

## File Organization

```
src/
├── main.rs           # Entry point, setup tracing
├── cli.rs            # Root clap command
├── cli/
│   ├── init.rs       # Node initialization
│   ├── run.rs        # Start daemon
│   ├── config.rs     # Config modifications
│   ├── auth.rs       # `merod auth set-admin` (offline admin-key mint)
│   ├── admin_creds.rs# Shared --admin-user/password-file/stdin resolution
│   ├── account.rs    # `merod account export|import|revoke-proof`
│   ├── kms.rs        # KMS subcommand
│   ├── validation.rs # Validation helpers
│   └── auth_mode.rs  # Authentication mode handling
├── defaults.rs       # Default values
├── kms/              # Key management service (sealed release, RTMR3 compose-hash check)
├── kms_policy.rs     # KMS policy
└── version.rs        # Version checking
```

## Patterns

### CLI Command Pattern

- ✅ DO: Follow pattern in `src/cli/init.rs`
- ✅ DO: Use `EyreResult` for error handling
- ❌ DON'T: Use `unwrap()` or `expect()` without safety comment

```rust
// Pattern: src/cli/init.rs
use clap::Parser;
use eyre::Result as EyreResult;

#[derive(Debug, Parser)]
pub struct InitCommand {
    #[clap(long)]
    server_port: Option<u16>,
}

impl InitCommand {
    pub async fn run(self, args: &RootArgs) -> EyreResult<()> {
        // ...
    }
}
```

### Logging Setup

```rust
// src/main.rs pattern
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

// Default: merod=info,calimero_=info
// Override with RUST_LOG env var
```

## Key Files

| File              | Purpose                   |
| ----------------- | ------------------------- |
| `src/main.rs`     | Entry, tracing setup      |
| `src/cli.rs`      | Root command definition   |
| `src/cli/run.rs`  | Main daemon startup logic |
| `src/cli/init.rs` | Node initialization       |
| `src/defaults.rs` | Default ports, paths      |

## JIT Index

```bash
# Find CLI subcommands
rg -n "#\[derive.*Parser\]" src/

# Find default values
rg -n "const " src/defaults.rs

# Find error handling
rg -n "EyreResult" src/
```

## Running

```bash
# Initialize node
merod --node node1 init --server-port 2428 --swarm-port 2528

# Run with debug logging
RUST_LOG=debug merod --node node1 run

# Run with specific crate logging
RUST_LOG=calimero_node=debug,calimero_network=debug merod --node node1 run
```

## Common Gotchas

- Node data stored at `~/.calimero/<node-name>/`
- Config file: `~/.calimero/<node-name>/config.toml`
- Ports must be available before starting
