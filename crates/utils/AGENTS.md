# calimero-utils-fs - Owner-Only Secret Files

`crates/utils` is a grouping directory with a single sub-crate, `crates/utils/fs` (package `calimero-utils-fs`): writing a secret to disk so only its owner can read it, on Unix and on Windows. The actix helpers that used to live here (`calimero-utils-actix`) are gone with the actor-based node.

## Package Identity

- **Crate**: `calimero-utils-fs`
- **Entry**: `crates/utils/fs/src/lib.rs` (one file, its tests inline)
- **Deps**: none on Unix; `windows-sys` (`Win32_Security*`, `Win32_System_*`) on Windows; `tempfile` for tests

## Commands

```bash
cargo build -p calimero-utils-fs
cargo test -p calimero-utils-fs
```

## Functions

| Function | Purpose |
| --- | --- |
| `create_owner_only(path)` | Create `path` exclusively (`create_new`), mode `0600` on Unix set in the open call, a DACL naming only the current user on Windows; an existing path is an error |
| `write_owner_only(path, contents)` | `create_owner_only`, write, `sync_all` |
| `replace_owner_only(path, contents)` | Write a fresh owner-only sibling (`.<name>.<pid>.tmp`) and rename it over `path`, so an existing file's wider permissions are never kept and no reader sees half a file |
| `restrict_existing_to_owner(path)` | Narrow a path that already exists (`0600`, or `0700` for a directory; the owner-only DACL on Windows), for files a library created |

## Consumers (verified)

- `crates/merod/src/node_config.rs`: `replace_owner_only` for the node config
- `crates/meroctl/src/file.rs` (`create_owner_only`, a download's `.part` file) and `src/env.rs` (`replace_owner_only` in `write_private`, sessions and passkeys)
- `tools/mero-sign/src/lib.rs`: `write_owner_only` for the signing key file

`restrict_existing_to_owner` has no caller outside its own tests.

## Invariants and Gotchas

- **Creation is exclusive**: `OpenOptions::create(true)` would keep an attacker-placed file's ACL and follow a planted symlink or junction; `create_new` refuses both. Do not relax it to truncate-and-reuse.
- **On Windows the ACL is applied after creation**: if it fails, the file is deleted before the error returns, because a secret that exists unprotected is worse than none.
- **`PROTECTED_DACL_SECURITY_INFORMATION` is the point** of the Windows path: without it the parent's inheritable entries merge back in and the file is as readable as its directory.
- **Prefer `create_owner_only` over `restrict_existing_to_owner`** wherever the file is ours to create: restricting afterwards leaves a window in which the path was readable.

Part of [crates/](../AGENTS.md).
