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

### On the governance path, all three gates resolve at the cut

The same rule the projection follows, for the same reason, and it took a second
pass to get right — the first implementation of these handlers read **live**
membership rows:

- **`AccountDeviceLinked`** — "is the account's root key a member" via
  `membership_path_at_cut` (direct or inherited). A live read decides against
  whatever this replica has folded, so a node that had already applied a
  concurrent removal of the root-key holder would refuse a link its peers
  recorded. A refusal writes nothing while the op keeps its place in the DAG, so
  the disagreement is permanent and has no later op to reconcile it.
- **`AccountDeviceUnlinked`** — **a group admin at the cut, and nobody else.**
  Revocation is terminal, so an ungated one is a permanent denial of service any
  member could inflict on any other; membership is not authority over other
  members' devices.
- **`AccountKeysRotated`** — deliberately ungated. The handoff is self-certifying
  against state the group already holds: a rotation for an account it has never
  learned is refused outright, and the only way it learns an account is a link
  that already passed the membership gate. Gating the relayer would only break
  legitimate re-gossip.

When the cut is real but unfolded here, the gates raise
`ApplyError::AuthorityUndecidable` and park for retry rather than guess from live
rows. A deterministic refusal, by contrast, returns `Ok` and records nothing —
erroring would stall forever on an op that can never succeed.

**The self-service revocation is not implemented, and cannot be gated on folded
state.** "Is the signer this account's current root key" depends on which
rotations this replica has folded, so two replicas would disagree about one op.
The lost-laptop case therefore needs the op to carry a **root-signed revocation
proof**, self-certifying exactly as `DeviceCert` is — a wire addition that belongs
with the CLI that mints it (phase F), not a gate that looks correct and diverges.

## Projection

The account plane carries **no LWW stamps**: a grow-only map of self-certifying
genesis records, handoffs keyed by the epoch they depart from, and a grow-only
set of revocation tombstones. It is a join-semilattice by construction, so it
converges without a tie-break.

Revocation lives in its **own tombstone set**, not as a flag on the binding. That
is what makes a revocation folding *before* its link still win; as a flag, a
revoke-then-link arrival order would silently resurrect the device.

It is literally a **set**, carrying no value. It first recorded the revoked
device's account, resolved as `devices.get(device).map_or(payload.account, ..)` —
which reads whether the link had folded yet, so link-then-revoke stored the
binding's account while revoke-then-link stored the payload's claim. That value is
hashed into `governance_hash`, so an op naming an account that disagreed with the
binding split the root purely by arrival order. Nothing ever read the value, so
the fix was to not have one: set union is a join by construction.

**Self-service revocation needs a binding that proves the claim.** `authorize`
resolved the revoker as `devices.get(device).map_or(payload.account, ..)` too,
which made the claim unfalsifiable when the device had no binding: any linked
member could name its own account beside an arbitrary device id and be
authorized. Since a tombstone is terminal *and* an early revocation beats the
link it withdraws, that permanently spent a device id the attacker had no
relationship to — observe a link op, revoke at an earlier cut, done. A
self-service revocation now requires a folded binding whose account is both the
author and the payload's claim; "no binding" leaves only the admin path, which is
still needed to eject a device whose link a cut has not folded.

The plane folds into `governance_hash`. Otherwise a link or revoke would be
hash-neutral and sync could report convergence while nodes disagreed about who
may author.

### The same shape again: seed collisions are checked at read time

`DeviceId::hlc_seed()` is the device's RGA/HLC instance seed, and two live replicas
sharing one mint colliding character ids — losing writes silently. So at most one
of a colliding pair may be live, lower id wins.

The first implementation enforced that at link time, rejecting an incoming device
when an already-linked one compared lower. That is order-dependent in the one
direction it does not check: low-then-high left a single device live, but
high-then-low admitted **both**, because a stored high id does not compare lower
than an incoming low one. Both planes now apply the rule where supersession is
applied — over the stored/folded set, in `live_bindings` and `live_devices` — where
it is a function of the op set and cannot depend on arrival order.

Dropping the link-time check also removed a full device scan from every link
apply, on a path any member can drive.

### Handoff candidates coexist; the walk picks the one that verifies

Absorbing a credential's handoff chain happens **before** the credential is
verified, and is gated only on the genesis matching the claimed account — which
anyone can satisfy, because a genesis is public data. So whoever can author an op
in the scope can put a handoff into any account's `(account, epoch)` slot.

If one candidate could occupy a slot, that is a **rotation rollback**: forge a
handoff the victim's key cannot have signed, win the slot, and `resolved_accounts`
stops there — reverting the account to the root key it deliberately rotated away
from. Rotation exists because that key was compromised. And it converges, so it
never shows up as a divergence.

Every candidate is therefore retained, keyed by `(new_root_sign_pk, signature)`,
and the walk takes the first that **verifies** in ascending new-key order.
Both halves of the key matter: keying on the new root key alone still allowed
displacement, because that key is broadcast in the clear, so a forged handoff
reusing it with a garbage signature landed on the identical key and overwrote the
real one. Ascending order preserves the tie-break between two rotations an account
genuinely signed itself.

Absorption is additionally confined to `handoff.account == cert.account` and to
chains within `MAX_ROOT_KEY_HANDOFFS`. Neither prevents the rollback on its own —
an attacker names the victim in both fields — but cross-account writes have no
legitimate use, and an uncapped chain grows this map without limit on a crate with
no wire-bounds layer.

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

`build_rotation` takes its recipient list as an input rather than reading
membership rows, so entitlement — an authorization decision — is answered by
folded state like every other one, instead of inside a wrapping helper.

### Divergence from the plan: the envelope addresses a member *or* a device

The plan had `KeyEnvelope.recipient` simply become a `DeviceId`. That cannot
work, and the reason is a bootstrap deadlock rather than a migration
inconvenience:

`AccountDeviceLinked` is an **encrypted `GroupOp`**. Publishing one therefore
requires already holding the scope key. If the only way to *receive* a scope key
were an envelope addressed to a device, a new member could never obtain the first
key — it would need a link to receive a key, and a key to publish a link.

So `recipient` is an `EnvelopeRecipient`, a discriminated address that carries
its own agreement key:

- `Member { identity, ephemeral_pk }` — ECDH over the Curve25519 form of the
  Ed25519 namespace identity. The **bootstrap** form, and permanent: key
  delivery, the sync pull path, and invitation/TEE admission have nothing else to
  address. Not legacy — necessary.
- `Device { device, ephemeral_pk }` — native X25519 to the certified KEM key.
  Everything after bootstrap.

Addressing and agreement are one field on purpose. Split across two fields, a
`DeviceId` could be paired with an Ed25519 ephemeral — a state no sender produces
and no unwrap path services. The variant tag is also inside the **signed**
payload, so rewriting the borsh discriminant cannot reinterpret a member
envelope's identity as a device id while the signature still verifies.

### Who gets addressed how

`current_key_recipients` resolves per member, device-first:

- A member this group knows an account for is addressed **only** through that
  account's live devices. There is no identity fallback for them, and that is the
  security property, not a gap: the revoked device runs on a node that still
  holds the member key, so an identity-addressed envelope would hand the key
  straight back. A member whose every device is revoked or superseded receives
  nothing until they enroll again.
- A member with no account here is addressed by identity — the bootstrap case,
  and the only one, since an account cannot exist for someone who never held the
  key long enough to publish a link.

Each entry is paired with the member it rests on (`EntitledRecipient`), which is
what makes excluding a removed member take **every device of theirs** with them.
Filtering by recipient alone could only drop the identity entry.

The member→account direction is re-derived per call by scanning account rows and
matching each one's current root key — see *Forward mapping* above for why a
cache would silently disable revocation after a restart.

### The pull path obeys the same rule

`GroupKeyRequest` originally named only the requester's identity key, which made
the pull the one path that could undo device-granular revocation: a node whose
device had just been revoked was still that member, so it was served the current
key on its next sync round.

The request now also carries the device it asks as, and the responder resolves the
reply with `key_recipient_for_requester` — the same device-first rule as the
fan-out, narrowed to one member. `requester_device` is deliberately
unauthenticated: the reply is sealed to that device's certified X25519 key, so a
false claim yields an envelope the caller cannot open. The wrap is the
authentication, which is why the request needs no signed proof.

The bootstrap case is unchanged and cannot be retired — a member with no account
in the scope is still served by identity, which is what lets a keyless joiner
start at all.

### Three gaps that block the acceptance test

Found while writing `account-device-revoke-lockout.yml`, and worth stating
because none is visible from the code that exists:

**1. Per-device *authorization* is not wired on the governance bridge.**
*(Closed — the resolver now falls through to the account plane. The device→
account mapping is read from materialized rows, because account ops do not reach
the fold on this bridge; the authority question — is that account's endorser a
member — still resolves at the op's cut.)*
Delivery is device-aware; authorization is not. `DeviceBinding.sign_pk` is
stored and folded into the hash but **never consulted**, so a device whose own
key is not a granted member cannot author — which is the whole point of a second
device. The fix is not the 96-file re-key onto `AccountId` that this document
defers to the cutover: it is one change to the membership resolver, to accept a
key that is the `sign_pk` of a live binding whose account is a member. It also
retires a field that is currently written and never read.

**2. There is no way to enroll into an *existing* account.** *(Closed — see
"Pairing" below.)* `ensure_enrolled` only ever minted a fresh account of the
enrolling node's own. Adopting an account whose genesis arrives from another node
is `ensure_enrolled_into`, driven by the two-way `pair-init` / `pair-complete`
exchange the ordering forces.

**3. One device per node per namespace.** `NodeDeviceIdentity` is keyed by
namespace, so "two devices" means two nodes. That is why the acceptance scenario
uses three: an admin, Alice's laptop (the only member), and Alice's phone —
which is deliberately *not* a member and participates solely as a device of
Alice's account. A pleasant consequence: nothing needs to tell merobox which
device authors, because the node selects it.

### How a non-member device participates at all

A paired device needs two things it cannot get from membership: a namespace
identity to sign with, and a gossip subscription to receive on. Both already
exist as membership-free production calls, and neither is `join_namespace` —
that publishes `RootOp::MemberJoinedAt`, which is the one thing a paired device
must not do.

- **Identity** — `NamespaceRepository::get_or_create_identity`. It resolves the
  namespace root by walking parent rows, and an unknown group has no parent row,
  so it returns itself. A node that has never heard of the namespace can
  therefore mint an identity for it. (`store_identity` is the setter underneath
  and is called only from there; searching for *it* finds only tests, which is
  misleading.)
- **Subscription** — `NodeClient::subscribe_namespace`. Idempotent, no
  membership check.

TEE fleet-join already does exactly this pair on a node that is not a member and
may never be admitted, so a non-member namespace participant is an established
shape rather than something pairing invents.

**The subscription is not durable on its own, and the failure is silent.**
Startup rehydration walks `list_all_groups`, which filters on membership, so a
node whose only relationship to a namespace is a device row resubscribes to
nothing after a restart — no error, no log line, it just stops receiving ops.
Fleet-join has the same hole and survives it because admission is its whole
purpose; for a paired device, non-membership is the steady state.

So startup also walks `NodeDeviceRepository::enrolled_namespaces` — the device
row family is written by *enrollment* rather than by joining, which makes it the
one on-disk set that means "namespaces this node can speak in" regardless of
membership. The two walks overlap for a member that enrolled a device, which is
harmless because subscribing is idempotent.

This is glue with no unit seam — the proof that it holds end to end is a restart
step in the acceptance scenario, not a `cargo test`.

### Pairing, and why the backfill needed no new op

Pairing is `account pair-init` on the new device then `account pair-complete` on
the one that holds the account. The split is forced, not stylistic: the new
device mints three values nobody else can derive — its `DeviceId` (which needs
the account, since the id is `H(account ‖ nonce)`), its KEM key, and its signing
key — while only the holder has the root that certifies them.

The signing key is the one easy to forget. The certificate names it and
per-device authorization resolves a signer *through* it, so omitting it from the
exchange yields a certificate naming a key no signature ever matches: the device
links and still cannot author.

`pair-complete` publishes **two** ops, and the second is what makes the first
useful:

1. `AccountDeviceLinked` — the encrypted `GroupOp`, carrying the root-signed
   certificate and the member endorsement. This confers authority.
2. `RootOp::KeyDelivery` — the current scope key, wrapped to the device's
   agreement key.

**The delivery needed no new op type, and the reason is the same bootstrap
constraint as everywhere else here.** `KeyEnvelope.recipient` is already an
`EnvelopeRecipient` with a `Device` variant, and `KeyDelivery` is already a
*cleartext* `RootOp`. That last part is load-bearing: the pairing device holds no
scope key, so a device-addressed envelope inside an encrypted `GroupOp` would be
unreadable by its only recipient — the identical deadlock that keeps the
member-addressed envelope alive. Being a root op is what breaks the cycle.

The wrap takes the KEM key from the exchange rather than re-reading it from the
folded binding, so the delivery does not depend on the publisher having already
folded the link it just published.

**Only the current key is delivered.** Peers retain rotated-out keys, so history
*could* be handed back, but that would make every newly paired device a
full-history reader — a capability decision that deserves its own change rather
than riding in on pairing. The cost is stated plainly: a paired device converges
on forward state and cannot decrypt ops sealed under retired epochs.

A failed delivery is reported (`key_delivered: false`) rather than swallowed or
treated as a failed pairing. The link already conferred authority and the
device's own sync pull re-requests the key — but until that lands the device
cannot read, and a flat success would hide exactly that.

### Not yet wired: the ops that trigger delivery

Two behaviours from the plan are **not** implemented, for the same reason: no
code path publishes an account op yet.

- **On link, an existing device backfills the new one.** Needs a publisher of
  `AccountDeviceLinked` to hang off. That publisher is the pairing flow in phase
  F; a listener built now would react to an event nothing emits.
- **On revoke, the scope key must rotate.** Same — plus the non-admin case needs
  a rotation debt the current ledger cannot express (it is keyed by member
  identity, and `GroupKeyRotated { departed }` excludes that member, which would
  cut the account's *other* devices off the key too).

Phase F therefore carries three things, not one: the CLI, the two deliveries
above, and the root-signed revocation proof that makes self-service revocation
possible without an order-dependent gate.

The delivery *machinery* both need is complete and tested: the envelope, both
wrap/unwrap modes, the recipient resolution, and a receive path that accepts a
rotation bundle carrying both addressing modes at once.

## Runtime and SDK

`executor_id()` returns the **account**. A deliberate semantic change: most apps
saying `executor_id` mean "which user", and the default name must give the safe
answer, or every `Map<executor_id, Vote>` silently becomes one-vote-per-device.
`device_id()` covers the rare device-granular case.

Per-account aggregation lives in the SDK, above storage. Counter slots stay keyed
by `DeviceId`; the mapping is handed *up* to the app, never down into the CRDT.

### The candidate slot is a bounded-cost DoS, not a closable one

Retaining every handoff candidate is what stops the rollback, and it has a cost:
`resolved_accounts` re-verifies the candidates at each epoch on every read. An
attacker padding a slot makes every subsequent read more expensive.

Both obvious bounds are wrong. Capping candidates per slot **on insert** is
order-dependent — which ones get in depends on arrival order, so two replicas
could resolve different keys. Keeping the lowest N by key is deterministic but
**exploitable**: grind N candidates ordering below the real one and it is evicted,
which is the rollback again.

Verifying at absorb time closes only epoch 0, where the required signer is the
genesis key carried in the op itself. At epoch *N* the required key is whatever
resolving 0..*N* produces, which the fold cannot know mid-stream — that is the
same reason supersession is a read-time question.

So the mitigations are: a per-op chain cap (`MAX_ROOT_KEY_HANDOFFS`), and
resolving **once per read** rather than the two-to-four times `acl_view` and
`governance_hash` previously each paid. What remains — many ops growing state
without bound — is not specific to this plane; it is the DAG's growth problem, and
it is answered by compaction, not by an admission rule.

## Target model: a dedicated account root

The account is currently rooted at the node's **namespace identity**. That works,
and everything above is built on it, but it fails the requirement that matters
most: it dies with the node. Lose every device and the root is gone with them, so
nothing can certify a replacement — the exact case recovery exists for.

Two candidates are ruled out. The **namespace identity** is the thing being
recovered from. The **libp2p keypair** is public: a genesis travels in the clear
and names its root key, so anyone seeing a link op would learn the account belongs
to that `PeerId`, correlating the node's namespaces with each other and with its
network identity. It would also make the transport key an identity root — the
one-key-many-jobs conflation this design exists to remove.

So the root is a **dedicated key, generated once per node, kept offline, used only
to sign device certificates and root-key handoffs.**

### Nonces are derived, not stored

```
nonce(namespace) = KDF(root_secret, namespace_id)
AccountId        = H(genesis(root_pk, nonce(namespace)))
```

Storing the nonce per namespace — as the row does today — puts it on the node, so
losing every device loses the nonces and the root can no longer *name* the accounts
it owns. Deriving it means the recovery input is one secret plus a list of
namespace ids, and the list is not secret.

Per-namespace nonces are what keep this from costing privacy: one root everywhere,
but a **different `AccountId` per namespace**, so nobody correlates a person across
namespaces. That tension is real and was previously resolved silently — one
identity everywhere and mutually unlinkable namespaces cannot both hold, and this
picks unlinkability while still giving recovery.

### The gate needs an endorsement

The link gate asks *"is this account's root key a member of this group?"* An
offline root is a member nowhere, so the link op carries a **member endorsement**:
the node's namespace member key signs a statement binding that account id to
itself. The gate becomes *"is the endorser a member at this cut, and did it validly
sign this account id?"*

Equally strong — only a member can endorse, and only the root holder can certify
devices — and it needs no re-key of membership onto `AccountId`. A wire addition,
so it is cheap while the schema is already a flag day and expensive afterwards.

### Recovery, and what it does not cover

Two separable halves, and conflating them is what makes recovery look impossible:

| | Provided by |
| --- | --- |
| Proving you are you | the **root key** — a self-certifying cert, verified from the account id alone |
| Getting scope keys | a **peer** — you hold none, and forward secrecy means they cannot be re-derived |

So a peer's role is transport, not judgement: it cannot impersonate the recovering
account, and it does not have to decide out-of-band who somebody is. The publish
also needs a peer, since `AccountDeviceLinked` is an encrypted `GroupOp` and a
keyless device cannot publish its own link — the bootstrap constraint again.

Peers retain rotated-out keys (`load_key_by_id` resolves them), so history *can* be
handed back. Whether it should is policy worth deciding deliberately.

**Root-key compromise stays unrecoverable.** A stolen root signs its own handoff.
Offline storage makes theft unlikely and the chain allows pre-emptive rotation, but
whoever holds the root is the account.

### Full-node restore

`root key + [namespace_ids]` is enough: for each namespace, provision an identity,
prove ownership with the root, let a peer deliver keys. Applications follow for
free — `BlobId` is content-addressed and the app id is in the group meta, so it is
fetched from any peer once the namespace is joined.

It cannot be *zero* input: nothing enumerates a person's namespaces from a key, and
namespaces do not index each other, so the id list has to come from somewhere.

This makes phase G (`merod export` / `import`) **dependent on this change, not
independent of it** — and much better on the far side. Without it, an export must
carry every namespace identity's private key, a pile of secrets each of which is a
full impersonation risk if the export leaks. With it, the export is one secret plus
a non-secret list, and the import is a cryptographic recovery rather than a
key-material restore.

### Where the root actually lives, and what is still missing

The root is generated on first use and written to the node's own RocksDB — the
`NodeAccountRoot` singleton in `Column::Group`, and **plaintext unless the operator
enabled at-rest encryption**, which is off by default.

So the recovery property is *available, not delivered*. The key is structurally
right — one key, node-level, per-namespace derived nonces, so a single backup would
cover every namespace — but nothing exports it. It survives a namespace-identity
rotation and does not survive losing the disk, which is the case the whole story is
about. Describing it as "kept offline" is aspirational until an export exists.

That is the concrete reason phase G depends on this rather than the reverse. Two
follow-ups, neither blocking pairing:

1. **`merod account export` / `import`** — the root out to paper or hardware and
   back. Until it exists, calling this a recovery key overstates it. Tracked as
   [#3335](https://github.com/calimero-network/core/issues/3335).
2. **Three secrets now sit in `Column::Group`** — namespace identity, device KEM
   secret, account root. Nothing ships that column wholesale today, so this is not
   an exposure, but the store's own docs list `Group` under "synced (replicated)".
   That invariant is held by convention, and the root is the worst thing to bet it
   on. Either move them to a column documented as never-synced, or fix the docs and
   add a guard that fails if anything iterates `Group` wholesale.

### What this deletes, and what it cannot

Deleted: rooting accounts at the namespace identity, the stored `account_nonce`,
and `account create` as a step a user is expected to run (auto-enrolment on the
existing `OpEvent::GroupKeyDelivered` replaces it; the command stays as an
idempotent escape hatch).

**Not deletable, and not legacy:**

- **The member-addressed envelope.** A keyless node cannot publish the link that
  would make it device-addressable. It is one message per node per namespace, not a
  compatibility mode — and the distinction matters, because treating it as a mode
  is what produced the fallback rule that let a revoked device pull its key back.
- **`legacy_account_id`.** Membership on the governance bridge is keyed by member
  key, and that derivation is how the whole membership fold bridges onto the
  account-keyed `AclView`. Removing it *is* the re-key onto `AccountId` that the
  cutover carries. The endorsement above exists precisely so this change does not
  have to wait for it.

## Known limits

- **Revocation latency is per scope.** A scope that has not folded the
  revocation still honours the old binding. Inherent to causal revocation.
- **Root-key compromise is not recoverable.** A stolen root key can sign its own
  handoff. Recovery needs a separate authority, which stays reachable because
  `AccountId` is not the key.
- **A group teardown clears the account plane with it.** `delete_group_local_rows`
  drops bindings, tombstones and per-account root keys, because a revocation
  tombstone is terminal: a group recreated under the same id would otherwise
  inherit device ids it can never enroll, with nothing in its own history to
  explain why.
- **A member with no live device cannot recover a key on its own, by design.**
  Re-enrolling needs an encrypted `GroupOp` and so needs the key, so a member
  whose every device is revoked or superseded depends on an admin to re-deliver
  it (`RootOp::KeyDelivery`) or to publish the replacement link on its behalf — a
  link op need not be signed by the device it enrolls. If such a member could
  re-key itself, revocation would mean nothing. The cost is that a legitimate
  "all my devices were lost" recovery is not self-service.
- **Member-addressed delivery remains reachable by admin action.** Re-admission
  (`add_group_members`) and an explicit `RootOp::KeyDelivery` both wrap to a
  member identity, so an admin can hand a key to a node that still runs a revoked
  device. That is an explicit privileged act, not an automatic path, but it means
  revocation is not proof against a careless admin.

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
| C | **Node device identity** — `NodeDeviceIdentity` row family (`0x44`), per-namespace `DeviceId` + X25519 secret | **done** |
| D | `KeyEnvelope` → `EnvelopeRecipient{Member,Device}`, native X25519 wrap, device-first fan-out, both-modes receive | **done** |
| D′ | On-link backfill (in F3) and revoke-triggered rotation (in F4) | **split across F** — each needs its publisher |
| D″ | Device-addressed sync pull (`GroupKeyRequest.requester_device`) | **done** |
| E | Runtime: `executor_id()`→account, `device_id()`, SDK aggregation | after F |
| F3 | `meroctl account pair-init` / `pair-complete`, on-link key backfill | **done** |
| F4 | `account revoke`, revoke-triggered rotation, root-signed revocation proof | **done** |
| F5 | merobox helpers, wire the acceptance scenario into CI | next — see the scenario's Wiring notes |
| G | `merod account export` / `import` | **deferred** — [#3335](https://github.com/calimero-network/core/issues/3335) |

`NodeDeviceIdentity` is an additive row family rather than two more fields on
`ContextIdentity`: that struct is `#[expect(clippy::exhaustive_structs)]` with
construction sites throughout the node, and a device secret has its own lifetime
anyway — minted once at enrollment, never rotated in place, dropped with the
namespace. It is keyed per namespace so one machine presents neither the same
replica id nor the same agreement key in two of them.

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

## The acceptance test

`apps/scaffolding-e2e/workflows/account-device-revoke-lockout.yml` — two nodes,
one account, two devices, revoke one, assert the revoked device is locked out and
the survivor is not.

**Committed before the code it tests, and not yet passing.** It is the definition
of done for phase F rather than something backfilled afterwards, and it is
deliberately not wired into `e2e-rust-apps.yml` until it can pass — a permanently
red required check trains people to ignore CI.

It is the only thing that will have proven the networked half of this feature.
Everything else is unit- and apply-path-tested, and `CLAUDE.md` is explicit that a
green `cargo test` does not prove a networked flow works. Two of its steps exist
because a unit test structurally cannot reach them: the **on-link backfill** (a
paired device holds no scope key of its own, so it can only read what it wrote if
an existing device wrapped the current keys for it) and the **pull-path lockout**
(a revoked device's node is still a member, so it can ask a peer for the rotated
key — and the failure is an absence of delivery, which only a log signal catches).

It needs one thing from outside this repo: a `call` step that can choose *which*
of a node's devices authors, which is a merobox change.

## Tests

`crates/projection/tests/account_plane.rs` — 15 tests. Two properties carry the
weight: **causal honor** (a write authored before a revocation stays valid when
re-judged on a node that already applied it) and **convergence** across all 120
delivery orders of a five-op workload.

That permutation check found both divergences recorded above. Neither would have
surfaced under a conventional apply-in-order test.
