# Account identity — one identity, many devices

Design for [#3290](https://github.com/calimero-network/core/issues/3290), built
on the unified causal log (`crates/op`, `crates/authz`, `crates/projection`)
rather than the legacy governance path.

This describes what is **implemented**, and where implementation diverged from
the original plan it records the divergence and why.

## Principle

Every identity is a **stable id** whose **current key set is projected state**.
Ids are content-addressed and immutable; keys rotate via ordinary ops.

Two ids, one strict boundary:

| | `AccountId` | `DeviceId` |
| --- | --- | --- |
| what it is | a person/agent | one installation |
| authorizes | yes — the only authz subject | never |
| signs | no (only certs) | yes, every op |
| CRDT replica id | never | yes |
| KEM recipient | no | yes |

**Non-goal:** accounts never reach below authz. Nothing requiring per-writer
uniqueness (counter slots, HLC seeds), per-key revocability (KEM recipients), or
cryptographic attribution (op signatures) may be re-keyed on `AccountId`.

Test for any future change: *if two devices of the same account did this
concurrently, is the result still correct?* If correctness depends on them being
distinct, it stays device-keyed, and any per-user aggregation happens **above**
storage.

## No backwards compatibility

There is no implicit account for a bare key. An unlinked device speaks for
nobody. The `self_account` / `self_device` derivations from the first draft were
removed: they existed only to keep pre-account identities working, and while the
protocol is alpha that is not worth the second identity model.

The transitional bridge still needs *some* account for a legacy member key, and
that derivation lives **private to `calimero-op-adapter`** — the crate deleted at
cutover. It is deliberately not offered by `calimero-account`: a value derived
from a bare key has no rotatable root and no revocable devices, and exposing one
as a first-class account would quietly reintroduce the id-equals-key conflation
this design exists to remove.

## Types

`crates/account/src/lib.rs` — `AccountId` = `H(AccountGenesis)`, `DeviceId` =
`H(account ‖ nonce)`, `AccountGenesis`, `RootKeyHandoff`, `DeviceCert`,
`KemPublicKey`. Neither id is a public key.

The genesis names the epoch-0 root key, so a certificate is verifiable **from the
account id alone** — no prior op, no ordering dependency. That is what lets a
device link itself into a scope in one self-contained op.

Device sign key and KEM key are separate. No certificate expiry: expiry needs
wall-clock agreement a causal system cannot provide, so withdrawal is a
revocation op.

## Op envelope

`Op` carries an `Authorship { account, device, device_key }` triple, all three
inside the `compute_id` preimage and therefore signed. Omitting `device_key`
would let an attacker swap the signing key; omitting `account` would let a
device's op be replayed under another account.

`Op::verify` checks the signature against the **device key** — an account has no
key of its own. Binding that key to the account is a separate at-cut decision.

Payload variants `DeviceLinked`, `DeviceRevoked`, `AccountKeysRotated` are
appended **after `Noop`** (tags 14–16), not grouped with the governance planes
they belong with: borsh tags by declaration order, so a thematic placement would
renumber later variants and silently change the id of every stored op using one.

## Authorization

One precondition ahead of the payload match: does the signing key currently speak
for the claimed account, at this cut? Resolved from folded state, never from live
store — a verdict depending on receiver state would let two nodes disagree about
one op and diverge on `scope_root`.

`DeviceLinked` is exempt (it *establishes* a binding) and carries its own rules,
of which the load-bearing one is: **the account must already be a member of the
scope.** Linking a device to an already-member account is not a privilege
escalation, because the account holds every right the device gains. That is why
the flow needs no admin approval and no per-scope root-key use — and it is also
the only thing between a stranger and unlimited link ops in a scope.

## Projection

The account plane carries **no LWW stamps**: a grow-only map of self-certifying
genesis records, handoffs keyed by the epoch they depart from, and a grow-only
set of revocation tombstones. It is a join-semilattice by construction, so it
converges without a tie-break.

Revocation lives in its **own tombstone set**, not as a flag on the binding. That
is what makes a revocation folding *before* its link still win; as a flag, a
revoke-then-link arrival order would silently resurrect the device.

The plane folds into `governance_hash`. Otherwise a link or revoke would be
hash-neutral and sync could report convergence while nodes disagreed about who
may author.

### Divergence from the plan: supersession is checked at read time

The plan had one admission rule shared by `authorize` and the fold. Implementation
showed that cannot hold.

Checking "has this root key been superseded" *during* the fold reads whatever
epoch has folded so far, which makes admission depend on delivery order. The
120-permutation test caught it directly. So the rule is split:

- `fold_device_link` — only rules whose answer cannot change as more ops arrive.
- `admit_device_link` — adds the supersession check, for `authorize`, which
  decides against a fixed cut where the question is well-defined.
- `ScopeState::live_devices()` — filters superseded bindings when the view is
  read, once the account's final epoch is known.

Both paths agree on observable state; they reach it in the order each can afford.

A second divergence, same cause: an *admitted* link recorded the account genesis
while a rejected one did not, so link-then-revoke and revoke-then-link disagreed.
The genesis is now absorbed unconditionally — it is self-certifying, so accepting
it is safe even when the link carrying it is refused.

## Forward mapping, never a cached reverse map

Where the legacy bridge must answer in member keys, it maps the *other* side
forward and re-derives per call (`DenyListRepository::denied_members`,
`accounts_to_member_keys`).

An account is a one-way hash, so the reverse is only recoverable from a source
that still holds keys. A cache populated while decoding ops would come back
**empty after a restart** — a node rebuilds its projection from the persisted op
log, and those ops carry accounts only — and the deny check would silently stop
matching. Re-deriving has no state to lose.

## Key delivery

Scope keys are wrapped once per **device**, under that device's X25519 key from
its certificate. Flat per-device wrapping is correct and unavoidable for
device-granular revocation; MLS-style tree wrapping is orthogonal and not needed
at current group sizes.

- **On link:** an existing device of the same account wraps the current scope
  keys for the new one. Self-service — no admin, no key rotation.
- **On revoke:** the scope key **must** rotate; the revoked device holds it.

`build_rotation` takes its recipient list as an input rather than reading
membership rows, so entitlement — an authorization decision — is answered by the
projection like every other one, instead of inside a wrapping helper.

## Runtime and SDK

`executor_id()` returns the **account**. A deliberate semantic change: most apps
saying `executor_id` mean "which user", and the default name must give the safe
answer, or every `Map<executor_id, Vote>` silently becomes one-vote-per-device.
`device_id()` covers the rare device-granular case.

Per-account aggregation lives in the SDK, above storage. Counter slots stay keyed
by `DeviceId`; the mapping is handed *up* to the app, never down into the CRDT.

## Known limits

- **Revocation latency is per scope.** A scope that has not folded the
  revocation still honours the old binding. Inherent to causal revocation.
- **Root-key compromise is not recoverable.** A stolen root key can sign its own
  handoff. Recovery needs a separate authority, which stays reachable because
  `AccountId` is not the key.

## Phasing, corrected

The original order put key delivery before provisioning. That is backwards:
**nothing can be delivered to devices that do not exist.** As of this writing no
code path produces a `DeviceLinked` op, so the folded device set is always empty
in production, and a fan-out reading it would wrap for zero recipients.

| | Scope | State |
| --- | --- | --- |
| A | Types, op envelope, projection planes, authz precondition, legacy bridge | **done** |
| B1 | Native X25519 agreement (`calimero-crypto`) | **done** |
| B2 | Key-rotation recipients as an input | **done** |
| B3 | Account ops on the **`GroupOp` wire** (tags 27–29), device-binding rows, apply handlers | **done** |
| **C** | **Node device identity** — per-device X25519 secret and `DeviceId` stored beside the member identity | **next** |
| D | `KeyEnvelope` → `recipient: DeviceId`, X25519 ephemeral; fan-out over `AccountBindingRepository::devices_of`; backfill on link, rotate on revoke | needs C |
| E | Runtime: `executor_id()`→account, `device_id()`, SDK aggregation | after D |
| F | `meroctl account create / link / revoke`, pairing UX | after E |
| G | `merod export` / `import` | **independent — deferred by decision** |

### Why the wire changed, and what it cost

Accounts have to travel on the transport that exists. `BroadcastMessage` carries
`NamespaceGovernanceDelta`, not unified ops, so a `DeviceLinked` authored into
the unified log reaches no peer. The account ops therefore ship as `GroupOp`
variants, and the fold is the governance apply path writing materialized rows.

The unified-log work (phase A) is **not wasted**: it is correct, tested, and
becomes live at cutover — and it was the specification for the governance path.
Every ordering rule below was found by its 120-permutation test before the
governance implementation existed.

### Membership is NOT re-keyed, and does not need to be

The plan assumed granting an account required re-keying membership onto
`AccountId` — 96 files on the authorization path. It does not.

An account whose **epoch-0 root key is a granted member key** belongs to that
member, because only the holder of that key's private half can sign
certificates under it. So the gate is *"is this account's root key a member of
the group?"*, answerable against the key-keyed rows that already exist.

Anyone may construct a genesis naming someone else's member key — the genesis is
public — and such an account passes the gate. It gains them nothing: enrolling a
device needs a signature from the root key they do not hold. The gate keeps
strangers from writing link rows for unrelated accounts; the signature keeps
them out of accounts that are not theirs.

Re-keying membership onto `AccountId` remains the cleaner end state and lands
with the cutover, but it is cleanup, not a prerequisite.

For D, the envelope's recipient is `DeviceId` and nothing else: an unwrapping
device already holds its own X25519 secret, and ECDH needs only the
`ephemeral_pk` already on the envelope, so the recipient's key never has to
travel. Only the *sender* needs it, and reads it from the folded binding — the
same source that decides whether the device is still authorized, so the fan-out
cannot wrap for a device the projection says is revoked.

## Tests

`crates/projection/tests/account_plane.rs` — 15 tests. Two properties carry the
weight: **causal honor** (a write authored before a revocation stays valid when
re-judged on a node that already applied it) and **convergence** across all 120
delivery orders of a five-op workload.

That permutation check found both divergences recorded above. Neither would have
surfaced under a conventional apply-in-order test.
