# ADR 0001 — Concurrent rotation semantics for `SharedStorage<T>`

| | |
|---|---|
| **Status** | Proposed |
| **Date** | 2026-04-24 |
| **Deciders** | Calimero core team |
| **Context** | [#2233](https://github.com/calimero-network/core/issues/2233) — DAG-causal Shared verification, phase **P4 (design)** |
| **Constrains** | [#2233](https://github.com/calimero-network/core/issues/2233) phase **P2** (rotation history schema) |

> First ADR in this repo. Convention: `docs/adr/NNNN-kebab-title.md`, monotonically numbered, "Proposed → Accepted → Superseded" lifecycle.

## Context

`SharedStorage<T>` is group-writable storage with a mutable writer set. Any current writer can rotate the writer set via `rotate_writers(new_writers)`. v2 (#2230) ships with monotonic-nonce + last-write-wins on `writers_nonce`; #2233 replaces that with DAG-causal verification.

When two current writers issue rotations *concurrently* (siblings in the DAG, neither in the other's causal history, both signed by valid pre-rotation writers), the merge has to pick a deterministic outcome. v2 picks LWW by `writers_nonce`; that loses information. Before P2 designs the rotation-history schema, we need to know what merge rule we're storing for, because the rule constrains the schema.

This ADR captures the rule and the rejected alternatives.

### Concurrent vs. causal: definitions used in this ADR

For two rotation actions `R₁` and `R₂` on the same entity, contained in causal deltas `D₁` and `D₂`:

- **`R₁ happens-before R₂`** iff `D₁.id` is in the transitive `parents` set of `D₂` (the DAG-causal "knows about" relation).
- **Concurrent** iff neither `R₁ happens-before R₂` nor `R₂ happens-before R₁`.

These are the standard CRDT terms; we use them in the strict DAG-structural sense, not wall-clock time.

## Decision

**Causal-first LWW, with HLC tiebreak, with signer-pubkey tiebreak.**

Given two rotations `R₁` (in delta `D₁`) and `R₂` (in delta `D₂`) on the same entity:

1. **Causal precedence wins, unconditionally.** If `R₁ happens-before R₂`, then `R₂` wins. The author of `R₂` saw `R₁`'s reality and chose to rotate from that state — overriding `R₂` would discard a writer's informed decision.
2. **For truly concurrent rotations**, the rotation with the **larger `D.hlc`** wins. `HybridTimestamp` is `Ord` and embeds both NTP64 wall-time and a node `ID`, giving a deterministic total order across nodes.
3. **HLC tiebreak (vanishingly rare)** by lexicographic comparison of the signing writer's pubkey bytes — smaller bytes win. Spelled out for spec completeness; practically unreachable because HLC's embedded node ID already disambiguates same-instant events from different nodes.

The same rule resolves writes (`insert`) by analogy: causal-first, then HLC, then signer-hash. Rotations and value writes use the same merge ordering.

### Worked examples

> Notation: `Wₐ`, `W_b` are writers. `(setₓ)` is the post-rotation writer set. Arrows `→` denote DAG edges (parent → child). Times in brackets are HLCs.

**Example A — sequential, no race.**
```
[t=10] D₁ by Wₐ: rotate → (Wₐ, W_b)
   ↓
[t=20] D₂ by W_b: rotate → (W_b, W_c)
```
`D₁ happens-before D₂`. Result: `(W_b, W_c)`. (No change vs. v2.)

**Example B — concurrent siblings, different intent.**
```
        [t=10] D_root: writers = (Wₐ, W_b)
              ↓             ↓
[t=20, Wₐ] D₁: → (Wₐ)    [t=21, W_b] D₂: → (W_b)
   (Wₐ rotates W_b out)    (W_b rotates Wₐ out)
```
Concurrent. HLC: `D₂` (21) > `D₁` (20). Result: `(W_b)`. `Wₐ` is rotated out; `Wₐ`'s rotation that removed `W_b` is discarded.

This is the *expected information loss* of any non-union rule — and is why option 2 (union-with-re-election) exists. We accept the loss; see "Consequences" below.

**Example C — concurrent siblings, same HLC (exotic).**
```
        D_root: writers = (Wₐ, W_b)
              ↓             ↓
[t=20, Wₐ] D₁: → (Wₐ, W_c)   [t=20, W_b] D₂: → (W_b, W_d)
```
HLC tie. Tiebreak by signer pubkey bytes: assume `bytes(Wₐ) < bytes(W_b)` lexicographically → `D₁` wins → result `(Wₐ, W_c)`.

In practice, `HybridTimestamp` includes a node ID, so two distinct nodes producing identical HLCs at the same nanosecond requires the IDs to also collide — astronomically unlikely. The tiebreak is for spec completeness, not real-world frequency.

**Example D — concurrent rotation vs. concurrent value write.**
```
        D_root: value = "hello", writers = (Wₐ, W_b)
              ↓             ↓
[t=20, Wₐ] D₁: rotate → (Wₐ)   [t=21, W_b] D₂: insert("world")
```
The value-write action (D₂'s insert) and the rotation action (D₁) target the same entity but are different action kinds. Resolved independently:
- **Writer set**: only `D₁` rotates → `(Wₐ)`.
- **Value**: `D₂` writes "world" with HLC 21; later HLC wins → "world".

Result: writers `(Wₐ)`, value `"world"`. `W_b`'s write is *kept* even though `W_b` is no longer a writer post-merge — `W_b` was authoritatively a writer at the time `D₂` was authored (siblings of `D_root`), and the verifier validates against `writers_at(entity, D₂.parents)`, not against the post-merge writer set. (See [#2233 epic constraints](https://github.com/calimero-network/core/issues/2233).)

This is the central guarantee that makes concurrent operation safe: a write signed by the writer-set-as-of-the-author's-causal-view never gets retroactively rejected by a later rotation.

## Alternatives considered

### A1 — LWW by `writers_nonce` *(what v2 ships)*

| Pro | Con |
|---|---|
| Simple, already implemented | Loses one rotation's intent on every concurrent rotation |
| No DAG infra needed | `writers_nonce` collisions on concurrent siblings — falls back to BTreeSet sort, fully arbitrary |
| | Doesn't address the four #2197 partition scenarios |

**Rejected** because it doesn't deliver the correctness improvement that motivates #2233. If the answer is "LWW is fine," the entire epic is unnecessary.

### A2 — Writer-set union with re-election

Apply both rotations, take the union, mark the entity as "needs re-election." A current writer must issue a signed re-election to reduce the set.

| Pro | Con |
|---|---|
| No information loss — both rotations preserved | UX disaster: every concurrent rotation requires a follow-up coordination step |
| Theoretically the most "CRDT-pure" choice | Forces every app to handle a "pending re-election" UI state |
| | Storage cost: P2 must carry intermediate union state |
| | If no re-election ever happens, the union persists indefinitely — *worse* than LWW because a malicious writer added by an attacker stays in until manually removed |
| | Double-spend / safety windows: between rotation and re-election, the union has more writers than either author intended |

**Rejected.** The intent-preservation upside doesn't justify the operational complexity. Apps that need this can implement it on top of the chosen rule (commit-then-re-elect via two `rotate_writers` calls) without the storage layer mandating it.

### A3 — Signer-pubkey hash tiebreak (alone)

| Pro | Con |
|---|---|
| Deterministic | Same intent loss as LWW |
| | Doesn't use causal information at all |

**Rejected as the primary rule.** Adopted as the *final* tiebreak in our chosen rule (after HLC) because it's a clean deterministic floor.

### A4 — First-causal-parent-wins *(closest to chosen rule)*

The original framing in the #2233 epic: "the rotation whose nearest common DAG ancestor is causally earliest wins."

| Pro | Con |
|---|---|
| Uses DAG infra naturally | The "nearest common ancestor" is the same delta for any two siblings — degenerates to needing a separate tiebreak anyway |
| | Underspecified for the sibling case (which is exactly the hard case) |

**Refined into the chosen rule.** The chosen rule keeps the spirit ("causal first") and replaces the underspecified ancestor comparison with concrete HLC + pubkey tiebreaks for true siblings.

### A5 — Reject concurrent rotations

The entity refuses to apply a rotation if its current writer set has already moved past the rotation's claimed view. The losing writer must re-fetch state and re-issue.

| Pro | Con |
|---|---|
| Strong intent preservation (winning rotation always reflects its author's full intent) | Breaks the "eventually converges without app intervention" CRDT guarantee |
| | Two co-admins rotating at the same time → both rotations *could* fail in pathological reorderings, requiring app retry logic |
| | Adversarial writers can DoS rotation by spamming concurrent attempts |
| | Nodes processing the rejection have to surface "rotation rejected, please retry" to userland — every SharedStorage user has to handle this |

**Rejected.** Calimero's model is "sync converges silently, no app retries needed for ordinary CRDT merges." Rotation should not be a special case where convergence requires application-level retry.

## Consequences

### What P2 needs to store

The chosen rule is **stateless beyond a per-entity rotation log** sorted by causal order. No intermediate "pending re-election" state, no union accumulation. P2 schema (option a in the epic) is sufficient:

```text
RotationLogEntry {
    delta_id:        [u8; 32],   // CausalDelta.id where the rotation lives
    delta_hlc:       HybridTimestamp,  // for sibling tiebreak
    signer:          PublicKey,        // for HLC-tie tiebreak
    new_writers:     BTreeSet<PublicKey>,
    writers_nonce:   u64,         // kept for v2 compat / debugging only
}
```

`writers_at(entity_id, causal_parents)` walks the log filtered to entries whose `delta_id` is in `causal_parents`' transitive ancestor set, picks the *latest* by the rule above, and returns its `new_writers`.

If `causal_parents` is empty (P1 codepath, snapshot leaf push, local apply), `writers_at` returns the most recently appended entry — matches v2 LWW behavior, preserves backward compatibility.

### What P3 (verifier) does with it

The verifier validates a Shared action's signature against `writers_at(action.entity_id, ctx.causal_parents)` — *not* against the currently stored writer set. This is what makes Example D safe.

The v2 `signer: Option<PublicKey>` hint on `SignatureData` (already shipped, dormant) is validated against the same `writers_at(...)` set — not current. Spelled out in the #2233 epic; reaffirmed here.

### What about pure value writes (insert)?

The same rule extends without change: causal-first, HLC second, signer-hash third. P2 doesn't need a separate value-write history; the existing CRDT-merge layer (LWW per slot for `LwwRegister`, etc.) already handles this — only the *verifier* needs to know `writers_at(entity, causal_parents)` to validate signatures, not to merge values.

### What we accept

- **One rotation's intent can be lost** in the truly-concurrent case (Example B). This is unavoidable without union-with-re-election (A2), and we judged A2's complexity worse than the loss.
- **Out-of-order delivery is fine.** Two nodes seeing `D₁` then `D₂` vs. `D₂` then `D₁` converge to the same result because the rule depends only on (causal relation, HLC, signer hash) — all immutable per-delta.
- **HLC clock skew matters.** A writer with a fast clock can win all concurrent races. This is the standard LWW concern; mitigated by the existing HLC + drift-tolerance check (`DRIFT_TOLERANCE_NANOS`). If clock-skew abuse becomes a real threat, A2 (union-with-re-election) is the escape hatch — schema can be extended without a wire-format break since the rotation log is local-only.

### What we explicitly do NOT decide here

- **Compaction policy.** The epic specifies "1000-entry sliding window + snapshot." That number belongs in P6 measurements; the *shape* (sliding window with a snapshot of the writer set at the boundary) is locked in here so P2 can lay out the index schema.
- **Re-election as an opt-in feature.** Apps that want union-with-re-election can build it: `rotate_writers(union)` followed by a coordinated `rotate_writers(reduced)`. The storage layer doesn't need to know.
- **Bootstrap race.** Listed as item 4 in #2197 / #2233. Not addressed by this ADR — bootstrap concurrency happens *before* there's a shared causal ancestor, so causal-first doesn't apply. P5/P6 is the right place; tracked separately.

## References

- [#2197](https://github.com/calimero-network/core/issues/2197) — original SharedStorage spec, partition scenarios
- [#2230](https://github.com/calimero-network/core/issues/2230) — v2 follow-ups (signer hint, rotate-self-out)
- [#2233](https://github.com/calimero-network/core/issues/2233) — DAG-causal verification epic (this ADR is phase P4-design)
- [#2249](https://github.com/calimero-network/core/pull/2249) — v2 PR (signer hint plumbing referenced above)
