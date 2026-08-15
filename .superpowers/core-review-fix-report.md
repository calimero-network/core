# PR #3427 (ephemeral presence) — review-fix report

Worktree: `/private/tmp/claude-501/-Users-xilosada-dev-calimero-work/18081ae4-539b-4394-90df-742de69810cc/scratchpad/eph-rebase`
Branch: `feat/ephemeral-presence-rebased`, base HEAD `cad104b2`.

All four findings fixed, plus the first of the two "cheap and adjacent" items.
The second (threading the resolved signing key through) is deliberately skipped —
rationale at the end.

---

## Finding 1 (SECURITY) — both new JSON-RPC handlers ignored authorization

### What changed

The auth handling in `jsonrpc/execute.rs` was factored into two shared pieces so
that all three handlers use literally the same gate, and neither ephemeral
handler can drift into a laxer variant of the no-auth carve-out.

1. `crates/server/src/jsonrpc.rs:236-278` — new `caller_identity<'a, E>(state,
   auth_key, auth_node_owner, method)`. This is `jsonrpc/execute.rs`'s previous
   inline `match` moved verbatim (same three paths, same `warn!` on the
   misconfigured-guard case, same `RpcError::InternalError("authentication
   required")`, same `debug!` on the intentional no-auth path). Only the log
   messages became method-parameterised.

2. `crates/server/src/execute.rs:57-79` — new
   `caller_authorized_for_context(ctx_client, context_id, caller)`. This is the
   membership check previously inline in `execute_request`:
   `CallerIdentity::Key` → `caller_account::for_context` + `has_member`;
   `CallerIdentity::NodeOwner` → always `Ok(true)`. `Err` means the lookup
   itself failed (fail closed, distinct from non-membership).

3. `crates/server/src/execute.rs:100-112` — `execute_request` now calls the
   helper. Behaviour is unchanged: same error strings
   ("Internal error during membership verification" / "Caller is not a member of
   this context"), same `debug!` skip log for `NodeOwner`.

4. `crates/server/src/jsonrpc/execute.rs:29-36` — uses `caller_identity`.

5. `crates/server/src/jsonrpc/set_ephemeral.rs:32-58` — handler now takes
   `auth_key` / `auth_node_owner` (no longer underscore-prefixed) and gates
   **first, before any other work** — before the size guard and before owned-
   identity resolution. Non-member → `SetEphemeralError::Unauthorized`; lookup
   failure → `SetEphemeralError::InternalError("internal error during membership
   verification")` (`error!`-logged with the real cause; the string handed to the
   client is deliberately generic).

6. `crates/server/src/jsonrpc/get_ephemeral.rs:21-52` — same gate, run before
   `ephemeral_snapshot` is called, so a non-member never reaches the decrypted
   presence data. Non-member → `GetEphemeralError::Unauthorized`.

7. `crates/server/primitives/src/jsonrpc.rs:305-309` and `:400-404` — new
   `Unauthorized` variant on both `SetEphemeralError` and `GetEphemeralError`.
   Both enums are `#[non_exhaustive]` and both were introduced by this PR, so
   this is not a break of any published surface (see "Cross-repo" below).

8. `crates/server/src/jsonrpc/test_support.rs` (new) — shared handler-test
   scaffolding: `state_with(auth_enabled, node_manager)` builds a `ServiceState`
   over an in-memory store, `seed_context_member` writes the `ContextIdentity`
   row `has_member` reads, and `StubNodeManager` / `stub_node_manager` is a
   minimal actor answering `NodeMessage::GetEphemeralSnapshot` so the
   get-path can be driven to completion. `actix` added to
   `crates/server/Cargo.toml` **dev-dependencies only** (workspace version,
   already in the tree).

No-auth mode is preserved exactly: `auth_enabled == false` with no extensions →
`CallerIdentity::NodeOwner` → membership check skipped, same as `execute`. This
matters for the merobox e2e workflow, whose nodes run in proxy/no-auth mode; it
already drives `execute` through the identical path.

### Covering tests

```
cargo test -p calimero-server --lib ephemeral
```

```
running 23 tests
test jsonrpc::set_ephemeral::handler_tests::non_member_key_is_refused ... ok
test jsonrpc::set_ephemeral::handler_tests::non_member_key_is_refused_before_the_size_guard ... ok
test jsonrpc::set_ephemeral::handler_tests::member_key_passes_the_gate ... ok
test jsonrpc::set_ephemeral::handler_tests::node_owner_skips_the_membership_check ... ok
test jsonrpc::set_ephemeral::handler_tests::auth_enabled_without_extensions_is_rejected ... ok
test jsonrpc::set_ephemeral::handler_tests::oversize_slice_returns_typed_slice_too_large ... ok
test jsonrpc::set_ephemeral::handler_tests::exact_cap_slice_passes_the_size_guard ... ok
test jsonrpc::get_ephemeral::handler_tests::non_member_key_is_refused ... ok
test jsonrpc::get_ephemeral::handler_tests::member_key_receives_the_snapshot ... ok
test jsonrpc::get_ephemeral::handler_tests::node_owner_skips_the_membership_check ... ok
test jsonrpc::get_ephemeral::handler_tests::no_auth_mode_still_serves_the_snapshot ... ok
test jsonrpc::get_ephemeral::handler_tests::auth_enabled_without_extensions_is_rejected ... ok
(+ 11 pre-existing wire-shape tests)

test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 66 filtered out; finished in 0.01s
```

Coverage per endpoint:
- refused: `non_member_key_is_refused` on BOTH endpoints.
- authorized member succeeds: `get_ephemeral::member_key_receives_the_snapshot`
  asserts the actual snapshot contents come back. For `set_ephemeral`,
  `member_key_passes_the_gate` asserts the member gets past the gate and fails
  later at owned-identity resolution (a member seeded without a private key has
  no owned identity, so the handler stops before the node actor) — i.e. a
  different error than the refusal, which is what proves the gate let it through.
- carve-out preserved: `no_auth_mode_still_serves_the_snapshot` (auth disabled,
  no extensions → served) and `auth_enabled_without_extensions_is_rejected`
  (misconfigured guard → `InternalError`), on top of
  `node_owner_skips_the_membership_check`.

---

## Finding 2 — heartbeat never refreshed the local author's own entry
## Finding 3 — `AwarenessStore::touch` was unreachable

Fixed together, as suggested.

`crates/node/src/handlers/ephemeral/outbound.rs:307-352` — new
`refresh_and_sweep(awareness_store, local, ttl_ms, now_ms) -> Vec<(ContextId,
Diff)>`. It **touches every local `(context_id, author)` pair first, then
sweeps**, and returns the diffs to emit. The touch is what was missing:
gossipsub never delivers a node's own publish back to itself, so nothing else
re-stamped `last_seen_ms` for a locally-set slice, and the node's own sweep
evicted its own presence one TTL after `set_local_ephemeral`.

`heartbeat_tick` (`:377-386`) now calls it and emits the returned diffs. The
sweep still covers every context the store holds entries for (union of the
store's contexts and the local pairs) — that behaviour and its comment moved
into the helper unchanged.

The extraction exists so the ordering guarantee is unit-testable: `heartbeat_tick`
needs a live `NodeManager` + actix `Context`, which no test in this crate builds;
`refresh_and_sweep` is pure over the store.

`crates/node/src/handlers/ephemeral/store.rs:104-113` — `touch`'s doc comment now
describes what actually happens (called by the heartbeat tick for the node's own
entries, immediately before the sweep, because a node never receives its own
gossip). `touch` now has a real caller, so the dead-code point is resolved.

`heartbeat_tick`'s doc list (`:295-306`) gained the refresh step.

### Covering tests

```
cargo test -p calimero-node --lib ephemeral
```

Two new tests in `crates/node/src/handlers/ephemeral/outbound.rs`:

- `local_author_survives_ttl_across_heartbeats` (`:846`) — a local author and a
  remote author both start at `t0`; the heartbeat ticks every
  `PRESENCE_HEARTBEAT_MS` for `TTL/HEARTBEAT + 4` ticks (17 500 ms > 7 000 ms
  TTL). Asserts the local author is never swept (0 `Diff::Remove`), the remote
  author — which stopped heartbeating — is swept exactly once, and the final
  snapshot holds only the local entry.
- `without_local_touch_the_local_author_is_evicted` (`:910`) — the falsifier:
  with an empty `local` slice (the pre-fix behaviour) the same entry IS evicted
  at the TTL boundary. Without this, the first test could pass for arithmetic
  reasons rather than because of the touch.

```
running 31 tests
test handlers::ephemeral::outbound::tests::local_author_survives_ttl_across_heartbeats ... ok
test handlers::ephemeral::outbound::tests::without_local_touch_the_local_author_is_evicted ... ok
...
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 469 filtered out; finished in 2.24s
```

---

## Finding 4 — e2e scripts could report a false PASS on network failure

Every `curl … || true` capture in both scripts now records curl's exit status
explicitly and fails loudly before the body is inspected.

### `workflows/scripts/ephemeral-ttl-check.sh`

- `:42-53` — the node-2 `set_ephemeral` seed. A failure here means the sweep was
  never armed, so node 1's entry surviving would prove nothing. `FATAL` + exit 1.
- `:62-72` — the load-bearing `get_ephemeral`. `FATAL` + exit 1 on a transport
  failure, before `REMAINING` is computed.
- `:74-82` (**additional, beyond the finding as written**) — the response must
  actually carry `result.entries`. See the note below; this is the case that
  really produced a false PASS.
- `:86-90` — `jq` failure on the count is now fatal instead of `|| echo 0`.

Note on the finding's mechanism: with jq 1.7, an *empty* body makes
`jq '[…]|length'` emit nothing, so `REMAINING` came out empty (not `0`) and the
script printed a garbled `FAIL … still  entries`. Bad, but not a PASS. The
actual false-PASS vector is a **JSON-RPC error response**: HTTP 200 (so `curl
-f` is happy) with `{"error": …}` and no `.result`, under which
`[.result.entries[$k]? // empty] | length` is legitimately `0` — indistinguishable
from a genuine eviction. That case is now caught by the `result.entries` shape
check. Demonstrated below.

### `workflows/scripts/ephemeral-presence-e2e.sh`

- `:51-61` — new `die_curl <label> <rc>` helper: records the failure through the
  normal `fail` accounting, prints the summary line, and exits 1. A request that
  never completed is not a failed assertion, it is an unusable run, and every
  downstream assertion in this script reads an empty body as success.
- `:88-94` — Phase 0 context reads on both nodes.
- `:119-126` — Phase 1 `execute(set)`. This is the exact case in the finding:
  empty body → empty `.error` → "kv-store execute(set) succeeded".
- `:136-139` — Phase 1 hash read (the Phase 4 baseline).
- `:158-165` — Phase 2 `set_ephemeral` (same `.error` masking).
- `:187-200` — Phase 3 poll loop: a single failed poll is still retriable (the
  node may be coming up) but is counted. If ALL polls failed at the transport,
  `die_curl` fires instead of blaming gossip; if only some did, the failure
  message reports how many.
- `:262-267` — Phase 4 hash read (the load-bearing no-DAG-growth guard).

### Verification

`shellcheck` clean, `sh -n` clean on both.

Behavioural proof, against a local stub HTTP server
(scratchpad `stub/srv.py`, `stub/srv2.py` — not committed):

1. Node unreachable (curl exit 7), `ephemeral-ttl-check.sh`:
   `FATAL: set_ephemeral on node 2 failed (curl exit 7) — the request never completed, so the sweep was never armed` / exit 1.
2. Node unreachable, `ephemeral-presence-e2e.sh`:
   `FAIL node1 context API responds: curl exit 7 — the request never completed; downstream assertions would be meaningless` / `=== 0 passed, 1 failed ===` / exit 1.
3. Node answers HTTP 200 with a JSON-RPC **error** for every call:
   - pre-fix (`git stash`): `node-1 entries remaining on node 2: 0` /
     `ok node 1 entry evicted …` / `=== 1 passed, 0 failed ===` / **exit 0 — a false PASS**.
   - post-fix: `FATAL: get_ephemeral did not return result.entries — the entry
     count would be vacuously 0.` / exit 1.
4. Happy path is unbroken: against a stub returning well-formed successes,
   `ephemeral-ttl-check.sh` → `=== 1 passed, 0 failed ===` exit 0, and
   `ephemeral-presence-e2e.sh` → `=== 12 passed, 0 failed ===` exit 0.

---

## Cheap and adjacent

**Done** — `crates/server/src/jsonrpc/set_ephemeral.rs:86-103`: owned-identity
resolution now matches `Some(Err(err))` separately and returns
`SetEphemeralError::InternalError(err.to_string())` (with an `error!` log);
`None` remains `NoOwnedIdentity`. Previously a transient store I/O error told
the client "no owned identity", i.e. "stop retrying" for a retryable condition.
Mirrors what `execute_request` already does.

**Skipped** — passing the resolved signing key from `set_local_ephemeral` into
`do_publish_ephemeral`. It complicates the code for little gain:
`do_publish_ephemeral`'s other caller is `heartbeat_tick`, which has no
pre-resolved key, so the parameter would have to be an `Option` with a
resolve-if-`None` branch inside; five existing test call sites would change; and
the private key would be captured in the spawned future for longer. The
duplicate read is one synchronous point read on the client-driven `set` path
only — the heartbeat path resolves exactly once, inside `do_publish_ephemeral`.
Not worth the branch.

---

## Cross-repo note

`calimero-server-primitives` is `publish = true` and pinned by mero-tee at rev
`1a418f46`. The change here adds an `Unauthorized` variant to
`SetEphemeralError` / `GetEphemeralError`. Both enums are `#[non_exhaustive]`
and both were introduced by this PR (they do not exist at `1a418f46`), and
mero-tee consumes only `Quote` and the tee-attestation surface. **No coordinated
rev bump is required.**

## Not touched (deferred by instruction)

Replay protection / `sent_at_ms`; the `GroupKeyring` epoch-0 tie-break;
unbounded distinct-author map; local echo diverging from the actual publish on
`NoGroup`/`NoGroupKey`.

## Verification commands

```
rustup run 1.88.0 cargo fmt --all --check   # clean
cargo check --workspace                     # clean
cargo clippy --workspace --all-targets -- -A warnings   # no errors
cargo test --workspace                      # see the summary in the handoff message
shellcheck workflows/scripts/ephemeral-presence-e2e.sh workflows/scripts/ephemeral-ttl-check.sh  # clean
```

Presence still writes to no persistent store: the only additions on the node
side are in-memory `AwarenessStore` operations; the server side only reads
membership rows.
