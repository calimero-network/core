# Rollback protection and keys per app: design

Nov 4, 2026 · Status: **rollback protection implemented** on `claude/kms-epoch-rollback`
(core: node, client, merod) and `claude/kms-epoch-rollback` (mero-tee: mero-kms);
**keys per app designed, not implemented yet** (see "Keys per app, phase 1"). Proposed text for the protocol
chapters is at the end; the chapters themselves are Xabi's and are not changed here.

It covers two items of `crates/node/AGENTS.md`, "Not done yet": the rollback counter
(a mero-kms epoch) and keys per app. Earlier notes: PR #4742 (keys per app, closed),
"Rollback protection: design" and "Rollback protection and org recovery" (Sandi, Oct 28).

## Short version

1. **mero-kms keeps a counter per node** of each cluster, keyed by the cluster's key
   path and the node id, and a **cluster epoch**, the number of rollbacks an operator
   has forced. Both come back *sealed* in the key release, bound to the challenge.
2. **The node keeps the same counter inside its sealed store** (a synced row, bound to
   its node id) and raises both about once a second: its store first, then mero-kms.
3. **At start, a store whose counter is below what mero-kms has seen is refused**: the
   node does not start, and says by how much it is behind. A clean restart, or a crash,
   finds its store at or above mero-kms's counter and starts as before.
4. **Recovery is explicit.** One node rolled back: wipe it and register it under a new
   node id (`/ops/nodes`). Every node rolled back, or the newest disks lost: the
   operator starts each node with `--accept-rollback`; mero-kms then lowers that node's
   counter to its store's, raises the cluster epoch, and every node reports the epoch
   (`/health`, `/ops/status`; in its attested KMS evidence next), so a forced rollback
   is visible instead of silent.
5. **Keys per app, phase 1:** every Raft group gets its own random data key, wrapped
   under K_c in the node's store, and seals that group's rows with it. Deleting a group
   drops its key. Groups written before keep K_c; nothing is rewritten.

## Threat model

The host controls each node's VM and disks. It can stop a TD, copy its encrypted disk,
and start the TD later on that copy; it chooses `--kms-url`, `--node-id`, every flag and
the instance metadata. It cannot read or change a TD's memory, forge a quote, or read
mero-kms's root. Sealing hides the content of a copy but not its age: K_r (and K_c)
open an old copy as well as a new one.

| # | The host | Today | With this design |
| --- | --- | --- | --- |
| T1 | Restarts one node on an older copy of its disk | The node forgets votes it cast and serves stale reads; two leaders can share a term | Refused at start (counter behind) |
| T2 | Restarts every node on copies of the same moment | Everything committed since is gone (revoked sessions, removed members, app writes); every replica agrees, so nothing looks wrong | Every node refused; recovery only by `--accept-rollback`, which raises the cluster epoch every node reports |
| T3 | Restarts a majority on old copies, the rest on current disks | The old majority elects a leader and overwrites the rest | The old majority is refused; the current minority starts |
| T4 | Starts an empty store under a node id that had a store | The node rejoins with no memory of its votes | Refused (no counter in the store, one in mero-kms) |
| T5 | Starts an old store under another node id | Old state served under a new name | Refused (the store names its node id) |
| T6 | Swaps rows or files between copies of different ages | Rows open: each is sealed alone | Not covered; see Residual risks |

What an attacker with disk access **can** still do: withhold the network or the disks
(availability), roll a node back by less than one checkpoint interval (about a second
of writes, which Raft still holds on the other replicas unless they are rolled back at
the same moment), and force a rollback with `--accept-rollback`, which every node then
reports. What it **cannot** do: start a node on a store older than the last checkpoint
without that report, move a store to another node id, or lower mero-kms's counter
without a TD that runs the attested merod and was asked to.

Out of scope: availability, and a host that can replace a majority of mero-kms
replicas *at once* (see Residual risks).

## Mechanism

### What mero-kms keeps

Per cluster key path (`{prefix}/{profile}/{peerId}`, the shared disk identity of one
cluster's nodes):

- `epoch: u64`, the number of forced rollbacks (starts at 0);
- for each node id, an entry `(generation, mark)`, ordered as a pair. `mark` only
  rises; a forced rollback sets the entry to `(new epoch, the store's mark)`, which is
  larger as a pair, so every replica can merge two views by taking the maximum.

Counters live in the replicas' memory, like the root. A raise is acknowledged only
after a majority of the `MERO_KMS_REPLICAS` replicas (baked into `kms.env`, so
measured) hold it; a read merges a majority's answers. Replicas authenticate each
other with an HMAC key derived from the root (`mero-kms/tdx-root/counters/v1`), so
only replicas of the same cluster count, and each answers with its own random replica
id, so one replica behind two URLs counts once. A replica that joins copies the merged
counters from a majority before it answers counter requests. Limits:
`MAX_COUNTER_CLUSTERS`, `MAX_COUNTER_NODES_PER_CLUSTER`.

Bring-up: a joiner cannot sync from a majority while only the bootstrap replica is
ready, so the bootstrap replica starts ready in a *genesis* state and a joiner that
hears only empty genesis replicas joins it. Genesis ends for good once one round hears
every replica ready (the first write requires it), and 10 minutes after the root was
made in any case. Leftover risk: a host that runs extra replicas inside that window.
Deployment: the image's `kms.env` must set `MERO_KMS_REPLICAS=5` (the default, 1,
keeps counters per replica, wrong behind a load balancer) and the bootstrap replica
needs `kms-peers`; that is image and Ansible work, not in these PRs.

### Key release, version 2

The node asks for the counters by naming its node id. Everything below is optional on
the wire, so an older merod and an older mero-kms keep working as they do today.

- `/attest` with `transportKey: true` and `capabilities: true` answers
  `capabilities: ["rollback/v1"]` and commits to the list:
  `SHA-256("mero-kms/attest-transport-key/v2" ‖ binding ‖ transport key ‖ "rollback/v1")`.
- `/get-key` carries `rollbackNodeId`. The quote's report data commits to it
  (`SHA-256("mero-kms/sealed-get-key/v2" ‖ sealTo ‖ peerId ‖ nodeId as 8 bytes BE)`)
  and the signed payload names it (`rollbackNodeId`).
- The answer is sealed as today, under the domain `mero-kms/sealed-key/v2`, and the
  plaintext is JSON: `{ keyHex, epoch, generation, mark, sessionIdB64, sessionKeyHex }`
  (`mark` absent when mero-kms has no entry for the node).
- The session key is `HKDF(root, salt "mero-kms/tdx-root/rollback-session/v1", info
  key path ‖ node id ‖ session id)`, so any replica recomputes it and none stores it.
  It lives only in the TD's memory.
- `POST /rollback/raise { peerId, nodeId, sessionIdB64, generation, mark, nonceB64,
  macB64 }` raises the entry, `POST /rollback/reset { ..., mark }` forces it (one
  epoch up). The MACs are HMAC-SHA256 under the session key over a domain
  (`mero-kms/rollback-raise/v1`, `mero-kms/rollback-reset/v1`) and every field; the
  answer `{ epoch, generation, mark, macB64 }` is MACed over the node's nonce
  (`mero-kms/rollback-ack/v1`), so a proxy can neither raise, nor reset, nor fake an
  acknowledgement. No quote per checkpoint.

### What the node keeps and checks

The node's store (`calimero-log`, column `meta`, a node-level row) holds
`{ node, generation, mark, epoch }`, written synced. At start, after the key release
and before any group starts (`calimero_node::rollback::check`):

| mero-kms's entry | The store's | Outcome |
| --- | --- | --- |
| none, KMS without counters | none | Starts **unprotected** (an older mero-kms); reported |
| none, KMS without counters | some | Refused: this store was protected |
| any | another node id | Refused |
| none | none | First start (or first since the upgrade): starts |
| some | none | Refused (an empty or pre-upgrade store under a used id) |
| `(g, m)` | `(g', d) < (g, m)` | Refused: rolled back by `m - d` checkpoints |
| `(g, m)` | `(g', d) >= (g, m)` | Starts |
| none | some | Refused (mero-kms lost its counters) |

Every refusal names `--accept-rollback`. Once started, the node raises: it writes
`mark + 1` to its store (synced), then to mero-kms, at once and every
`CHECKPOINT_EVERY` (1 s). The store goes first, so a crash between the two leaves the
store ahead, which starts. A mero-kms that does not answer is retried; the node keeps
serving, logs it, and reports the age of its last checkpoint.

`--dev` nodes and nodes with `--storage-key*` take no counters (no KMS): development only.

### Recovery

- **One node rolled back** (T1, T4): the node refuses to start. Wipe its data
  directory, remove it (`DELETE /ops/nodes/{id}`, after moving its apps' replicas) and
  register it again under a **new** node id. It joins empty, as a learner, and catches up.
  Reusing the old id with an empty store is refused.
- **Every node rolled back, or the newest disks lost** (T2, T3): the operator restarts
  each node with `--accept-rollback`. The node asks mero-kms (`/rollback/reset`) to set
  its entry to its store's mark; mero-kms raises the cluster epoch. Every node reports
  its epoch (now: `rollbackEpoch` in `/health`, `rollback` in `/ops/status`; next:
  in the attested KMS evidence at `GET /attestation?purpose=kms`, which changes the
  attestation check mero-js mirrors), so a client that remembers the epoch it last saw can
  tell a forced rollback happened. The flag is a host decision, as the restore of an old
  disk is: this design makes it visible, not impossible. Requiring the owner's consent
  (mero-kms pinning the owner's key at the first release) is the next step; see Open
  questions.

### Upgrade of existing clusters

- **merod first, old mero-kms:** the node asks `/attest` for capabilities; an old
  mero-kms ignores the field and answers as today (v1 binding), so the node fetches
  its key as today and starts unprotected (reported). Nothing on disk changes.
- **New mero-kms, old merod:** the old request names no node id; the KMS answers v1.
- **Both new, existing store:** the store has no counter and mero-kms has no entry, so
  the first start writes one. From then on the store is protected, and an answer
  without counters (a downgraded or stripped request) is refused.
- A new mero-tee release is a new mero-kms cluster (new root, new K_r), and a node does
  not move release in place (`encryption.mdx`): a next-release node is a new node with
  a new store, so counters never have to cross releases.
- Migration tests: a store written before this change starts and gains a counter
  (`a_store_from_before_the_counter_starts_and_gains_one`); a store with a counter
  refuses a KMS answer without one.

## Keys per app, phase 1

**Not in these PRs.** Rows move between nodes still sealed (a snapshot streams the
state rows as stored and an install writes them as they arrive), so a key per group
that differs per node means opening and re-sealing every row on both sides of every
snapshot: a change to `calimero-log`'s snapshot path (Xabi's area), with a migration
test and a benchmark of its own. It follows as its own PR.

PR #4742 recommended (option D) a random data key per app, held only by its replicas
and wrapped at rest under a per-node key. Phase 1 does the local half, which needs no
mero-kms change and no hand-over protocol:

- When a group is first started on a node, the store makes a random 32-byte key for
  it, wraps it under K_c (AES-256-GCM, bound to the group id) and keeps it in the
  group's `meta` row `data-key`. The group's `log`, `state` and `meta` rows are sealed
  under it instead of K_c (the AAD is unchanged).
- Snapshots stay sealed under the snapshot key derived from K_c, so a follower on
  another node (with its own data key for the group) opens them as today, and
  re-seals every row under its own key on install.
- Removing a group deletes its key: rows left behind on that node no longer open.
- **Migration:** a group whose rows exist but which has no `data-key` (written before
  this change) keeps sealing under K_c; nothing is rewritten. Its next snapshot install
  moves it to a key of its own. Regression: a data directory written before opens.
- What it does not do yet (phase 2, needs mero-kms and the replica hand-over): keys
  that differ per app *across* nodes but are shared by an app's replicas, crypto-
  shredding of copies already taken, and placement-bound release.

## Residual risks

- **Splicing (T6).** Rows are sealed one by one; an attacker who assembles a store from
  rows of different ages is not detected by the counter. On a TDX node the store sits
  on a disk encrypted with the KMS disk key, so splicing means swapping encrypted blocks
  blind, which breaks the store rather than rolling it back.
- **A short rollback** (under `CHECKPOINT_EVERY`) looks like a crash. Raft keeps the
  entries on the other replicas.
- **mero-kms losing a majority at once.** Counters are as durable as a majority of
  mero-kms replicas. A host that replaces a majority of replicas at the same moment
  resets the counters; every node with a counter then refuses to start (table above), so
  this shows as an outage, not a silent rollback.

## Tests

- mero-tee: a lower raise is ignored; a raise with a wrong MAC is refused; a reset
  raises the epoch; a v2 release returns the entry sealed; counters merge across
  replicas and survive a replica replaced by a joiner; an acknowledgement needs a
  majority.
- core `calimero-kms-client`: `a_v2_release_carries_the_counters_and_a_session_that_raises_them`
  (a stand-in KMS; a stripped answer gives no counters), the shared vectors.
- core `calimero-node` (the user-facing layer: a node starting on a data directory,
  `tests/rollback.rs`): `a_restored_older_data_dir_is_refused_and_a_normal_restart_starts`,
  `an_empty_store_or_another_nodes_store_is_refused`,
  `accept_rollback_starts_and_raises_the_epoch`,
  `a_store_from_before_the_counter_starts_and_gains_one`.
- core `calimero-log`: `a_node_row_survives_a_reopen_and_opens_only_under_its_key`;
  for keys per app (next PR) `each_group_seals_under_its_own_key`,
  `a_groups_rows_do_not_open_under_another_groups_key`,
  `a_store_written_before_data_keys_opens`.

## Open questions

1. **Who may force a rollback.** Visible-only now; owner-signed next (the KMS would pin
   the owner's key at the first release).
2. **When mero-kms is unreachable.** Keep serving and report the checkpoint's age
   (proposed), or stop taking writes after a bound (5 minutes was proposed earlier)?
3. **Interval.** 1 s for every node; one HMAC and a majority round on mero-kms each.
4. **Clients.** mero-js and the Rust client could remember the epoch per cluster and
   warn when it rises; that is a client change, not in these PRs.

## Proposed text for the protocol chapters

For `protocol/encryption.mdx`, replacing the last sentence of "What an observer can
still see":

> A restored older copy of a node's disk is refused: mero-kms keeps, per node, a
> counter the node also keeps in its sealed store and raises about once a second, and a
> node whose store is behind it does not start. An operator can force such a start
> (`--accept-rollback`); mero-kms then raises the cluster's epoch, which every node
> reports in its attestation.

For `protocol/storage.mdx`, under the `meta` column: a group's `data-key` row (its
data key, wrapped under K_c) and the node's `rollback` row.
