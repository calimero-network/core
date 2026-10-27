---
name: unreachable-subsystems
description: Finds code that compiles, lints clean and is referenced, but that nothing can actually reach — dead islands whose items reference each other while the outermost edge points at a route, command, or registration that does not exist. Use when auditing for vestigial subsystems, superseded models left in place, endpoints with no server route, CLI commands that 404, or scenario files registered in no CI matrix. Complements dead-code-cleanup, which handles compiler-visible orphans.
---

# Find unreachable subsystems in calimero-network/core

Find code that **compiles, lints clean, and is referenced — but that nothing can
actually reach**. Not unused imports or orphan functions: the compiler already
catches those, and the `dead-code-cleanup` skill already covers them. You are
looking for the class beneath that.

## The thing you are hunting

A **dead island**: a set of items that reference each other, so every one of them
looks used, while the island as a whole is connected to nothing a user or another
system can invoke.

```
CLI subcommand ─► client method ─► HTTP route that does not exist
      │                │
      └────────────────┴──► response type ─► primitive type
```

Every arrow is a real reference. `cargo check` is happy. `clippy -D warnings` is
silent. `#[warn(dead_code)]` sees nothing, because each item genuinely has a
caller. The island is dead only because the **outermost** edge points at
something that isn't there.

This is why the technique is not "find things with no references". It is:

> **Trace every chain outward until it reaches an anchor. A chain that
> terminates without reaching one is dead, however many references it contains.**

## Anchors in this repo

An anchor is something outside the code that can pull it into execution:

| Anchor | Where to check |
| --- | --- |
| an HTTP route | `router` in `crates/cloud-node/src/api.rs` (every `.route(`) |
| a CLI subcommand a user can type | the clap enums in `crates/meroctl/src/*.rs`, `crates/merod/src/main.rs` and `crates/merod/src/node_config.rs` |
| a merobox step or scenario | `apps/*/workflows/*.yml`, `workflows/**`, and the step's existence in the merobox package |
| a CI job | `.github/workflows/*.yml` — a scenario file that exists but is registered nowhere never runs |
| a test | any `#[test]` / `#[tokio::test]` that actually asserts on it |
| a wire/event payload | types named in `calimero-primitives` events, WASM host functions, `#[app::*]` macros |
| a public SDK/API surface | something an external consumer imports by name |

If a chain ends anywhere else — including at another item in the same island —
keep walking. Do not stop at the first reference you find.

## Worked example — the one that prompted this

(From v1: the crates it names, `calimero-server`, `calimero-client` and v1's
`meroctl`, were deleted with the v1 node. The method is what carries over.)

`crates/primitives/src/identity.rs` held `Did`, `RootKey`, `ClientKey` and
`ContextUser`. `RootKey` carried a `wallet_address`, dating it to wallet-login,
before the account model existed.

What a naive reference count said:

- `Did` — 5 references
- `ClientKey` — 9 references

Both look alive. What tracing outward found:

- `Did`'s references were **its own definition, one line of `AGENTS.md`, and the
  string `"Did not dial"` in a log message.** Zero real consumers.
- `ClientKey` was used by `GetContextClientKeysResponse`, which was used by
  `Client::get_context_client_keys`, which was used by
  `meroctl context get <ctx> client-keys` — a **shipped subcommand**.

That last chain is the instructive one. It has a CLI anchor, so it should be
alive. But the client method calls `admin-api/contexts/{id}/client-keys`, and
**that route does not exist** — not in `endpoints.json`, not in the server. The
subcommand 404s. The anchor was real; what it anchored to was not.

The correct move was to walk one step further than felt necessary. Net: −176
lines across five crates, including a user-facing command that had been broken
in the field.

Note also what was **not** dead in the same file: `AccountId` and `DeviceId` live
in `calimero-primitives` rather than `calimero-account` for a stated dependency
reason (the account crate depends on primitives, and the ids appear in
client-facing event payloads defined there). Two generations of the same concept
shared one file, and only one was vestigial. Do not assume co-location implies
shared fate.

## Where to look first

Places where this class accumulates:

1. **`calimero-cloud-client` methods** whose path does not appear in
   `router` (`crates/cloud-node/src/api.rs`).
2. **`meroctl` subcommands** whose client call is one of the above, or whose
   request body names fields the handler's `Deserialize` struct does not have.
3. **Routes with no caller**: a route in `router` that no client, no
   `meroctl` command, mero-js nor calimero-client-py calls may be dead, or a
   missing command.
4. **v1 crates kept for the runtime** (`node/primitives`, `context/config`,
   `network/primitives`, `governance-types`, `store`, `store/blobs`,
   `utils/actix`, `crypto`): items in them only the deleted v1 node used.
5. **merobox scenario files** not registered in any `.github/workflows/*.yml`
   matrix — they exist, look maintained, and never run. The v1 scenarios under
   `apps/*/workflows/` and `workflows/` are the first place to look.
6. **Types superseded by a newer model** where the old one was left in place:
   grep for concepts the docs describe in the past tense, or fields naming
   abandoned integrations (`wallet_`, `near_`, `did_`).
7. **Feature-gated code** where the feature is enabled nowhere.

## Verify before you report — the traps

Do **not** report something as dead until you have ruled out:

- **Serde-only wire types.** A struct with no Rust callers may still be
  deserialized from JSON by an external client. Check `mero-js`,
  `calimero-client-py`, and merobox before concluding.
- **Macro-generated references.** `#[app::*]`, derive macros, and
  `storage-macros` can reference types no plain grep will show.
- **Trait impls as the whole point.** An `impl Report for X` is a "reference" to
  `X`, but if nothing constructs `X`, both are dead.
- **Test-only survival.** Used only by tests that test *it* is dead; used by
  tests that test *something else* may be a real fixture.
- **Feature-gated builds.** Check `--features mock-attestation`,
  `outbound-testing` and `calimero-storage/testing` before saying nothing
  compiles against it.
- **The `AGENTS.md` echo.** Docs mentioning a type are not a consumer, and a
  reference count that includes them will mislead you. Filter them out.
- **String false positives.** `"Did not dial"` matched `\bDid\b`. Read every hit;
  do not trust the count.

## What to produce

For each island, a short report:

1. **What it is** — the set of items, and what concept they implement.
2. **Why it is unreachable** — the chain, ending at the missing anchor. Name the
   exact thing that does not exist (a route, a registration, a caller).
3. **Evidence** — reference counts *after* filtering docs and string matches, and
   the specific check that proves the anchor is absent (e.g. "not in
   `endpoints.json`", "not in the CI matrix").
4. **Blast radius** — every file that must change to remove it.
5. **Anything user-visible** — a broken CLI command or a 404ing client method is
   more urgent than dead types, and should be called out separately rather than
   buried in a line count.
6. **What looks similar but is alive**, and why — so a reviewer can see you
   distinguished them.

Rank by whether removal fixes something a user could hit, then by size.

## Ground rules

- **Read every hit.** Counts lie; `"Did not dial"` is why.
- **Prove the anchor is absent**, do not infer it from silence. "I could not find
  a route" is weaker than "`router` in `crates/cloud-node/src/api.rs` has no
  `.route(` for that path".
- **One island per report.** Do not bundle unrelated removals.
- **Do not remove anything in this pass.** Report first; removal is a separate,
  reviewed change with `cargo fmt --check`, `clippy -D warnings` and
  `cargo test`.
- If a chain is ambiguous, say so and state what would settle it. A confident
  wrong removal costs more than an honest maybe.
