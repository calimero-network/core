# RFC: Network Event Delivery Architecture

> **📖 Part of the Sync Protocol documentation.** See [SYNC-PROTOCOL-INDEX.md](./SYNC-PROTOCOL-INDEX.md) for the full index.

**Date**: January 31, 2026  
**Status**: Discussion Draft  
**Authors**: Calimero Team  
**Branch**: `test/tree_sync`

---

## TL;DR

During sync protocol work, we discovered that **cross-arbiter message delivery via `LazyRecipient<NetworkEvent>` silently drops messages under load**. We shipped a workaround (dedicated mpsc channel), but the underlying architectural tension between Actix actors and tokio async remains unresolved.

**This RFC proposes we discuss**: Should we migrate away from Actix entirely?

---

## The Problem

### What We Observed

3-node sync tests were failing intermittently. Nodes would miss gossipsub messages and fail to converge. After investigation:

```
NetworkManager (Arbiter A) ─── LazyRecipient ──→ NodeManager (Arbiter B)
                                    │
                                    └── MESSAGES SILENTLY DROPPED
```

**Symptoms**:
- No errors logged
- No backpressure signals
- Messages simply vanished
- More likely under higher message rates

### Root Cause Analysis

`LazyRecipient<M>` is Actix's mechanism for cross-arbiter actor communication. It:
1. Resolves the target actor address lazily
2. Uses internal channels to bridge arbiters
3. **Has undocumented buffering/dropping behavior**

We couldn't find:
- Capacity limits documented
- Drop conditions documented
- Metrics on internal buffer state

### The Workaround We Shipped

```rust
// BEFORE (broken)
NetworkManager → LazyRecipient<NetworkEvent> → NodeManager

// AFTER (workaround)
NetworkManager → mpsc::channel(1000) → NetworkEventBridge → NodeManager
                                              │
                              (tokio task that polls channel
                               and sends to Actix actor)
```

**New components**:
- `NetworkEventChannel` - tokio mpsc with Prometheus metrics
- `NetworkEventBridge` - tokio task that forwards to Actix
- Explicit backpressure (channel full = log warning)
- Explicit drops (counter incremented, not silent)

---

## Current Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Current State                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌──────────────────────┐      ┌──────────────────────┐                     │
│  │   NetworkManager     │      │    NodeManager       │                     │
│  │   (libp2p + Actix)   │      │    (Actix Actor)     │                     │
│  │                      │      │                      │                     │
│  │  - Swarm polling     │      │  - Context mgmt      │                     │
│  │  - Gossipsub         │      │  - Sync orchestration│                     │
│  │  - Stream handling   │      │  - Delta processing  │                     │
│  └──────────┬───────────┘      └──────────▲───────────┘                     │
│             │                             │                                  │
│             │  mpsc channel               │  Actix messages                  │
│             │  (our workaround)           │  (works within arbiter)          │
│             ▼                             │                                  │
│  ┌──────────────────────┐                 │                                  │
│  │  NetworkEventBridge  │─────────────────┘                                  │
│  │  (tokio task)        │                                                    │
│  └──────────────────────┘                                                    │
│                                                                             │
│  Problems:                                                                   │
│  • Two message systems (Actix + channels)                                   │
│  • Bridge adds latency + complexity                                         │
│  • Actix actor model not fully utilized                                     │
│  • Mixed runtimes (Actix runtime + tokio)                                   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Options for Discussion

### Option A: Keep Current Workaround

**Do nothing more. Channel bridge works.**

| Pros | Cons |
|------|------|
| Already shipped | Technical debt remains |
| Tests pass | Two messaging paradigms |
| Low immediate effort | Confusing for new developers |
| | Actix still used elsewhere |

**Effort**: 0  
**Risk**: Low (for now)  
**Recommendation**: Acceptable for short term

---

### Option B: Migrate Fully to Actix

**Investigate and fix LazyRecipient properly. Embrace Actix.**

| Pros | Cons |
|------|------|
| Single paradigm | LazyRecipient behavior unclear |
| Actor model benefits | May require Actix upstream changes |
| Less code (remove bridge) | Actix ecosystem shrinking |
| | Still mixed with tokio for libp2p |

**Effort**: Medium (2-3 weeks investigation + fix)  
**Risk**: Medium (may hit dead ends)  
**Recommendation**: Only if Actix expertise available

---

### Option C: Migrate Away from Actix

**Replace Actix actors with tokio tasks + channels.**

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Proposed: Pure Tokio                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌──────────────────────┐                  ┌──────────────────────┐         │
│  │   NetworkService     │                  │    NodeService       │         │
│  │   (tokio task)       │  ───channel───►  │    (tokio task)      │         │
│  │                      │                  │                      │         │
│  │  - Swarm polling     │                  │  - Context mgmt      │         │
│  │  - Event dispatch    │                  │  - Sync orchestration│         │
│  └──────────────────────┘                  └──────────────────────┘         │
│                                                                             │
│  Benefits:                                                                   │
│  • Single runtime (tokio)                                                   │
│  • Explicit channels (debuggable)                                           │
│  • No actor address resolution                                              │
│  • Standard async/await patterns                                            │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

| Pros | Cons |
|------|------|
| Single runtime | Large refactor |
| Explicit control flow | Migration risk |
| Better tooling (tokio-console) | Server handlers still Actix? |
| Growing ecosystem | Team learning curve |
| Easier testing | |

**Effort**: Large (3-5 weeks)  
**Risk**: High (core refactor)  
**Recommendation**: Best long-term, needs planning

---

### Option D: Hybrid with Clear Boundaries

**Keep Actix for HTTP/WS servers, tokio for internal services.**

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Proposed: Hybrid Boundary                             │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────────────────────────────────────────────┐                │
│  │                   Actix Web (HTTP/WS)                   │                │
│  │                   (Keep as-is, works well)              │                │
│  └─────────────────────────────────────────────────────────┘                │
│                              │                                              │
│                              ▼ (channels)                                   │
│  ┌─────────────────────────────────────────────────────────┐                │
│  │                   Tokio Services                         │                │
│  │   NetworkService ←──channel──► NodeService              │                │
│  │       ↓                            ↓                     │                │
│  │   SyncManager ←──channel──► ContextManager              │                │
│  └─────────────────────────────────────────────────────────┘                │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

| Pros | Cons |
|------|------|
| Incremental migration | Still two paradigms |
| Keep working Actix Web | Boundary maintenance |
| Lower risk per phase | Longer total timeline |

**Effort**: Medium per phase, Large total  
**Risk**: Medium  
**Recommendation**: Pragmatic approach

---

## Questions for Discussion

1. **How critical is the actor model for us?**
   - Do we benefit from actor isolation/supervision?
   - Or is it incidental complexity from early decisions?

2. **What's the Actix expertise level on the team?**
   - Can someone debug LazyRecipient internals?
   - Or are we treating Actix as a black box?

3. **What's the migration appetite?**
   - Is Q2 2026 a good time for core refactoring?
   - Or do we have higher priorities?

4. **Are there other Actix pain points?**
   - Actor lifecycle management?
   - Testing difficulties?
   - Other cross-arbiter issues?

---

## Recommendation

**Short term (now)**: Ship with Option A (workaround in place, tests passing)

**Medium term (Q2 2026)**: Plan Option D (hybrid with clear boundaries)
- Start with NetworkManager → tokio service
- Keep Actix Web for servers
- Incremental, lower risk

**Long term (Q3+ 2026)**: Evaluate Option C based on Q2 learnings

---

## Related

- `crates/node/src/network_event_channel.rs` - Current workaround
- `crates/node/src/network_event_processor.rs` - Bridge implementation
- `BRANCH-CHECKPOINT-2026-01-31.md` - Full context
- `DECISION-LOG.md` - Other architectural decisions

---

*Prepared for internal discussion - January 31, 2026*
