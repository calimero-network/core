# Gating the system group by release approval: design proposal

Oct 29, 2026 · Status: **proposal, not implemented** · No runtime code changes with this document.

Release approvals keep an app's data off node images nobody approved: `Node::may_send`
refuses an app group's Raft entries and snapshots to a peer whose attested mero-tee release
is not approved for that app. The system group is exempt, so every attested node, approved
or not, receives every account, organization, passkey, session record, invite and the
session-signing key. The product decision is that the system group is gated too: a new,
unapproved image must not receive users' and organizations' private metadata.

This document inventories what the system group holds, compares ways to gate it, and
recommends one, with a phased plan, tests, a threat model and the questions the product
owner has to answer.

Short version:

1. **Gating replication alone would not be enough today.** An unapproved node can already
   mint a session for any account (the session key is in the replicated state), call any
   app's view or call as any caller through `internal/*` (the routes trust the cluster
   secret, which every release holds), and write any system command for any actor
   (`apply` ignores the passkey consent). Phase 0 closes these. They are bugs against the
   current threat model too, since they also defeat the app-data gate.
2. **Recommended end state: realms.** The system state is split into a public
   **directory** that every attested node replicates and applies, and private **realms**
   (one per organization, per account and per publisher scope), whose rows travel in the
   same Raft log sealed under a realm key and signed with a realm signing key. Only nodes
   on a release the realm's approver approved receive that key. Entries are judged by the
   node that builds them (it holds the realm), and every node checks them with public data
   only, so replicas stay deterministic, and any attested node can still be a system voter.
3. **Who approves a realm** is pluggable: the realm's approver for app data. The code on
   master gives it to the organization or account owner (or the operator). The coordinator's
   model (operator, or the publisher of the apps an organization uses) maps onto the same
   mechanism; section 4.2 covers an organization that uses several publishers.
4. **The session key goes away.** Tokens are signed by the issuing node's attested key, and
   a node accepts a token only from an issuer whose release may serve the resource. User
   actions that grant credentials or approvals carry a passkey assertion checked at apply.

## 1. Today, from the code

### 1.1 How the system group replicates

| What | Where |
| --- | --- |
| The system group is Raft group 1; its machine keeps the whole `SystemState` in memory and checkpoints it as one JSON document in each snapshot | `crates/node/src/system.rs`, `SystemState` (line 186) and `impl Machine for SystemMachine` (line 2157: `IN_MEMORY`, `snapshot` = `serde_json::to_vec(&self.state)`) |
| Each entry is a `SystemCmd` serialized as JSON, judged by `apply` on every replica | `system.rs`, `SystemCmd` (line 544), `SystemMachine::run` (line 911) |
| The release gate exempts the system group | `crates/node/src/node.rs`, `Node::may_send` (line 781): `if group == SYSTEM_GROUP { return true; }` |
| Every registered node joins the system group as a learner, whatever its release | `node.rs`, `admit_registered_nodes` (around line 945) |
| The leader promotes caught-up learners to 3 voters, whatever their release | `node.rs`, `promote_learners` (line 988); `crates/node/src/membership.rs`, `grow_voters` |
| At rest, entries and snapshots are sealed under K_c, which every node of every release holds | `docs/.../protocol/encryption.mdx` ("The cluster key K_c"); `crates/node/src/cluster_key.rs`, `deliver` (line 394), gated by the peer policy, not by approvals |
| mdma's rollout adds new-release nodes, which core admits as learners and promotes as old voters leave | mdma `dispatcher/app/rollouts.py` (steps `add_nodes`, `remove_old`) |

So a node on any attested release holds the system state in memory, receives every system
entry and snapshot, and can be elected system leader.

### 1.2 What else an unapproved node can do today

These are found by reading the code; each needs a red test before its fix (section 5.11).

- **F1. It can mint a session for anyone.** `SystemState::session_keys` holds K_s
  (`system.rs` line 203; added by the leader with `AddSessionKey`). `principal`
  (`crates/node/src/api.rs` line 266) accepts any token whose HMAC verifies under a K_s
  version in the state, whose session id is not revoked and whose passkey is still on the
  account (lines 294-311). `calimero_identity::mint_session` (`crates/identity/src/lib.rs`
  line 87) is all it takes. The node then calls any app as that user through the
  ordinary sealed API of an approved node. This defeats the app-data gate.
- **F2. Internal routes trust any cluster-secret holder.** `peer_only` (`api.rs` line
  3653) checks only `x-cluster-auth`, `HMAC(K_c, ...)`, which every release can compute.
  `internal_view` (line 4131) and `internal_call` (line 4122) run with the `caller` the
  body names (`ViewRequest::caller`, `CallRequest::caller` in `node.rs`).
  `internal_events` (line 4173) streams any hosted app's events with the subscriber the
  caller picks. `internal_blob_get` (line 3777) returns any blob, and
  `internal_challenge_take` (line 161) returns an open passkey challenge. The peer channel
  is `Noise_NK` (`crates/sealed/src/server.rs`): the server never learns which node is
  calling, so it cannot apply a release gate to inbound requests.
- **F3. It can write any system command as any actor.** `internal_system` (`api.rs` line
  3736) appends whatever `SystemCmd` a cluster-secret holder posts. `apply` trusts `actor`,
  and ignores the passkey consent: `consent: _` in `UpgradeApp` (`system.rs` line 1142),
  `TransferApp` (1261), `SetReleaseApproval` (1595), `SetOutboundApproval` (1634) and
  `DeleteOrg` (1692). An unapproved node can therefore post
  `SetReleaseApproval { scope: Org { org }, actor: <the org's owner>, version: <its own>,
  approved: true }` and approve itself. With operator governance, `SetOperatorRelease`
  (line 1575) carries no proof of the operator at all.
- **F4. Inbound Raft RPCs are not tied to group membership.** `internal_raft` (`api.rs`
  line 3726) hands any group's RPC to the engine after `peer_only`. Whether openraft
  refuses an `AppendEntries` or `InstallSnapshot` from a node outside the group's
  membership was not checked for this document. If it does not refuse, a malicious node
  can overwrite an app group's state. This is listed as an open item.
- **F5. Clients cannot tell an approved node from an unapproved one.** `attest::verify`
  (`crates/client/src/attest.rs`) checks that the node's release is signed and at least the
  floor, not that the user's approver approved it. A client therefore sends bearer tokens,
  API keys and passkey assertions to any attested node. A login assertion is a bearer
  credential for whoever holds it: a node can fetch a challenge from another node
  (`/auth/passkey/login/begin`), have the user sign it, and finish the sign-in itself.
- **F6. K_c opens every node's disk.** With a colluding host, an unapproved image that was
  delivered K_c can open a copied disk or backup of an approved node. That is the
  "keys per app" item (`crates/node/AGENTS.md`, "Not done yet"; a design proposal for
  per-app encryption is open, not merged). It matters here because the
  realm data at rest must not be sealed under K_c alone.

F1 to F3 also break the app-data gate, so they are worth fixing whatever is decided about
the system group.

## 2. Inventory of the system state

Classes:

- **(a)** needed by every node to work at all: to route, place, gate, elect and attest.
- **(b)** private to one organization or one account (or one publisher's scope).
- **(c)** public by design.
- **(s)** a cluster-wide secret, which no design should replicate in plaintext.

There is no email field in the system state. `Account::name` is a display name, and mdma
holds email addresses.

### 2.1 `SystemState` (`crates/node/src/system.rs`, line 186)

| Field | Holds | Class | Who needs it | Notes |
| --- | --- | --- | --- | --- |
| `nodes` (`types::NodeRecord`, `types.rs` line 98) | Node id, URL, zone | a | Everyone: peer URLs, placement, key delivery | Also in `/health` and the Ops API |
| `removed_nodes` | Ids the operator removed | a | The system leader's reconcile | |
| `next_group` | The next app group number | a | Whoever creates an app | Allocator, see 5.4 |
| `governance` | Who holds release, upgrade and outbound decisions | a, c | Everyone; attested with every receipt key | `receipt_key_binding` |
| `quotas` | Cluster-wide limits | a | `apply` of quota-checked commands | The configured limits; usage is computed per account |
| `operator_releases` | The operator's approved releases | a | `may_send`, placement, where the operator approves | |
| `release_deadlines` (in flight, branch `claude/release-approval-capability-and-deadlines`) | Release → end of new work | a | Every node judging new work | |
| `operator_outbound` | Approved outbound prefixes by signer | a/c | App runs on every replica | Operator decisions, no user data |
| `apps` (`types::AppRecord`, `types.rs` line 107): `app_id`, `group`, `replicas` | Routing | a | Every node: start groups, forward calls, `may_send` | |
| `apps`: `name`, `owner`, `org`, `access` (account lists, team), `confirm_methods`, `executor`, `bundle`, `created_at`, `init_args`, `metadata`, `last_upgrade` | What the instance is and who uses it | b (c for `Access::Public` apps' name and metadata) | The app's replicas and the nodes that serve its users | `init_args` can hold anything the creator passed |
| `bundles` (`BundleRecord`) | Hash → size, uploader, package ref | b (c for registry or operator releases) | The app's replicas | `uploaded_by` links an account to code |
| `packages` (`PackageRecord`, `PackageVersion`) | Name, signer pin, owner, versions, manifest metadata, outbound list, `installed_by`, `installers` | Split: the `name → signer_id` pin is a/c (a cluster-wide invariant: first release wins); operator-installed releases are c (`sees_release` shows them to everyone); account-uploaded releases are b | Installers, their org co-members, accounts reaching an app on them (`SystemState::sees_release`) | |
| `accounts` (`types::Account`, `types.rs` line 217): `name`, `created_at`, `passkey_labels`, `aliases` | Profile, device names, the user's names for apps | b | Nodes serving the account | |
| `accounts`: `passkeys` (`calimero_identity::Credential`: id, public key, `sign_count`) | WebAuthn credentials | b today; the public key is public by WebAuthn's design, so it can be c (see 5.6) | Every node that verifies an assertion | `sign_count` reveals sign-in activity: keep it private |
| `accounts`: `approved_releases`, `outbound_approvals` | The account's own approvals | b, with a public copy of the release set (5.2) | `may_send`, placement, routing | |
| `credentials` | Credential id → account | b today; c in the proposal | The node finishing a ceremony | Random ids |
| `api_keys` (`types::ApiKeyRecord`, line 241) | Key hash → account, label, scope, expiry | b | Nodes serving the account | Hashes only |
| `refresh`, `rotated_refresh` (`types::RefreshRecord`, line 260) | Refresh hashes → account, session, passkey, package | b | Nodes serving the account | |
| `revoked_sessions`, `revoked_at` | Revoked session ids and when | b | Every node verifying the account's tokens | Low sensitivity |
| `session_keys` (`SessionKey`, line 34) | K_s, current and previous | **s** | Today, every node | Removed by the proposal (5.6) |
| `orgs` (`types::Org`, line 355): `name`, `members` and roles, `teams` (`types::Team`, line 328: tree, rows, read-only rows, capabilities, metadata, `removed`), `default_capabilities`, `capabilities`, `metadata`, `member_metadata`, `upgrade_runs` | The organization | b | Nodes serving the org's apps and members; app runs read the caller's standing (`app::caller_org`, `app.rs` line 1706) | |
| `orgs`: `approved_releases`, `outbound_approvals` | The org's approvals | b, with a public copy of the release set | `may_send`, placement, routing | |
| `invites` (`types::Invite`, line 543) | Invite hash → org, role, teams, expiry, uses | b | The node redeeming | |

### 2.2 Outside `SystemState`

| What | Class | Where |
| --- | --- | --- |
| System log entries | Same as the fields each `SystemCmd` writes (appendix A) | `SystemCmd` JSON, sealed at rest under K_c only |
| System snapshots and backups | Everything above | `SystemMachine::snapshot`; `crates/node/src/backup.rs` |
| Open passkey challenges | b (short-lived) | `api.rs`, `Pending` (line 48), in the issuing node's memory |
| Raft membership of group 1 | a | openraft's stored membership |

## 3. Constraints any design must meet

- **C1. Determinism.** Every replica applies the same entries in the same order and must
  reach the same state. Today `apply` judges each command against the full state
  (`docs/.../protocol/system-group.mdx`, "The check at apply is the one that counts"). A node
  that cannot read part of the state cannot run that check.
- **C2. Entry size.** An entry is at most 1 MiB (`MAX_ENTRY_BYTES`, `replication.mdx`).
- **C3. mero-kms is frozen, stateless, and has one root per release** (mero-tee
  `docs/design/gcp-tdx-kms.md`). It cannot hand out a key that outlives a release or that
  follows approvals. A secret that must outlive a release passes from node to node by
  attestation ("TEE-only secrets must not depend on a release's root"), as K_c already does
  and as the per-app encryption keys proposal (option D) does for app data.
- **C4. Approvals are per scope, and accounts span scopes.** On master an organization's set
  (`Org::approved_releases`) covers its apps, an account's own set covers its personal apps,
  and the operator's set covers everything under operator governance
  (`SystemState::approved_for`, `system.rs` line 322). In the coordinator's model the
  approver is the operator or the publisher of the app. Either way, one account can belong
  to several organizations, and one organization can use apps from several publishers.
- **C5. Access checks need fresh state on the serving node** (`Node::sys_fresh`, `node.rs`
  line 709; `gate`, `api.rs` line 1790). App runs need the caller's org standing
  (`caller_org`). So the node that hosts an app must hold its organization's membership.
- **C6. Rollout.** New-release nodes start as learners and become voters as old nodes
  leave. Apps move as their approvers approve. Old nodes leave only when nothing needs them.
  The design must not stop a rollout for users whose approver did approve.

## 4. Who approves, and the options

### 4.1 The approver of a realm

A **realm** is the unit of private state that one approval decision covers. The mechanism
in section 5 takes, for every realm `R`, a set of releases `hold(R)`: nodes on those
releases may hold `R`'s private rows. Who sets `hold(R)` is a policy:

| Governance | Org realm | Account realm | Publisher scope (app records of one publisher's apps) |
| --- | --- | --- | --- |
| Operator holds release approvals (`Authority::Operator`, or the coordinator's "operator-held cluster") | `operator_releases` | `operator_releases` | `operator_releases` |
| Owners hold them (master today) | `Org::approved_releases` | `Account::approved_releases` ∪ the sets of the orgs the account is in | the org's set |
| Publishers hold them (the coordinator's model) | the union of the sets of the publishers whose apps the org runs, by default (4.2) | the union over the account's orgs, plus the publishers of its personal apps | that publisher's set |

Under operator governance there is a single set, so the whole design collapses to one
realm with one key, held by nodes on `operator_releases`. That is option (iii) below, and it
falls out of the same code.

### 4.2 An organization that uses apps from several publishers

Take org X running publisher P's chat app and publisher Q's docs app. P approved release
r2, Q did not.

- A P-approved node on r2 that hosts X's chat instance must read X's membership, roles and
  teams to gate calls and answer `caller_org` (C5). If X's core rows go only to releases that
  **every** publisher of X approved (intersection), P's approval of r2 is useless for X until
  Q approves. That is the rollout stall C6 rules out.
- If X's core rows go to releases that **any** of X's publishers approved (union), then P's
  approval sends X's membership to r2, including the fact that X also uses Q's app.

The proposal splits the org realm so that each part goes where it is needed:

- **Org core** (`X/core`): name, members, roles, teams, capabilities, member metadata,
  invites. `hold = ∪ hold(P)` over the publishers whose apps X runs. Every publisher's
  approved nodes need this to serve X's members on that publisher's apps, and each app
  already sees the caller's standing through `caller_org`.
- **Publisher scope** (`X/pub:P`): X's instance records of P's apps (name, access, metadata,
  init args, last upgrade, upgrade runs over them), X's outbound approvals for P's signer.
  `hold = hold(P)`. Q's nodes on a release only Q approved never see that X runs P's app.
- **An organization veto (optional, Q2).** X may restrict `X/core` to `∪ hold(P) ∩ allow(X)`
  with an allowlist of its own. The cost is that an app of a publisher whose release X did
  not allow stops being servable for X, which is the trade-off the org chose.

The same reasoning gives the account realm: an account's auth rows (credential keys, API
key hashes, refresh and revocation rows) must be on every node that serves any of its orgs,
so `hold(A/auth) = ∪ hold(X/core)` over its orgs, plus the publishers of its personal
apps. Its profile rows (aliases, device labels) can use the same set or a narrower one
(Q3).

### 4.3 The options

**(i) Split the state into a public directory and sealed realms in one log.** Section 5.
Private rows travel sealed under a realm key that only nodes on `hold(R)` receive, by
attestation, from nodes that already hold it.

- Security: an unapproved node holds ciphertext and the public directory. It cannot read or
  forge realm rows (they are sealed and signed). It still sees sizes, timing and which
  opaque realm ids change.
- Consensus: entries are judged by their builder, a node that holds the realm, and carry
  the result. Every node checks them with public data only (signature, base index, public
  diff ownership), so C1 holds and any attested node can stay a voter.
- Auth: a node authenticates only accounts whose auth realm it holds. Others get
  `421 WRONG_NODE` with the nodes that can (5.7). Acceptable, since such a node hosts none
  of their apps.
- Rollout: per realm; a new-release node serves every realm whose approver approved it, at
  once. Old nodes stay until every realm has holders on the new release (C6, Q4).
- Migration: a format switch with a partition entry, then the plaintext log prefix is
  purged (5.10).
- Performance: one Ed25519 signature per system write, one verification per node, AEAD
  per row, keys delivered in batches on each node join (5.12).

**(ii) A cluster-wide approval quorum.** A release may hold the whole system state once
k of n parties approve it.

- Security: it gives a decision over an org's metadata to parties the org did not name.
  Sign-up is open (`maxAccounts`), so a quorum of accounts can be packed. A quorum of
  publishers or org owners lets the majority approve for the minority.
- Consensus and rollout: simple. One set, as in (iii).
- Verdict: rejected. It contradicts both the owner and the publisher model.

**(iii) Operator-held only.** The system group is gated by `operator_releases`.

- Security: right where governance gives the operator release approvals, since the operator
  already decides which image holds the apps' data. Under owner or publisher governance it
  gives the operator a decision that governance says it does not hold.
- Verdict: adopted as the degenerate case of (i) under operator governance. Not enough
  alone.

**(iv) Unapproved nodes join as non-voting learners of a redacted projection.** The leader
sends unapproved learners entries and snapshots with the private parts removed.

- Consensus: Raft's log-matching check uses index and term, not content, so a redacted
  payload could be sent. But a learner that cannot read a command cannot apply its public
  effects either (`DeleteOrg` removes app routes, `CreateApp` takes a group number), so the
  entries would have to carry their results. That is the same rework (i) needs. A redacted
  learner can never be promoted (its state is not a voter's), so a rollout cannot replace the
  old voters with new-release nodes until the release is approved by every realm. Snapshots
  would have to be redacted per recipient, and every holder of the full log would be a
  possible leak point in the replication path.
- Verdict: rejected. (i) gets the same effect with the redaction done by cryptography, with
  one log that is identical everywhere and no limit on who votes.

**(v) Unanimity.** The whole private state goes to `∩ hold(R)` over every realm.

- Security: strict, and needs no new approver.
- Rollout: a new release serves nobody until every account and org approves it. One dormant
  account blocks the release forever.
- Verdict: rejected as an end state. It is what (i) does with one realm, so it costs
  nothing extra if wanted as a stopgap.

**(vi) One Raft group per realm.** Each org, account and publisher scope gets its own group,
placed and gated like an app group.

- Security: reuses `may_send` and placement unchanged.
- Consensus: commands that touch two realms (`RedeemInvite`, `CreateOrg`, `TransferApp`,
  `CreateApp` with an account quota) become distributed transactions. The credential index
  still has to be global. 100 000 accounts mean 100 000 groups with a heartbeat every 200 ms.
- Verdict: rejected. Sharding accounts by approval set to save groups reduces it to (i).

| | Confidentiality vs an unapproved image | Deterministic apply | Who may vote | Rollout | Migration | Cost |
| --- | --- | --- | --- | --- | --- | --- |
| (i) realms | Yes (ciphertext only) | Yes, public checks only | Any attested node | Per realm | Partition plus log purge | Low |
| (ii) quorum | Yes, for releases the quorum rejects | Yes | Approved nodes | Cluster-wide | Small | Low |
| (iii) operator only | Yes | Yes | Approved nodes | Cluster-wide | Small | Low |
| (iv) redacted learners | Yes | Only with results in entries | Approved nodes only | Stalls | Same as (i) | Medium |
| (v) unanimity | Yes | Yes | Any (with (i)) | Stalls on any holdout | As (i) | Low |
| (vi) group per realm | Yes | Yes | Per group | Per realm | Large | High |

## 5. Recommendation: realms in the system log

### 5.1 Principles

1. Realm keys give **confidentiality**. **Authority** comes from proofs that every node can
   check from public data: a realm signature for "a holder built this", a passkey assertion
   for "the user asked for this", an operator signature for "the operator asked for this".
2. Every check `apply` makes uses public data only, so a node with no realm key applies
   every entry exactly as a holder does.
3. The node that serves a request holds the realm, judges the command as `apply` does now,
   and records the result.

### 5.2 Data model

The system machine's state becomes:

```text
Directory (public: replicated and applied in the clear by every node)
  nodes, removed_nodes, governance, quotas, operator_releases, operator_outbound,
  release_deadlines, next_group
  routes:      app_id   -> { realm, group, replicas }
  realms:      realm_id -> RealmInfo { kind, hold: BTreeSet<release>, serve: BTreeSet<release>,
                                       keys: [(version, verify_key)], holders: BTreeSet<node>,
                                       last_index }
  credentials: cred_id  -> { account, public_key }            (5.6, Q5)
  package_pins: name    -> signer_id
  catalog:     operator-installed package releases
  node_keys:   node     -> { token_key, release, quote }      (5.6)
  system_format: 1 | 2

Realm rows (stored by every node in the system group's `state` column, opened by holders)
  (realm_id, hidden row key) -> AEAD(DEK[realm, v], row)
     AAD = "calimero.realm.v1" ‖ realm_id ‖ row key ‖ v
```

- `realm_id` is opaque: `org:<id>/core`, `org:<id>/pub:<signer>`, `account:<id>/auth`,
  `account:<id>/profile`, or `cluster` under operator governance. Account and org ids are
  already random.
- `hold` is who may hold the realm's rows (4.1). `serve` is who may act for users on the
  realm's resources (5.6): for an org scope its approver's set, for an account its own.
  Both are public copies so that a node without the key can route and mdma can plan. They
  are written only by entries signed by the realm (or, under operator governance, by the
  operator).
- The row families are the private fields of section 2, keyed by what they are today
  (`account`, `apikey:<hash>`, `refresh:<hash>`, `member:<account>`, `team:<id>`,
  `invite:<hash>`, `app:<id>`, `bundle:<hash>`, `package:<name>/<version>` ...). Row keys are
  hidden with `HMAC(DEK, "calimero.realm.hidden-keys.v1")`, as app state rows are.
- The machine stops being `IN_MEMORY` for realm rows: they live in the group's rows like an
  app group's state (`calimero_log::StateTx`), so snapshots stream them as sealed frames.
  The directory stays in memory and in snapshots. Holders keep a decrypted cache per realm.
- `AppRecord` splits into its route (public) and its record (in `X/pub:P` for an org app,
  in `account:<id>/profile` for a personal one, or public for an `Access::Public` app's name
  and metadata if Q6 says so).

### 5.3 Keys

- **Per realm**: `DEK[R, v]` (AES-256-GCM) and `SK[R, v]` (Ed25519), random, made by the
  node that creates the realm (sign-up, `CreateOrg`, the first app of a publisher in an
  org). The verify key goes in `RealmInfo.keys`.
- **Not from mero-kms** (C3). A node keeps a keyring in its `meta` column, wrapped under
  its node key K_n from mero-kms if the per-app encryption keys proposal (option D) lands first, else
  under K_r, its release key. **Never under K_c**, which every release holds (F6).
- **Delivery.** A holder's reconcile loop finds registered nodes `N` with
  `release_at(N) ∈ hold(R)` and `N ∉ holders(R)` and sends their keys to N's registered
  address over a fresh attested channel (`POST internal/realm-keys/deliver`), batched per
  target node with at most `MAX_REALM_KEYS_PER_DELIVERY` realms per request. This mirrors
  `cluster_key::deliver`. N then writes `RealmHolder { realm, node, version }`, signed with
  `SK[R, v]`, so `holders` is public and auditable.
- **Withdrawal or narrowing.** When a release leaves `hold(R)` (an approval withdrawn, a
  member leaving the last org that brought a release, an org dropping a publisher), the
  realm rotates: a holder makes version v+1, delivers it to the nodes still eligible, and
  writes `RotateRealmKey { realm, v+1, verify_key }` signed with v. New rows are sealed under
  v+1, and a background `ResealRealm` re-seals older rows in entries of at most 1 MiB.
  Nodes no longer eligible delete the key and their cache. As with app data today, they
  keep what they already received.
- **Durability.** A realm with no holder is lost. `RemoveNode` is refused at apply while it
  would leave a realm with fewer than `min(2, eligible nodes)` holders (it is already
  refused while an app keeps a replica on the node). Withdrawing the last approved release
  stays refused.

### 5.4 Entries and apply

```text
SystemEntry::Public { cmd, proof }          // infrastructure: judged at apply as today
SystemEntry::Realm(RealmEntry {
    realms: Vec<(realm_id, key_version, base: u64)>,   // every realm the command touches
    kind:   CommandKind,                                // e.g. "createApiKey"
    public: PublicDiff,                                 // directory rows the realms own
    sealed: Vec<(realm_id, SealedRow)>,
    proof:  Option<UserProof>,      // passkey assertion over digest(kind, realms, public, sealed)
    sigs:   Vec<Ed25519>,           // one per realm, over the same digest
})
```

On every node, holder or not, `apply` of a realm entry:

1. checks each signature against `RealmInfo.keys` (a bad one: no-op, "bad signature");
2. checks each realm's `last_index == base` (otherwise a no-op conflict; this also stops a
   replayed entry);
3. for the command kinds that need it, checks the user's assertion against the public
   credential key and the digest (5.6);
4. checks that the public diff touches only directory rows owned by the signing realms
   (their routes, their `RealmInfo`, their accounts' credentials), and allocates `next_group`
   itself, never from the diff;
5. applies the public diff, stores the sealed rows, sets each realm's `last_index`;
6. on holders only, opens the rows into the realm cache.

Steps 1-5 read public data only, so every node ends in the same directory and the same
ciphertext (C1).

**Building.** The serving node holds every realm the command touches (it had to, to
authenticate the user and read the state). It reads its caches at the realms' `last_index`,
runs today's checks from `SystemMachine::run` (they move to the builder, as an app call's do
in `Group::propose`), seals and signs, and appends through the leader (`internal/system`,
unchanged route). A refused command is answered without appending; today it commits as a
no-op. On a conflict the builder rebuilds, as `Node::call` does. Commands on different
realms never conflict. `sys_write` answers with the builder's verdict once its own replica
has applied the index.

**Cross-realm commands** (appendix A) need a builder that holds all of them:
`RedeemInvite` (`org/core` and `account/auth`), `CreateOrg` (account and new org),
`TransferApp`, and `CreateApp` for the per-account app quota. If no node holds both (an
account whose releases and the org's do not overlap), the request gets `421` with the
nodes that would, or none (Q8).

**Derived hold sets.** `hold(A/auth)` depends on A's memberships, which are private. It is
stored, not derived: an entry that adds A to X or changes X's approver set also updates the
affected accounts' `RealmInfo.hold`, signed by both realms. When one approval changes many
members' sets, the builder splits the updates into follow-up entries; until they land, the
new release simply lacks keys for some members, which delays service but leaks nothing.

**Public commands** (`Genesis`, `RegisterNode`, `RemoveNode`, `SetQuotas`,
`SetOperatorRelease`, `SetReleaseDeadline`, operator outbound approvals) are judged at apply
as today, with an operator signature as proof (5.8).

### 5.5 Raft membership

- Voters and the system leader need no realm key: they order entries, store ciphertext and
  apply the directory. So **any attested, registered node may be a learner or voter**, as
  today. `admit_registered_nodes`, `promote_learners` and mdma's rollout stay as they are.
- A malicious voter or leader cannot read or forge realm rows. It can delay or drop entries,
  and, as leader, answer a stale index to `sys_fresh` and so delay when a revocation takes
  effect on the nodes that ask it. Those are availability and freshness attacks, which the
  operator and host can already mount. Options: accept (recommended now), check
  revocation-sensitive reads against a second voter's index, or restrict voters to releases
  in every realm's `hold` (unanimity again). This is Q7.

### 5.6 Authentication

- **Node-signed sessions replace K_s.** Each node makes an Ed25519 token key in memory at
  start and publishes `NodeKey { node, token_key, release, quote }`, whose quote binds the
  key (`report_data = nonce ‖ SHA-256("calimero.tee-attest.token-key.v1" ‖ key)`), checked
  by `verify_report` on every node. Tokens become `v2.<claims>.<signature>`, with the issuing
  node in the claims. A verifier accepts a token only if the issuer's key is published with a
  valid quote, and the issuer's release is in `serve` of the realm of the resource (the
  app's org scope for an app call, the account for `/auth/*`). It then checks revocation and
  the passkey from the account realm, as `principal` does today. `session_keys`,
  `AddSessionKey` and the leader's rotation go. A node's restart ends its tokens (15 minutes
  at most), and clients refresh: refresh rows live in the account realm and any holder
  exchanges them.
  Effect: a node approved only for org X cannot act as X's members in org Y, even though it
  holds their auth rows.
- **Public credential keys.** `credentials` becomes `cred_id → (account, public key)` in the
  directory, so assertions verify at apply on every node. `sign_count` stays in the realm.
- **User proofs at apply.** Commands that grant credentials or decisions carry the user's
  assertion over the command digest: `AddPasskey`, `RemovePasskey`, `CreateApiKey`,
  `SetReleaseApproval`, `SetOutboundApproval`, `UpgradeApp` (owner), `TransferApp`,
  `DeleteOrg`, and `CreateRefresh` (the sign-in assertion). For refresh exchange, the entry
  carries the consumed single-use token itself, whose hash is in the realm; the realm
  signature covers it. This fixes F3 for these commands and keeps a holder from granting
  itself credentials.
- **Node-bound ceremonies.** A challenge becomes
  `H("calimero.challenge.v2" ‖ issuer transport key ‖ random)`. Before it calls WebAuthn,
  the client checks the challenge against the transport key of the node it attested. A
  node finishes a ceremony only with a challenge it issued, or one whose issuer's release is
  in `hold(A/auth)`. This fixes the relay in F5.
- **Where a user can be served.** A node that does not hold the realm a request needs (the
  account in a token's claims, or the realm of the app's route) answers
  `421 WRONG_NODE { realm, nodes: [url...] }` from the public directory: the nodes whose
  release is in that realm's `hold` or `serve`. An API key does not name its account, so a
  node that does not find its hash answers `401`, and clients send API keys only to nodes
  whose release is in the account's sets (5.9). An `Access::Public` app called anonymously can still be forwarded by
  any node (Q6).

### 5.7 Peer identity and internal routes (Phase 0)

- **Mutual attestation.** The peer channel moves to `Noise_XK` (or keeps NK and adds an
  in-session exchange where the caller proves its attested transport key). The receiver
  matches the caller's static key with the key it attested at that node's registered address
  (`HttpAttestor::seen_release` already keeps the release per address) and knows the
  caller's node id and release. This is a sealed wire-format change: version bump, and a
  paired mero-js change only if clients adopt it (peers alone can use XK).
- **Every internal route judges the caller's release**, the inbound twin of `may_send`:
  `internal/view`, `call`, `events`, `blob/get`, `blob/held`, `blob/delete`, `storage`,
  `app-ready` require a release in `serve` of the app's realm. `internal/raft` requires the
  sender to be a member of that group. `internal/system` requires the caller to hold the
  realms a realm entry names (the signatures prove it anyway) and refuses public commands
  without the operator's signature, except the leader's own housekeeping. `challenge/take`
  answers registered nodes only, and the finishing node checks the issuer (5.6).
- **Operator commands carry proof.** Genesis pins an operator verify key in the directory
  (alongside governance, so receipt-key attestations bind it). mdma signs each operator
  command with a counter. `apply` checks it. Without this, any node can approve releases
  under operator governance (F3).

### 5.8 API changes

| Change | Who |
| --- | --- |
| `421 WRONG_NODE { realm, nodes }` on any route whose realm this node does not hold or serve | Clients |
| Session token v2 (node-signed); v1 refused after the switch | Clients refresh |
| `GET /auth/me` gains `realms` (ids, `hold`, `serve`); `GET /orgs/{org}` gains the same for the org's scopes | Clients, consoles |
| `GET /realms/{realm}/attestation?nonce=`: every holder's receipt-key attestation (like `/apps/{app}/attestation`), with `Client::verify_realm` | Users verifying where their metadata lives |
| Assertions over a command digest for the commands in 5.6 (the current `confirm` flow, with the nonce stored in the consent so apply can rebuild the challenge) | Clients |
| Challenge format v2 (node-bound) | Clients |
| `GET /ops/placement` gains realms: opaque id, `hold`, holder count, whether the target release has holders yet | mdma |
| Ops commands signed with the operator key | mdma |
| Release-approval consent text says the approval also lets the release hold the scope's members' metadata | Consoles |

### 5.9 What clients and mdma change

- **mero-js, meroctl, calimero-client-py.** Keep the account's `hold` and `serve` sets from
  `/auth/me`. Send a token, API key or passkey assertion only to a node whose attested
  release is in them. Before the first sign-in, use the node the user signed up through, or
  a front-door list. Follow `421` to a listed node after attesting it. Check node-bound
  challenges. Add `verify_realm`. The attestation check stays in `calimero_client::attest`,
  mirrored in mero-js `src/cloud-v2/attestation.ts`, changed in both or neither
  (`CODING_STANDARDS.md`).
- **Load balancer.** It cannot read sealed traffic, so it cannot route by realm. Clients
  handle `421`; DNS keeps pointing at all nodes.
- **mdma rollout** (`dispatcher/app/rollouts.py`). `move_apps` is unchanged. A new wait
  before `remove_old`: every realm must have holders on the new release, or be accepted as
  left behind (Q4). The console lists realms that block, by org or account, as it does
  `OrgPendingRelease` today.
- **mdma signs operator commands** with the key pinned at genesis.

### 5.10 Phases and migration

**Phase 0: close F1-F5 (any design needs this).** Mutual peer identity; the release gate
on every internal route; consent verified at apply for the commands that already carry it;
operator-signed public commands; node-bound challenges; the inbound Raft membership check.
This changes the sealed wire format and the consent record, not the system state's shape.

**Phase 1: the realm format.** The directory and realm rows, realm keys and delivery,
builder-judged signed entries, node-signed sessions, `421`, realm placement in the Ops API.
A node advertises `formats: [1, 2]` in `/health` and its `NodeRecord`. The system leader
writes `SwitchFormat { to: 2 }` once every registered node supports 2. Until then nothing
changes. Every node of a rollout runs the new merod, so the gate first protects the rollout
**after** the one that ships it.

Migration at the switch (all nodes still hold the plaintext, so it exposes nothing new):

1. The leader writes `Partition`: for every realm the public `RealmInfo` (verify keys,
   `hold` computed by 4.1 from today's approvals, memberships and app publishers) and the
   directory rows. It delivers the keys to eligible nodes as in 5.3.
2. Holders write `SealRealm` chunks (at most 1 MiB each, signed per realm) carrying each
   realm's rows. Until a realm's chunks have applied, every node keeps answering for it from
   the legacy in-memory state.
3. Once every realm is sealed, the leader writes `DropLegacy`. Every node discards the
   legacy fields, takes a snapshot and purges its log up to that index, so no plaintext entry
   or snapshot is left to send to a node that joins later. Old snapshot files are deleted.
4. K_s is retired at `DropLegacy`. v1 tokens are refused, and clients refresh.

Nodes that ran before the switch keep what they already saw, and so do backups taken
before it (Q9).

**Phase 2: clients and mdma** (5.9). **Phase 3: hardening**: the freshness option of Q7 if
chosen, and per-app encryption keys so that app data stops depending on K_c (F6).

### 5.11 Tests

Each test is written red first, at the layer a user hits (`CLAUDE.md`).

Phase 0, in `crates/node/tests/cluster.rs` next to
`only_releases_the_owner_approved_hold_an_apps_data` (which already starts node 3 on an
unapproved release):

- `an_unapproved_node_cannot_read_an_app_through_internal_routes`: node 3 posts
  `internal/view` with `caller: alice`. Red today: it gets alice's data.
- `an_unapproved_node_cannot_mint_a_session`: a token minted from node 3's `session_keys`
  is refused by node 1 (red today; green when K_s is gone or node 3 is gated).
- `a_release_approval_without_the_owners_assertion_is_refused_at_apply`: a unit test in
  `system.rs` beside `each_decision_belongs_to_the_party_the_deployment_chose`, which today
  passes `consent: Null` and succeeds.
- `an_operator_command_without_the_operator_signature_is_refused_at_apply`.
- `a_raft_rpc_from_outside_the_group_is_refused` (also settles F4).
- `a_relayed_login_challenge_is_refused`.

Phase 1:

- `an_unapproved_node_holds_no_private_system_state`: node 3 on an unapproved release;
  scan its `log` and `state` columns and snapshot files with K_c and the realm keys it holds,
  and find none of alice's name, passkey label, alias, org name, team name or invite hash.
- `entries_apply_the_same_with_and_without_realm_keys`: a property test with two machines,
  one holding every key and one holding none, fed the same random command stream; equal
  directories and equal ciphertext rows.
- `a_realm_entry_with_a_bad_signature_or_a_stale_base_is_a_no_op_everywhere`.
- `approving_a_release_delivers_the_realm_keys_and_the_node_serves_the_user`, and the
  reverse for withdrawal, which rotates the key.
- `an_org_on_two_publishers_shows_each_publisher_scope_only_to_its_releases` (4.2).
- `a_node_approved_for_one_org_cannot_act_for_its_members_in_another` (5.6).
- `removing_the_last_holder_of_a_realm_is_refused`.
- `a_client_follows_421_to_a_node_that_holds_its_realm`, a merobox scenario too.

Migration (scenarios run one build on fresh state, so these are their own tests):

- An `old_datadir.rs`-style fixture: a format-1 data directory (system snapshot and
  plaintext entries) opens, the switch runs, and the raw scan above finds no plaintext; sign
  in and refresh still work, and a v1 token is refused after `DropLegacy`.
- A mixed cluster: nodes with and without format 2 support. No switch until all support it.
- A crash between `Partition` and `DropLegacy` resumes (as `crates/log/src/migration_tests.rs`
  does for the store).
- A restored pre-switch backup is refused, or re-partitioned, per Q9.

### 5.12 Performance

- **System writes.** One Ed25519 signature on the builder and one verification on each node
  (tens of microseconds), AEAD per sealed row. The system log is low-rate: sign-ins,
  refreshes every 15 minutes per active client (10 000 active clients ≈ 11 writes/s),
  directory changes.
- **Contention.** The base check is per realm, so writes for different realms never
  conflict. Today every system command is serialized and judged at apply.
- **Requests.** Token verification becomes an Ed25519 verify (about 50 µs) instead of an
  HMAC; a per-node cache of verified tokens makes it once per token. A client on a node that
  cannot serve it pays one `421` and one attestation.
- **Memory and disk.** Today every node keeps the whole state in memory. With realms, a
  node keeps the directory and the realms it holds in memory, and the rest as ciphertext
  rows on disk. Rows grow by a 12-byte nonce, a 16-byte tag and a hidden key.
- **Joins.** Key delivery to a new node is one batched request per holder: 100 000 realms ×
  ~100 bytes ≈ 10 MB, once.

## 6. Threat model

| Attacker | Today | After Phase 0 | After Phase 1 | Left |
| --- | --- | --- | --- | --- |
| **Malicious, validly signed image on a release no realm approved** (e.g. built by a compromised release pipeline) | Reads every account, org, passkey, API key hash, invite and app record; holds K_s and mints a session for anyone (F1); reads and writes any app as anyone through `internal/*` (F2); approves itself (F3); can lead the system group | Cannot use `internal/*` for apps or forge approvals or operator commands. Still reads the whole system state and still holds K_s | Holds ciphertext and the directory: node list, opaque realm ids and their release sets, app routes, credential public keys, package pins, operator decisions. Cannot mint sessions others accept, forge realm rows or user actions | Sizes and timing of realm writes; availability and freshness as a voter (Q7); what it received before the switch |
| **The same image on a release some realms approved** | As above, for everyone | As above | Reads exactly the realms that approved its release, including the auth rows of those orgs' members; can act for those members in those realms only (its tokens are refused elsewhere, and credential grants need the user's assertion) | Inside the approving realms it is trusted by their choice, as with app data |
| **Operator** (ops token, mdma) | Registers nodes, starts images, removes voters, takes backups; under operator governance approves releases. Cannot read state through the Ops API | Its commands need its signature; nodes cannot forge them | Sees realm ids, release sets and holder counts in `/ops/placement`; under owner or publisher governance cannot put any realm on an image its approver did not approve | Availability; under operator governance it decides, as `docs/.../protocol/governance.mdx` says |
| **Host / hypervisor** | Reads sealed disks only with K_c; with a malicious image it has delivered K_c to, opens any node's disk and backups (F6), system state included | Same | Realm rows on disk need the realm key, wrapped under K_n or K_r of an approved release: a copied disk opened by an unapproved image shows ciphertext | App data still under K_c until per-app encryption keys; rollback of a disk is not detected |
| **Network** | Nothing past the sealed channel | Same | Same | Traffic analysis |

## 7. Open questions for the product owner

1. **Who approves metadata?** Does one release approval cover both an app's data and the
   metadata of the scope (one consent), or is metadata a separate approval? On master
   approvals are per org and per account owner; in the coordinator's model they are per
   cluster, by the operator or the publisher. Which is the target, and when?
2. **An organization's veto.** In the publisher model, may a customer org restrict where its
   own membership goes (`allow(X)`, 4.2), at the cost of not being served by publishers
   whose releases it excludes?
3. **Accounts in several orgs.** Joining an org sends the account's auth rows to every
   release that org's approvers approved. Acceptable? If not, the account must approve
   each release itself before any org can serve it there.
4. **Dormant realms.** An org or account whose approver never approves the new release
   keeps its metadata only on old-release nodes, which then cannot be retired; today only
   app owners can block a rollout. Keep old nodes, delete after a deadline, escrow the realm
   key to the user's passkey (WebAuthn PRF) until they return, or let the operator move it
   (which contradicts owner and publisher governance)?
5. **What may every attested node see?** Proposed public: node list, opaque realm ids with
   their release sets and holder counts, app routes (app id → group, replicas), credential
   ids with public keys and their account ids, package name pins, the operator's catalog.
   Is that acceptable, including the membership hints that release-set updates reveal?
6. **Public apps.** May a node outside an app's approved releases forward anonymous calls
   to an `Access::Public` app, seeing arguments and results in clear? Are public apps'
   names and metadata public?
7. **Voters.** May nodes on unapproved releases be system voters and leader (they can delay
   writes and serve a stale read index), or must voters be on releases every realm holds?
8. **Invites across disjoint releases.** If no node holds both the inviting org and the
   invitee's account, should the redemption wait, or extend the account's hold set at once?
9. **Before the switch.** Every node, approved or not, has already seen the plaintext state
   and K_s, and so have backups. Accept this, retire K_s at the switch (every access token
   ends; clients refresh), and refuse to restore pre-switch backups?
10. **App data and K_c.** Gating the system group does not stop a host colluding with a
    malicious image from opening app data on a copied disk until per-app encryption keys land. Should
    the two ship together?
11. **mdma's view.** May the operator see which realms block a rollout, named by org or
    account (as `OrgPendingRelease` does today), or only counts?
12. **Deadlines.** Should the in-flight release deadlines also apply to metadata, stopping
    new work in a realm that has not approved by the deadline?

## 8. Decisions (product owner, 2026-10-29)

The questions in section 7 are answered as follows. Phase 0 (closing F1–F5) is
scheduled first and does not depend on them.

1. **One approval.** A release approval covers both the scope's app data and its
   metadata. Approvers: node images per cluster, fixed at genesis, by the operator or
   the app's publisher (the developer org that publishes the package); app upgrades per
   app, by the publisher or the instance's owner. Same model for shared and dedicated
   clusters.
2. **No organization veto** for now: a customer org does not restrict where its
   membership goes.
3. **Joining an org** sends the member's auth rows to that org's approved releases:
   accepted.
4. **Dormant realms**: old-release nodes are kept while any realm depends on them, and
   the release deadline restricts new work in that realm (no app creation, installs or
   upgrades). Data is never moved or deleted without approval.
5. **Public directory**: as proposed in section 7.5.
6. **Public apps**: no; a node outside an app's approved releases does not forward
   anonymous calls for it.
7. **Voters**: nodes on unapproved releases may be system voters and leader (an
   availability, not a confidentiality, concern; the rollout needs it).
8. **Invites across disjoint releases**: redemption extends the account's hold set to
   the inviting org's approved releases at once (consistent with 3).
9. **Before the switch**: accept prior exposure; retire K_s at the switch so every
   token refreshes; refuse to restore pre-switch backups.
10. **Order**: Phase 0 first, then realms together with per-app encryption keys.
11. **mdma's view**: publishers (cloud users) by name; their customers' orgs and
    accounts as counts only.
12. **Deadlines** apply to metadata too, since it is the same approval (1).

Rollouts on clusters of fewer than three nodes end at three nodes (core keeps at
least min(3, registered) replicas and never removes the last voter).

## Appendix A. `SystemCmd` by scope

| Command | Scope after the split | User proof at apply |
| --- | --- | --- |
| `RegisterNode`, `RemoveNode`, `SetQuotas`, `SetOperatorRelease`, `Genesis` | Public | Operator signature (`Genesis`: first leader, as today) |
| `PutBundle` | Account (the uploader) | |
| `InstallPackage`, `UninstallPackage` | Account or publisher scope; public pin; public catalog for operator installs | Operator signature for operator installs |
| `CreateApp` | The app's org scope or account; account quota (cross-realm) | |
| `UpgradeApp`, `UpgradeFailed` | The app's scope | Owner assertion, or operator signature |
| `TransferApp` | The app's scope and the receiving account (cross-realm) | Owner assertion |
| `DeleteApp`, `SetAccess`, `SetAppMetadata`, `SetReplicas` | The app's scope (public route) | |
| `CreateAccount` | New account realm; public credential; `maxAccounts` counted from public realm info | Registration attestation |
| `AddPasskey`, `RemovePasskey` | Account; public credential | Assertion by an existing passkey |
| `SetSignCount`, `SetPasskeyLabel`, `SetAlias`, `RevokeAllSessions`, `RevokeSession`, `RefreshReused`, `RevokeApiKey` | Account | |
| `CreateApiKey`, `CreateRefresh`, `UseRefresh` | Account | Assertion; for `UseRefresh`, the consumed token |
| `AddSessionKey` | Removed (5.6) | |
| `CreateOrg` | Account and the new org (cross-realm) | |
| `SetReleaseApproval` | Org scope or account; public `hold`/`serve` | Approver assertion |
| `SetOutboundApproval` | Publisher scope or account; public for the operator's | Approver assertion, or operator signature |
| `DeleteOrg` | Org core and scopes; public routes | Owner assertion |
| `SetOrgMetadata`, `SetMemberMetadata`, `CreateTeam`, `UpdateTeam`, `DeleteTeam`, `SetTeamMember`, `RemoveTeamMember`, `SetCapabilities`, `SetDefaultCapabilities`, `CreateInvite` | Org core (`DeleteTeam` also removes public routes) | |
| `SetOrgRole`, `RemoveOrgMember` | Org core; member accounts' `hold` | |
| `RedeemInvite` | Org core and the account (cross-realm); the account's `hold` | |
| `StartUpgradeRun`, `FinishUpgradeRun` | Publisher scope | |
