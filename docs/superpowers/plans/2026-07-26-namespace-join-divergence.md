# Namespace-join divergence fix: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A node that joins a namespace while its governance broadcast cannot reach existing members must converge with the group once connectivity returns, instead of staying permanently invisible.

**Architecture:** Three product changes plus a harness cleanup. The joiner retries its own membership broadcast when it reached nobody, re-armed off the existing peer-subscribed event. The readiness beacon gains an optional invitation proof so an existing member can safely pull from a peer it does not yet recognise. The disconnect redial path stops excluding mDNS-discovered peers. The E2E scenario drops dead config that misdescribes it.

**Tech Stack:** Rust 1.88.0, actix actors, libp2p 0.56 (gossipsub, mDNS, QUIC/TCP), borsh, Docker + merobox for E2E.

**Design doc:** `docs/superpowers/specs/2026-07-26-namespace-join-divergence-design.md`

## Global Constraints

- Comments: avoid where the code speaks for itself. Where needed, at most 2 lines, explaining why rather than what.
- No PR numbers, issue numbers, or tool/agent names in any code comment.
- Reuse existing helpers, types, and events. Do not introduce a new abstraction where an existing one fits.
- Import order follows StdExternalCrate: std, external crates, `crate`/`super`, local modules.
- No `mod.rs`. No dead code. No `.unwrap()`/`.expect()` without a `// SAFETY:` justification.
- Commit format: `<type>(<scope>): <short summary>`, imperative, lowercase, no trailing period.
- `cargo fmt --check`, `cargo clippy -- -A warnings`, and `cargo test` must pass before each commit.
- Branch off latest `origin/master`. Never commit to `master`.
- merobox pinned at `0.6.43` locally and in CI.

---

### Task 0: Branch, toolchain, and baseline measurement

**Files:**
- No source changes.

**Interfaces:**
- Consumes: nothing.
- Produces: branch `fix/namespace-join-divergence`; a baseline pass-rate number used in the final PR description.

- [ ] **Step 1: Fetch and branch from latest master**

```bash
git fetch origin
git switch -c fix/namespace-join-divergence origin/master
git log --oneline -1
```

- [ ] **Step 2: Upgrade merobox to the pinned version**

The local CLI is 0.6.39; CI requires `>=0.6.43`. Pin both to the same exact version.

```bash
pipx upgrade merobox 2>/dev/null || pip install --user --upgrade 'merobox==0.6.43'
merobox --version   # expect: merobox, version 0.6.43
```

- [ ] **Step 3: Clear stale containers**

```bash
merobox stop --all || true
docker ps --format '{{.Names}}'   # expect no notready-node-* containers
```

- [ ] **Step 4: Build the local node image**

```bash
./scripts/build-all-apps.sh
docker build -t merod:local -f Dockerfile .
```

If a repo-local helper exists for this, prefer it. Check `.github/actions/build-local-merod/action.yml` for the exact build command CI uses and mirror it.

- [ ] **Step 5: Record the baseline pass rate**

```bash
for i in $(seq 1 20); do
  if merobox bootstrap run apps/scaffolding-e2e/workflows/group-join-mesh-not-ready.yml >/tmp/base_$i.log 2>&1;
  then echo "$i PASS"; else echo "$i FAIL"; fi
  merobox stop --all >/dev/null 2>&1
done | tee /tmp/baseline.txt
grep -c PASS /tmp/baseline.txt
```

Expected: roughly 10 of 20 pass. Save `/tmp/baseline.txt`; the number goes in the PR description.

- [ ] **Step 6: Commit the design doc**

```bash
git add docs/superpowers/specs/2026-07-26-namespace-join-divergence-design.md docs/superpowers/plans/2026-07-26-namespace-join-divergence.md
git commit -m "docs(governance): design for namespace-join divergence fix"
```

---

### Task 1: Redial mDNS-discovered peers on disconnect

**Files:**
- Modify: `crates/network/src/handlers/stream/swarm.rs:196-201`
- Test: `crates/network/src/discovery/state_tests.rs`

**Interfaces:**
- Consumes: `DiscoveryState::is_peer_relay`, `is_peer_rendezvous`, `is_peer_discovered_via`, `peer_direct_addrs`, `NetworkManager::redial_direct`.
- Produces: no new API. Behavioural change only.

The `ConnectionClosed` handler currently skips the entire direct-redial and rendezvous-recovery block for any peer discovered via mDNS. In CI and on any LAN deployment mDNS is the only discovery mechanism, so the redial never runs and recovery waits on the 300-second mDNS query interval.

- [ ] **Step 1: Write the failing test**

Add to `crates/network/src/discovery/state_tests.rs`:

```rust
#[test]
fn mdns_discovered_peer_retains_direct_addrs_for_redial() {
    let mut state = DiscoveryState::default();
    let peer_id = PeerId::random();
    let addr: Multiaddr = "/ip4/172.17.0.2/udp/2428/quic-v1".parse().unwrap();

    state.add_peer_discovery_mechanism(&peer_id, PeerDiscoveryMechanism::Mdns);
    state.update_peer_protocols(&peer_id, &[]);
    state.add_peer_addr(&peer_id, &addr);

    assert!(state.is_peer_discovered_via(&peer_id, PeerDiscoveryMechanism::Mdns));
    assert!(
        !state.peer_direct_addrs(&peer_id).is_empty(),
        "an mdns-discovered peer must expose a direct address for redial"
    );
}
```

Method names must match the existing ones in `crates/network/src/discovery/state.rs`. Read that file first and adjust the setup calls to the real API; do not invent methods.

- [ ] **Step 2: Run the test**

```bash
cargo test -p calimero-network mdns_discovered_peer_retains_direct_addrs_for_redial
```

Expected: PASS or a compile error naming a wrong method. If it compiles and passes, it is a guard for step 3, not a red test. Fix any method-name errors before continuing.

- [ ] **Step 3: Remove the mDNS exclusion**

In `crates/network/src/handlers/stream/swarm.rs`, change the branch condition from:

```rust
} else if !self.discovery.state.is_peer_rendezvous(&peer_id)
    && !self
        .discovery
        .state
        .is_peer_discovered_via(&peer_id, PeerDiscoveryMechanism::Mdns)
{
```

to:

```rust
} else if !self.discovery.state.is_peer_rendezvous(&peer_id) {
```

Update the surrounding block comment: the two-branch split is now relay versus everything else. Delete the now-stale sentence naming mdns in the branch description and the `&& !mdns` note. Keep it to two lines.

Remove the `PeerDiscoveryMechanism` import if it becomes unused in this file.

- [ ] **Step 4: Verify the crate builds and tests pass**

```bash
cargo test -p calimero-network
cargo clippy -p calimero-network -- -A warnings
```

Expected: PASS, no unused-import warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/network/src/handlers/stream/swarm.rs crates/network/src/discovery/state_tests.rs
git commit -m "fix(network): redial mdns-discovered peers on disconnect"
```

---

### Task 2: Expose the signed op from the namespace publish path

**Files:**
- Modify: `crates/governance-store/src/namespace/governance.rs` (around line 547 and the `sign_apply_and_publish_namespace_op` wrapper at 2030-2041)

**Interfaces:**
- Consumes: `SignedNamespaceOp::sign`, `DeliveryReport`.
- Produces:
  - `NamespaceGovernance::sign_apply_and_publish_returning_op(&self, node_client, ack_router, signer_sk, op) -> EyreResult<(DeliveryReport, SignedNamespaceOp)>`
  - `calimero_governance_store::sign_apply_and_publish_namespace_op_returning_op(store, node_client, ack_router, namespace_id, signer_sk, op) -> EyreResult<(DeliveryReport, SignedNamespaceOp)>`

Task 3 needs the exact signed op in hand so it can rebroadcast it without re-signing. Re-signing would mint a second `MemberJoinedAt` at the next nonce, which is why the previous implementation gave up after one attempt. There is no load-op-by-hash API in the store, so capture it at publish time.

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `crates/governance-store/src/namespace/tests.rs`:

```rust
#[tokio::test]
async fn sign_apply_and_publish_returns_the_signed_op() {
    let (store, node_client, ack_router, ns_id, sk) = namespace_publish_fixture();
    let op = NamespaceOp::Root(RootOp::MemberJoinedAt {
        member: sk.public_key(),
        signed_invitation: test_invitation(&sk, ns_id),
        joined_at: 0,
    });

    let (report, signed) = NamespaceGovernance::new(&store, ns_id)
        .sign_apply_and_publish_returning_op(&node_client, &ack_router, &sk, op)
        .await
        .expect("publish");

    assert_eq!(
        report.op_hash,
        hash_scoped_namespace(ns_topic(ns_id).as_str().as_bytes(), &signed).unwrap(),
        "returned op must be the one the report describes"
    );
}
```

Reuse whatever fixture helpers already exist in that module for building a store, node client, ack router, and invitation. Read the file first and match the existing helper names; do not add new fixtures if equivalents exist.

- [ ] **Step 2: Run the test**

```bash
cargo test -p calimero-governance-store sign_apply_and_publish_returns_the_signed_op
```

Expected: FAIL, `no method named sign_apply_and_publish_returning_op`.

- [ ] **Step 3: Split the existing method**

Rename the body of `NamespaceGovernance::sign_apply_and_publish` to `sign_apply_and_publish_returning_op`, change its return type to `EyreResult<(DeliveryReport, SignedNamespaceOp)>`, and return the `signed` value already bound at line 547 alongside the report.

Reduce the original to a delegating wrapper so no existing caller changes:

```rust
pub async fn sign_apply_and_publish(
    &self,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    signer_sk: &PrivateKey,
    op: NamespaceOp,
) -> EyreResult<DeliveryReport> {
    let (report, _signed) = self
        .sign_apply_and_publish_returning_op(node_client, ack_router, signer_sk, op)
        .await?;
    Ok(report)
}
```

Add the free-function sibling next to `sign_apply_and_publish_namespace_op` at line 2030, mirroring its shape exactly.

- [ ] **Step 4: Run the test and the crate suite**

```bash
cargo test -p calimero-governance-store
```

Expected: PASS, and no existing test breaks.

- [ ] **Step 5: Commit**

```bash
git add crates/governance-store/src/namespace/governance.rs crates/governance-store/src/namespace/tests.rs
git commit -m "refactor(governance-store): return the signed op from namespace publish"
```

---

### Task 3: Rebroadcast the membership op when it reached nobody

**Files:**
- Modify: `crates/node/src/readiness.rs` (struct at 448-459, plus a new message handler)
- Modify: `crates/context/src/handlers/join_group.rs:373-390`
- Test: `crates/node/src/readiness.rs` test module

**Interfaces:**
- Consumes: `sign_apply_and_publish_namespace_op_returning_op` (Task 2), `ReadinessManager { node_client, datastore }`, the existing `EmitOutOfCycleBeacon` handler path.
- Produces:
  - `pub struct PendingRepublish { pub namespace_id: [u8; 32], pub op: SignedNamespaceOp, pub invitation: SignedGroupOpenInvitation }` (actix `Message`, `Result = ()`)
  - `pub struct PendingJoin { pub op: SignedNamespaceOp, pub invitation: SignedGroupOpenInvitation, pub queued_at: Instant }`
  - `ReadinessManager::pending_republish: HashMap<[u8; 32], PendingJoin>`

The invitation is carried here as well as the op, because Task 6 needs it to attach the admission proof to outgoing beacons. A named struct rather than a tuple, so Task 6 adds no churn.

`ReadinessManager` already holds `node_client`, `datastore`, and a per-(peer, namespace) rate-limit map, and already receives a signal on peer-subscribed via `EmitOutOfCycleBeacon`. That is the moment a previously unreachable member becomes reachable, so it is the correct place to retry.

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/node/src/readiness.rs`:

```rust
fn pending_join_at(queued_at: Instant) -> PendingJoin {
    PendingJoin { op: test_signed_op(), invitation: test_invitation(), queued_at }
}

#[test]
fn pending_republish_expires_after_cap() {
    let mut pending: HashMap<[u8; 32], PendingJoin> = HashMap::new();
    let stale = Instant::now() - (REPUBLISH_CAP + Duration::from_secs(1));
    let _ = pending.insert([7u8; 32], pending_join_at(stale));

    prune_expired_republishes(&mut pending, Instant::now());

    assert!(pending.is_empty(), "entries past the cap must be dropped");
}

#[test]
fn pending_republish_survives_within_cap() {
    let mut pending: HashMap<[u8; 32], PendingJoin> = HashMap::new();
    let _ = pending.insert([7u8; 32], pending_join_at(Instant::now()));

    prune_expired_republishes(&mut pending, Instant::now());

    assert_eq!(pending.len(), 1);
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p calimero-node pending_republish
```

Expected: FAIL, `cannot find function prune_expired_republishes`.

- [ ] **Step 3: Add the registry and the pruning helper**

In `crates/node/src/readiness.rs`:

```rust
const REPUBLISH_CAP: Duration = Duration::from_secs(600);
```

Add the field to `ReadinessManager`:

```rust
    /// Membership ops that reached no peer at join time, retried when a
    /// namespace peer next subscribes. Bounded by `REPUBLISH_CAP`.
    pub pending_republish: HashMap<[u8; 32], PendingJoin>,
```

Add the helper as a free function so it is unit-testable without an actor:

```rust
pub struct PendingJoin {
    pub op: SignedNamespaceOp,
    pub invitation: SignedGroupOpenInvitation,
    pub queued_at: Instant,
}

fn prune_expired_republishes(pending: &mut HashMap<[u8; 32], PendingJoin>, now: Instant) {
    pending.retain(|_, p| now.duration_since(p.queued_at) < REPUBLISH_CAP);
}
```

Update every `ReadinessManager` construction site to initialise the new field. Find them with `rg -n "ReadinessManager \{" crates/`.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p calimero-node pending_republish
```

Expected: PASS.

- [ ] **Step 5: Add the registration message**

In `crates/node/src/readiness.rs`:

```rust
#[derive(Message)]
#[rtype(result = "()")]
pub struct PendingRepublish {
    pub namespace_id: [u8; 32],
    pub op: SignedNamespaceOp,
    pub invitation: SignedGroupOpenInvitation,
}

impl Handler<PendingRepublish> for ReadinessManager {
    type Result = ();

    fn handle(&mut self, msg: PendingRepublish, _ctx: &mut Self::Context) {
        let _ = self.pending_republish.insert(
            msg.namespace_id,
            PendingJoin {
                op: msg.op,
                invitation: msg.invitation,
                queued_at: Instant::now(),
            },
        );
    }
}
```

- [ ] **Step 6: Drain the registry on peer-subscribed**

In the existing `EmitOutOfCycleBeacon` handler, after the beacon emission, add the drain. Publish the stored op on the namespace topic using the same transport path the beacon emission already uses:

```rust
prune_expired_republishes(&mut self.pending_republish, Instant::now());
// Keep the entry: Task 6 still needs the invitation until membership lands.
if let Some(op) = self.pending_republish.get(&msg.namespace_id).map(|p| p.op.clone()) {
    let net = self.node_client.network_client().clone();
    let topic = calimero_context::governance_broadcast::ns_topic(msg.namespace_id.into());
    if let Ok(inner) = borsh::to_vec(&NamespaceTopicMsg::Op(op)) {
        let envelope = BroadcastMessage::NamespaceGovernanceDelta {
            namespace_id: msg.namespace_id,
            delta_id: [0u8; 32],
            parent_ids: Vec::new(),
            payload: inner,
        };
        if let Ok(bytes) = borsh::to_vec(&envelope) {
            let _ = actix::spawn(async move { let _ = net.publish(topic, bytes).await; });
        }
    }
}
```

Mirror the exact envelope construction already used for beacons at `crates/node/src/readiness.rs:726-745`; do not duplicate it if a helper can be extracted. If extracting, keep it in the same file.

- [ ] **Step 7: Register from the join handler**

In `crates/context/src/handlers/join_group.rs`, replace the existing publish call with the op-returning variant and register on zero acks:

```rust
match calimero_governance_store::sign_apply_and_publish_namespace_op_returning_op(
    &datastore,
    &node_client,
    &ack_router,
    namespace_id.into(),
    &sk,
    member_joined_op,
)
.await
{
    Ok((report, signed)) if report.acked_by.is_empty() => {
        // Reached no peer; retry when a namespace peer next subscribes.
        node_client.queue_membership_republish(namespace_id, signed);
    }
    Ok(_) => {}
    Err(e) => warn!(?e, "failed to apply/publish MemberJoined locally (non-fatal)"),
}
```

Trim the existing 8-line comment above the op construction to two lines: keep the reason the local apply is unconditional, drop the rest.

`queue_membership_republish` is a thin `NodeClient` method that does `readiness_addr.do_send(PendingRepublish { .. })`. Add it next to the existing readiness-related methods on `NodeClient`; if `NodeClient` has no readiness handle, send via the same route `subscriptions.rs` uses to reach `manager.readiness_addr` and adjust accordingly.

- [ ] **Step 8: Build and test**

```bash
cargo test -p calimero-node -p calimero-context
cargo clippy -p calimero-node -p calimero-context -- -A warnings
```

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/node/src/readiness.rs crates/context/src/handlers/join_group.rs crates/node/primitives/src/client.rs
git commit -m "fix(context): retry the membership broadcast when it reached no peer"
```

---

### Task 4: Add the optional admission proof to the readiness beacon

**Files:**
- Modify: `crates/governance-types/src/wire.rs:101-160`
- Test: `crates/governance-types/src/wire.rs` test module

**Interfaces:**
- Consumes: `SignedGroupOpenInvitation`.
- Produces: `SignedReadinessBeacon.admission_proof: Option<SignedGroupOpenInvitation>` and the same field on `SignableReadinessBeacon`.

- [ ] **Step 1: Write the failing tests**

Add to the `wire.rs` test module, alongside the existing `signed_readiness_beacon_*` tamper tests:

```rust
#[test]
fn beacon_signature_covers_admission_proof() {
    let sk = PrivateKey::random(&mut rand::thread_rng());
    let ns: NamespaceId = [3u8; 32].into();
    let mut beacon = signed_beacon_with_proof(&sk, ns, Some(test_invitation(&sk, ns)));

    assert!(beacon.verify_signature().is_ok());

    beacon.admission_proof = Some(test_invitation(&sk, [9u8; 32].into()));
    assert!(
        beacon.verify_signature().is_err(),
        "swapping the proof must invalidate the beacon"
    );
}

#[test]
fn beacon_without_proof_round_trips() {
    let sk = PrivateKey::random(&mut rand::thread_rng());
    let beacon = signed_beacon_with_proof(&sk, [3u8; 32].into(), None);
    let bytes = borsh::to_vec(&beacon).unwrap();
    let decoded: SignedReadinessBeacon = borsh::from_slice(&bytes).unwrap();

    assert!(decoded.admission_proof.is_none());
    assert!(decoded.verify_signature().is_ok());
}

#[test]
fn old_beacon_layout_rejects_new_encoding() {
    // Pins the mixed-version contract: an un-upgraded node must fail
    // closed on a new-format beacon rather than misparse it.
    #[derive(BorshDeserialize)]
    #[allow(dead_code)]
    struct OldBeacon {
        namespace_id: NamespaceId,
        peer_pubkey: PublicKey,
        dag_head: [u8; 32],
        applied_through: u64,
        ts_millis: u64,
        strong: bool,
        signature: [u8; 64],
    }

    let sk = PrivateKey::random(&mut rand::thread_rng());
    let ns: NamespaceId = [3u8; 32].into();
    let new_bytes = borsh::to_vec(&signed_beacon_with_proof(
        &sk,
        ns,
        Some(test_invitation(&sk, ns)),
    ))
    .unwrap();

    assert!(borsh::from_slice::<OldBeacon>(&new_bytes).is_err());
}
```

Add `signed_beacon_with_proof` as a test helper by extending the existing `signed_beacon` helper in that module with a proof parameter, rather than writing a second builder.

- [ ] **Step 2: Run the tests**

```bash
cargo test -p calimero-governance-types beacon
```

Expected: FAIL, `no field named admission_proof`.

- [ ] **Step 3: Add the field**

Add to both `SignableReadinessBeacon` and `SignedReadinessBeacon`, positioned immediately before `signature` so `signature` stays last:

```rust
    pub admission_proof: Option<SignedGroupOpenInvitation>,
```

Update `to_signable()` to carry it (`admission_proof: self.admission_proof.clone()`). Update the doc comment on `SignableReadinessBeacon` from "all six fields" to "all seven fields".

Fix every construction site the compiler reports; the known ones are `crates/node/src/readiness.rs:690` and the `wire.rs` and `governance_broadcast` test helpers.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p calimero-governance-types
cargo test --workspace
```

Expected: PASS across the workspace.

- [ ] **Step 5: Commit**

```bash
git add crates/governance-types/src/wire.rs
git commit -m "feat(governance-types): carry an optional admission proof on the readiness beacon"
```

---

### Task 5: Accept a provable non-member beacon as a pull trigger

**Files:**
- Modify: `crates/governance-store/src/governance_broadcast.rs:193-201`
- Modify: `crates/node/src/handlers/network_event/readiness.rs:79-90`
- Test: `crates/governance-store/src/governance_broadcast/tests.rs`

**Interfaces:**
- Consumes: `verify_open_invitation_signature`, `require_inviter_permission`, `ReentryRepository::is_invitation_consumed`, `MembershipRepository::is_admin_or_has_capability`.
- Produces: `pub fn beacon_admission_provable(store: &Store, beacon: &SignedReadinessBeacon) -> bool`

`verify_readiness_beacon` keeps its exact current meaning and return type: true only for a beacon whose signer is already a known member. The new function is consulted only on the drop path, and it never writes membership. It only decides whether pulling from this peer is worth doing.

- [ ] **Step 1: Write the failing tests**

Add to `crates/governance-store/src/governance_broadcast/tests.rs`, next to the existing `verify_readiness_beacon_*` tests:

```rust
#[tokio::test]
async fn admission_provable_accepts_valid_invitation_from_non_member() {
    let store = empty_store();
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let ns: NamespaceId = [42u8; 32].into();
    plant_namespace_member(&store, ns, &admin_sk.public_key());

    let inv = invitation_from(&admin_sk, ns, &joiner_sk.public_key());
    let beacon = signed_beacon_with_proof(&joiner_sk, ns, Some(inv));

    assert!(!verify_readiness_beacon(&store, &beacon), "still not a member");
    assert!(beacon_admission_provable(&store, &beacon));
}

#[tokio::test]
async fn admission_provable_rejects_missing_proof() {
    let store = empty_store();
    let sk = PrivateKey::random(&mut rand::thread_rng());
    let beacon = signed_beacon_with_proof(&sk, [42u8; 32].into(), None);
    assert!(!beacon_admission_provable(&store, &beacon));
}

#[tokio::test]
async fn admission_provable_rejects_wrong_namespace() {
    let store = empty_store();
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let ns: NamespaceId = [42u8; 32].into();
    plant_namespace_member(&store, ns, &admin_sk.public_key());

    let inv = invitation_from(&admin_sk, [43u8; 32].into(), &joiner_sk.public_key());
    let beacon = signed_beacon_with_proof(&joiner_sk, ns, Some(inv));

    assert!(!beacon_admission_provable(&store, &beacon));
}

#[tokio::test]
async fn admission_provable_rejects_unauthorised_inviter() {
    let store = empty_store();
    let stranger_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let ns: NamespaceId = [42u8; 32].into();

    let inv = invitation_from(&stranger_sk, ns, &joiner_sk.public_key());
    let beacon = signed_beacon_with_proof(&joiner_sk, ns, Some(inv));

    assert!(!beacon_admission_provable(&store, &beacon));
}

#[tokio::test]
async fn admission_provable_rejects_proof_for_another_identity() {
    let store = empty_store();
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let other_sk = PrivateKey::random(&mut rand::thread_rng());
    let ns: NamespaceId = [42u8; 32].into();
    plant_namespace_member(&store, ns, &admin_sk.public_key());

    let inv = invitation_from(&admin_sk, ns, &other_sk.public_key());
    let beacon = signed_beacon_with_proof(&joiner_sk, ns, Some(inv));

    assert!(
        !beacon_admission_provable(&store, &beacon),
        "an invitation admitting a different key must not vouch for this signer"
    );
}

#[tokio::test]
async fn admission_provable_rejects_consumed_nonce() {
    let store = empty_store();
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let ns: NamespaceId = [42u8; 32].into();
    plant_namespace_member(&store, ns, &admin_sk.public_key());

    let inv = invitation_from(&admin_sk, ns, &joiner_sk.public_key());
    ReentryRepository::new(&store)
        .mark_invitation_consumed(&ns.into(), inv.invitation.invitation_nonce)
        .expect("mark consumed");
    let beacon = signed_beacon_with_proof(&joiner_sk, ns, Some(inv));

    assert!(
        !beacon_admission_provable(&store, &beacon),
        "a spent invitation must not vouch for a replayed beacon"
    );
}
```

`invitation_from` builds a `SignedGroupOpenInvitation` for a given namespace admitting a given public key. Reuse the invitation builder that `create_group_invitation` tests already use if one exists; otherwise add it as a single test helper in this module.

- [ ] **Step 2: Run the tests**

```bash
cargo test -p calimero-governance-store admission_provable
```

Expected: FAIL, `cannot find function beacon_admission_provable`.

- [ ] **Step 3: Implement**

In `crates/governance-store/src/governance_broadcast.rs`, below `verify_readiness_beacon`:

```rust
/// Whether a non-member beacon carries an invitation that proves admission.
/// Grants no membership; it only makes pulling from this peer worthwhile.
#[must_use]
pub fn beacon_admission_provable(store: &Store, beacon: &SignedReadinessBeacon) -> bool {
    if beacon.verify_signature().is_err() {
        return false;
    }
    let Some(inv) = beacon.admission_proof.as_ref() else {
        return false;
    };
    if inv.invitation.namespace_id != beacon.namespace_id {
        return false;
    }
    if NamespaceMembership::verify_open_invitation_signature(inv).is_err() {
        return false;
    }
    if inv.invitation.invitee_identity != beacon.peer_pubkey {
        return false;
    }
    let inviter = PublicKey::from(inv.invitation.inviter_identity.to_bytes());
    if !MembershipRepository::new(store)
        .is_admin_or_has_capability(
            &beacon.namespace_id.into(),
            &inviter,
            MemberCapabilities::CAN_INVITE_MEMBERS.bits(),
        )
        .unwrap_or(false)
    {
        return false;
    }
    !ReentryRepository::new(store)
        .is_invitation_consumed(
            &beacon.namespace_id.into(),
            inv.invitation.invitation_nonce,
        )
        .unwrap_or(true)
}
```

Field names on `SignedGroupOpenInvitation` and its inner `invitation` must be read from the type definition before writing this; `invitee_identity` and `invitation_nonce` are the expected names but confirm them. Adjust the calls to `is_invitation_consumed` to the real signature.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p calimero-governance-store
```

Expected: PASS.

- [ ] **Step 5: Wire the pull trigger**

In `crates/node/src/handlers/network_event/readiness.rs`, the current drop branch is:

```rust
if !verify_readiness_beacon(&manager.datastore, &beacon) {
    debug!(namespace_id = ..., "ReadinessBeacon failed verification; dropping");
    return;
}
```

Change it to fall through to the provable path before dropping:

```rust
if !verify_readiness_beacon(&manager.datastore, &beacon) {
    if beacon_admission_provable(&manager.datastore, &beacon) {
        // Not a known member yet, but admission is provable: pull rather
        // than trust the beacon's contents.
        trigger_provable_peer_sync(manager, ctx, beacon.namespace_id);
    }
    return;
}
```

`trigger_provable_peer_sync` reuses the existing per-namespace debounce already present in this file for beacon-triggered syncs (`debounce_allows_sync` / `NS_BEACON_SYNC_DEBOUNCE`) and then calls `node_client.sync_namespace(namespace_id)`. Do not add a second debounce map.

Note that the beacon's contents are still never written to the `ReadinessCache` on this path.

- [ ] **Step 6: Build and test**

```bash
cargo test -p calimero-node -p calimero-governance-store
cargo clippy -p calimero-node -p calimero-governance-store -- -A warnings
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/governance-store/src/governance_broadcast.rs crates/governance-store/src/governance_broadcast/tests.rs crates/node/src/handlers/network_event/readiness.rs
git commit -m "fix(governance): pull from a non-member peer that can prove admission"
```

---

### Task 6: Attach the proof while membership is unconfirmed

**Files:**
- Modify: `crates/node/src/readiness.rs:686-700`
- Test: `crates/node/src/readiness.rs` test module

**Interfaces:**
- Consumes: `MembershipRepository::namespace_pubkeys`, the invitation stored during join.
- Produces: no new public API.

The proof must ride the beacon only until the group acknowledges the joiner. Steady state stays one byte.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn proof_attached_only_while_unconfirmed() {
    assert!(should_attach_proof(/* self_is_known_member */ false, Instant::now()));
    assert!(!should_attach_proof(true, Instant::now()));
    assert!(!should_attach_proof(
        false,
        Instant::now() - (REPUBLISH_CAP + Duration::from_secs(1))
    ));
}
```

- [ ] **Step 2: Run it**

```bash
cargo test -p calimero-node proof_attached_only_while_unconfirmed
```

Expected: FAIL, `cannot find function should_attach_proof`.

- [ ] **Step 3: Implement**

```rust
fn should_attach_proof(self_is_known_member: bool, joined_at: Instant) -> bool {
    !self_is_known_member && Instant::now().duration_since(joined_at) < REPUBLISH_CAP
}
```

In the beacon construction at line 690, set `admission_proof` from the `pending_republish` registry Task 3 added, gated on `should_attach_proof`. That map is already the source of truth for "we joined and are unconfirmed" and already carries the invitation, so no second registry is needed:

```rust
let admission_proof = self
    .pending_republish
    .get(&ns_id)
    .filter(|p| should_attach_proof(self_is_known_member, p.queued_at))
    .map(|p| p.invitation.clone());
```

Derive `self_is_known_member` from `MembershipRepository::new(&self.datastore).namespace_pubkeys(ns_id.into())` containing `peer_pubkey`. When it becomes true, drop the entry so the proof stops riding subsequent beacons and the republish stops being retried.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p calimero-node
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/node/src/readiness.rs
git commit -m "feat(node): attach the admission proof until membership is confirmed"
```

---

### Task 7: Harness honesty

**Files:**
- Modify: `apps/scaffolding-e2e/workflows/group-join-mesh-not-ready.yml:145-170`
- Modify: `.github/actions/setup-merobox/action.yml:49-50`

**Interfaces:** none.

- [ ] **Step 1: Remove the dead trigger and correct the comment**

Delete the `trigger_sync: true` line from the `wait_for_sync` step. merobox only issues a sync RPC for `context` targets, so on a `group_id` target it is a no-op that misdescribes the step.

Replace the 13-line justification comment with two lines stating what the step is:

```yaml
  # Convergence must happen unaided: a real user does not trigger a sync.
  # Poll both nodes' group hash for up to 120s and require them to agree.
```

- [ ] **Step 2: Pin merobox**

Change the install specifier from `merobox>=0.6.43` to `merobox==0.6.43` so CI behaviour cannot change without a commit.

- [ ] **Step 3: Verify the scenario still parses**

```bash
merobox bootstrap run apps/scaffolding-e2e/workflows/group-join-mesh-not-ready.yml --dry-run 2>&1 | tail -5 || \
  merobox bootstrap validate apps/scaffolding-e2e/workflows/group-join-mesh-not-ready.yml
```

If neither flag exists, run the scenario once and confirm it reaches the `wait_for_sync` step.

- [ ] **Step 4: Commit**

```bash
git add apps/scaffolding-e2e/workflows/group-join-mesh-not-ready.yml .github/actions/setup-merobox/action.yml
git commit -m "test(e2e): drop dead trigger_sync and pin merobox"
```

---

### Task 8: Validate end to end

**Files:** none.

- [ ] **Step 1: Full workspace gate**

```bash
cargo fmt --check
cargo clippy --workspace -- -A warnings
cargo test --workspace
```

Expected: all pass.

- [ ] **Step 2: Rebuild the node image**

```bash
docker build -t merod:local -f Dockerfile .
```

- [ ] **Step 3: Run the scenario 20 times**

```bash
for i in $(seq 1 20); do
  if merobox bootstrap run apps/scaffolding-e2e/workflows/group-join-mesh-not-ready.yml >/tmp/fix_$i.log 2>&1;
  then echo "$i PASS"; else echo "$i FAIL"; fi
  merobox stop --all >/dev/null 2>&1
done | tee /tmp/fixed.txt
grep -c PASS /tmp/fixed.txt
```

Expected: 20 of 20.

- [ ] **Step 4: If any run fails, do not raise the retry count**

Pull the node logs for the failing iteration and check the known predictor:

```bash
grep -c "triggering proactive backfill" docker-logs/*node-1.log
grep -c "ReadinessBeacon failed verification" docker-logs/*node-1.log
grep -c "acks=0" docker-logs/*node-2.log
```

A failure with none of the three fixes engaged means a third failure class exists that the design does not cover. Stop and reopen the investigation rather than adjusting timeouts.

- [ ] **Step 5: Run the neighbouring scenarios for regressions**

The beacon and redial paths are shared. Run the scenarios most likely to be affected:

```bash
for wf in apps/scaffolding-e2e/workflows/group-*.yml workflows/sync-tests/*.yml; do
  echo "== $wf"; merobox bootstrap run "$wf" >/tmp/reg.log 2>&1 && echo PASS || echo FAIL
  merobox stop --all >/dev/null 2>&1
done
```

Expected: no scenario that passed on the baseline now fails.

- [ ] **Step 6: Record the before/after numbers**

Write both counts into the PR description: baseline from Task 0 Step 5, fixed from Step 3 above. The repository's definition of done requires reproduction plus before/after evidence.

---

## Edge cases covered by the tests above

| Case | Task |
|---|---|
| Un-upgraded node receives a new-format beacon | 4, `old_beacon_layout_rejects_new_encoding` |
| Proof swapped for a different namespace's invitation | 4, `beacon_signature_covers_admission_proof` |
| Beacon with no proof, steady state | 4, `beacon_without_proof_round_trips` |
| Invitation issued by someone without invite rights | 5, `admission_provable_rejects_unauthorised_inviter` |
| Invitation admitting a different key than the signer | 5, `admission_provable_rejects_proof_for_another_identity` |
| Invitation for the wrong namespace | 5, `admission_provable_rejects_wrong_namespace` |
| Already-consumed invitation nonce | 5, `admission_provable_rejects_consumed_nonce` |
| Republish entry outliving its cap | 3, `pending_republish_expires_after_cap` |
| Proof still attached after confirmation | 6, `proof_attached_only_while_unconfirmed` |
| mDNS peer loses its redial path | 1, `mdns_discovered_peer_retains_direct_addrs_for_redial` |

## Not in scope

- Making namespace sync bidirectional.
- Restoring active catch-up arms to the namespace heartbeat.
- Any change that lets the E2E scenario converge with manual help.
