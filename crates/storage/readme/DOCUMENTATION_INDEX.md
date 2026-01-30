# Documentation Index

Complete guide to Calimero Storage CRDT documentation.

---

## Start Here

**New to CRDTs?** → [Main README](../README.md)  
**Want to code?** → [Collections API](collections.md)  
**Need examples?** → [Main README Examples](../README.md#examples)

---

## For Developers

### Getting Started
1. **[Main README](../README.md)** - Overview, quick start with `#[derive(Mergeable)]`, examples
2. **[Collections API](collections.md)** - Complete API reference + derive macro
3. **[Nesting Guide](nesting.md)** - How to use nested structures + custom structs

### Common Tasks
- **Add a counter:** See [Collections API - Counter](collections.md#counter)
- **Store user data:** See [Collections API - LwwRegister](collections.md#lwwregistert)
- **Build a document editor:** See [Collections API - RGA](collections.md#replicatedgrowablearray-rga)
- **Create nested maps:** See [Nesting Guide](nesting.md#pattern-3-nested-maps-two-levels)
- **Use custom structs:** See [Collections API - #[derive(Mergeable)]](collections.md#using-custom-structs-derivemergeable)

### Troubleshooting
- **App diverges:** Check root fields are CRDTs ([Migration Guide](migration.md))
- **Merge too slow:** See [Performance Guide](performance.md#optimization-tips)
- **Not sure which collection:** See [Collections API - Decision Tree](collections.md#quick-selection-guide)

---

## For Architects

### Understanding the System
1. **[Architecture](architecture.md)** - How it works internally
2. **[Merging Deep-Dive](merging.md)** - DAG vs explicit merge explained
3. **[Design Decisions](design-decisions.md)** - Why we built it this way
4. **[Network Sync](network-sync.md)** - Efficient synchronization protocols

### Performance
- **[Performance Guide](performance.md)** - Benchmarks, optimization tips
- **[Merging](merging.md#merge-frequency-analysis)** - When merge is called

### Planning
- **[Migration Guide](migration.md)** - Upgrading existing apps
- **[TODO](../../../TODO.md)** - Future enhancements

---

## By Topic

### Conflict Resolution
- [Merging Deep-Dive](merging.md) - Complete explanation
- [Architecture - Layer System](architecture.md#the-three-layer-system)
- [Performance - Merge Costs](performance.md#operation-costs)

### Network Synchronization
- [Network Sync](network-sync.md) - Protocol overview and selection
- [Network Sync - Bloom Filter](network-sync.md#protocol-4-bloom-filter-sync) - Probabilistic diff detection
- [Network Sync - Smart Selection](network-sync.md#protocol-selection-smart-adaptive-sync) - Automatic protocol choice

### Nesting
- [Nesting Guide](nesting.md) - Patterns and examples
- [Collections API - Nesting sections](collections.md) - Per-collection support
- [Performance - Nesting](performance.md#nesting-performance)

### Migration
- [Migration Guide](migration.md) - Complete migration walkthrough
- [Nesting Guide - Anti-Patterns](nesting.md#anti-patterns-what-not-to-do)

### Performance
- [Performance Guide](performance.md) - Complete guide
- [Architecture - Performance Deep-Dive](architecture.md#performance-deep-dive)
- [Merging - Complexity Analysis](merging.md#merge-complexity-analysis)

---

## Quick Links

**Need to:**
- Understand merge? → [Merging Deep-Dive](merging.md)
- Optimize performance? → [Performance Guide](performance.md)
- Migrate app? → [Migration Guide](migration.md)
- Learn API? → [Collections API](collections.md)
- Understand architecture? → [Architecture](architecture.md)
- See design rationale? → [Design Decisions](design-decisions.md)
- Sync nodes efficiently? → [Network Sync](network-sync.md)

---

## Document Organization

```
crates/storage/
├── README.md                    # START HERE - Overview + quick start
├── TODO.md                       # Future work
└── readme/
    ├── DOCUMENTATION_INDEX.md    # This file
    ├── collections.md            # Complete API reference
    ├── nesting.md                # Nesting patterns guide
    ├── architecture.md           # How it works internally
    ├── merging.md                # Conflict resolution explained
    ├── performance.md            # Optimization guide
    ├── migration.md              # Upgrading guide
    ├── design-decisions.md       # Why we built it this way
    └── network-sync.md           # Network synchronization protocols
```

---

## Reading Paths

### Path 1: Quick Start (15 minutes)

1. [Main README](../README.md) - Overview
2. [Collections API](collections.md) - Find your collection
3. Start coding!

### Path 2: Deep Understanding (2 hours)

1. [Main README](../README.md) - Overview
2. [Architecture](architecture.md) - How it works
3. [Merging Deep-Dive](merging.md) - Conflict resolution
4. [Collections API](collections.md) - All collections
5. [Nesting Guide](nesting.md) - Advanced patterns

### Path 3: Production Deployment (1 day)

1. [Main README](../README.md) - Overview
2. [Collections API](collections.md) - API reference
3. [Nesting Guide](nesting.md) - Best practices
4. [Performance Guide](performance.md) - Optimization
5. [Migration Guide](migration.md) - If upgrading
6. Deploy and monitor!

---

## Example Apps

Working examples in `apps/`:

| App | Demonstrates | Use Case |
|-----|--------------|----------|
| **team-metrics-macro** | `#[derive(Mergeable)]` ✨ | Zero-boilerplate custom structs |
| **team-metrics-custom** | Manual `Mergeable` impl | Custom merge logic |
| **nested-crdt-test** | All nesting patterns | Complex nested structures |
| **collaborative-editor** | RGA + counters | Real-time text editing |
| **kv-store** | Basic UnorderedMap | Simple key-value storage |

**Compare approaches:**
- `apps/team-metrics-macro` vs `apps/team-metrics-custom` - Same functionality, different implementation!

---

## External Resources

### CRDT Theory
- ["A Comprehensive Study of CRDTs" (Shapiro et al.)](https://arxiv.org/abs/1011.5808)
- [CRDT.tech](https://crdt.tech/) - Community resources

### Related Systems
- [Automerge](https://automerge.org/) - JavaScript CRDTs
- [Yjs](https://docs.yjs.dev/) - High-performance CRDTs
- [Conflict-Free Replicated Data Types](https://en.wikipedia.org/wiki/Conflict-free_replicated_data_type)

---

## Contributing to Docs

Found an error? Want to improve something?

1. Open an issue describing the problem
2. Or submit a PR with fixes
3. See [CONTRIBUTING.md](../../../CONTRIBUTING.md)

---

**Last Updated:** 2025-10-29  
**Version:** 0.10.0

