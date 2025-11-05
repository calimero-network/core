# Migration Plan - From Old to New Architecture

## 🎯 Goal

Safely migrate from actor-based code to new stateless architecture.

---

## 📊 Current State Analysis

### What's NEW (Keep):
```
✅ crates/protocols/        - Stateless protocols (KEEP!)
✅ crates/sync/             - Sync orchestration (KEEP!)
✅ crates/node/runtime/     - New runtime (KEEP!)
✅ crates/node/services/    - Extracted services (KEEP!)
✅ crates/context/repository.rs - Extracted repository (KEEP!)
✅ crates/context/application_manager.rs - Extracted manager (KEEP!)
```

### What's OLD (Can Delete):
```
❌ crates/node/src/sync/    - OLD sync code (replaced by protocols + sync crates)
   ├── manager.rs           - (1,088 lines) → Replaced by runtime/event_loop.rs + sync crate
   ├── key.rs              - (113 lines) → Replaced by protocols/p2p/key_exchange.rs
   ├── blobs.rs            - (263 lines) → Replaced by protocols/p2p/blob_request.rs
   ├── delta_request.rs    - (420 lines) → Replaced by protocols/p2p/delta_request.rs
   ├── stream.rs           - (85 lines) → Replaced by protocols/stream/helpers.rs
   ├── secure_stream.rs    - (856 lines) → Replaced by protocols/stream/authenticated.rs
   ├── tracking.rs         - (143 lines) → Moved to protocols/stream/tracking.rs
   └── helpers.rs          - (27 lines) → Merged into protocols/stream/helpers.rs

❌ crates/node/src/handlers/state_delta.rs - (765 lines) → Replaced by protocols/gossipsub/state_delta.rs
```

### What Needs Migration (Update):
```
⚠️  crates/node/src/run.rs - Update to use new runtime
⚠️  crates/node/src/lib.rs - Export new runtime, deprecate old
```

### Documentation to Cleanup:
```
🗑️ ARCHITECTURAL_PROBLEMS.md - Analysis doc (can delete after migration)
🗑️ HONEST_ASSESSMENT.md - Analysis doc (can delete)
🗑️ WHAT_NODE_NEEDS.md - Requirements doc (can delete)
🗑️ NODE_REFACTORING_PLAN.md - Old plan (superseded)
🗑️ SESSION_SUMMARY.md - Temp summary (consolidated into EPIC_SESSION_SUMMARY.md)
```

---

## 🔄 Migration Strategy

### Phase 1: Wire New Runtime (NOW)
1. ✅ Create runtime module structure
2. ⏳ Update network layer to use runtime channels
3. ⏳ Wire handlers to use protocols instead of old sync code

### Phase 2: Delete Old Code (NEXT)
1. Delete `crates/node/src/sync/` directory (entire old sync module)
2. Delete `crates/node/src/handlers/state_delta.rs` (replaced by protocol)
3. Update imports across the codebase

### Phase 3: Cleanup Documentation (AFTER)
1. Delete temporary analysis docs
2. Keep essential architecture docs
3. Update READMEs

### Phase 4: Tests & Polish (FINAL)
1. Add missing tests
2. Add comprehensive documentation
3. Final cleanup

---

## ⚠️ Safe Deletion Checklist

Before deleting old code, verify:
- [ ] New runtime handles all old functionality
- [ ] All tests still pass
- [ ] No references to old code in handlers
- [ ] Network layer wired to new runtime

---

## 🚀 Execution Order

1. **Wire network layer** (listeners → runtime channels)
2. **Update handlers** (use protocols directly)
3. **Delete old sync/** (entire directory)
4. **Delete old state_delta handler**
5. **Cleanup docs** (temp analysis files)
6. **Add tests** (comprehensive coverage)
7. **Add docs** (architecture, usage, migration guide)
8. **Final polish** (linting, formatting, etc)

Let's go! 🚀

