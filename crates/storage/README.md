# Calimero Storage - CRDT Collections

> **Conflict-free Replicated Data Types for Distributed Applications**

Build distributed applications that automatically resolve conflicts. Write natural code with nested data structures - the storage layer handles synchronization and merge logic for you.

---

## Quick Start

```rust
use calimero_sdk::app;
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap, Vector};
use calimero_storage_macros::Mergeable;  // For custom structs!

#[app::state(emits = MyEvent)]
#[derive(BorshSerialize, BorshDeserialize)]
pub struct MyApp {
    // All these combinations work automatically!
    view_counts: UnorderedMap<String, Counter>,
    user_profiles: UnorderedMap<String, UserProfile>,
    recent_events: Vector<Event>,
}

// Custom struct with nested CRDTs - just add #[derive(Mergeable)]!
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct UserProfile {
    name: LwwRegister<String>,        // Last-Write-Wins with timestamps
    tags: UnorderedSet<String>,        // Add-wins set
    scores: UnorderedMap<String, Counter>,  // Nested CRDTs!
}
// Zero boilerplate - macro generates merge code! ✨

// That's it! No manual merge code, no conflict resolution - it just works! ✨
```

**What you get:**
- ✅ **Zero merge code** - automatic conflict resolution
- ✅ **Nested structures** - maps in maps, vectors of counters, etc.
- ✅ **No divergence** - all nodes converge to same state
- ✅ **Natural syntax** - use like normal Rust collections

---

## Available Collections

| Collection                        | Use Case            | Merge Strategy            | Nesting         |
| --------------------------------- | ------------------- | ------------------------- | --------------- |
| **Counter**                       | Counters, metrics   | **Sum**                   | Leaf            |
| **LwwRegister&lt;T&gt;**          | Single values       | **Latest timestamp**      | Leaf            |
| **ReplicatedGrowableArray**       | Text, documents     | **Character-level**       | Leaf            |
| **UnorderedMap&lt;K,V&gt;**       | Key-value storage   | **Recursive per-entry**   | ✅ Can nest      |
| **Vector&lt;T&gt;**               | Ordered lists       | **Element-wise**          | ✅ Can nest      |
| **UnorderedSet&lt;T&gt;**         | Unique values       | **Union**                 | Simple values   |

---

## Common Patterns

### Pattern 1: Counters for Metrics

```rust
#[app::state]
pub struct Analytics {
    page_views: UnorderedMap<String, Counter>,
}

// Concurrent increments automatically sum!
// Node A: page_views["home"].increment() → 5
// Node B: page_views["home"].increment() → 7 (concurrent)
// After sync: page_views["home"] = 12 ✅
```

### Pattern 2: User Profiles with LWW

```rust
#[app::state]
pub struct UserManager {
    users: UnorderedMap<String, LwwRegister<UserProfile>>,
}

// Latest update wins (by timestamp)
// Node A: users["alice"] = Profile { name: "Alice A" } @ T1
// Node B: users["alice"] = Profile { name: "Alice B" } @ T2
// After sync: users["alice"] = Profile @ T2 ✅
```

### Pattern 3: Nested Maps

```rust
#[app::state]
pub struct DocumentEditor {
    // Map of document IDs to their metadata
    metadata: UnorderedMap<String, UnorderedMap<String, String>>,
}

// Concurrent updates to different fields preserve both!
// Node A: metadata["doc-1"]["title"] = "My Doc"
// Node B: metadata["doc-1"]["author"] = "Alice" (concurrent)
// After sync: BOTH fields present! ✅
```

### Pattern 4: Activity Logs

```rust
#[app::state]
pub struct ActivityTracker {
    events: Vector<Event>,
    unique_users: UnorderedSet<String>,
}

// Vector: append-heavy workloads
// Set: unique membership testing
```

---

## Performance Characteristics

| Operation                      | Complexity   | When Merge Called   | Impact   |
| ------------------------------ | ------------ | ------------------- | -------- |
| **Local insert**               | O(1)         | ❌ Never             | None     |
| **Remote sync (diff keys)**    | O(1)         | ❌ Never             | None     |
| **Remote sync (same key)**     | O(1)         | ❌ HLC+LWW           | None     |
| **Root conflict**              | O(F×E)       | ✅ Rare              | < 1%     |

**F** = number of root fields (typically 3-10)  
**E** = entries per field (typically 10-100)

**Bottom line:** Merge overhead is negligible - network latency dominates (50-200ms >> 1-2ms merge time)

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│ Your Code (Natural Rust)                                    │
│                                                             │
│ #[app::state]                                               │
│ pub struct MyApp {                                          │
│     data: UnorderedMap<String, Document>                   │
│ }                                                           │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ Macro Auto-Generates                                        │
│                                                             │
│ • impl Mergeable for MyApp                                  │
│ • Registration hook                                         │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ Storage Layer (Automatic)                                   │
│                                                             │
│ ┌──────────┐  ┌──────────┐  ┌──────────┐                  │
│ │   DAG    │  │ Elements │  │  Merge   │                  │
│ │ (99% of  │→ │ (IDs +   │→ │ (1% of   │                  │
│ │ conflicts)│  │ storage) │  │ conflicts)│                  │
│ └──────────┘  └──────────┘  └──────────┘                  │
└─────────────────────────────────────────────────────────────┘
```

**99% of operations:** DAG + element IDs handle conflicts (O(1), fast)  
**1% of operations:** Explicit merge for root conflicts (O(F×E), rare)

---

## When Merge is Called

```
Local Operation (99%):
    insert() → Storage write → ✅ Done (no merge)

Remote Sync - Different Element (90%):
    receive update → Different ID → ✅ Apply directly (no merge)

Remote Sync - Same Element (9%):
    receive update → Same ID → HLC comparison → ✅ LWW (no explicit merge)

Remote Sync - Root Conflict (1%):
    receive update → Root ID + concurrent → 🔄 Call Mergeable::merge()
    → Recursive field-level merge → ✅ Both updates preserved
```

---

## Decision Tree: Which Collection to Use?

```
Need to count/sum things?
└─> Counter

Need a single value with conflict resolution?
└─> LwwRegister<T>

Need text editing?
└─> ReplicatedGrowableArray

Need key-value storage?
├─> Simple values?
│   └─> UnorderedMap<K, V>
└─> Nested CRDTs?
    └─> UnorderedMap<K, Counter>             // Counters sum
    └─> UnorderedMap<K, LwwRegister<T>>      // Timestamps win
    └─> UnorderedMap<K, UnorderedMap<K2, V>> // Nested maps

Need ordered list?
├─> Append-heavy (logs, metrics)?
│   └─> Vector<T>
└─> Arbitrary edits (text)?
    └─> ReplicatedGrowableArray

Need unique membership?
└─> UnorderedSet<T>
```

---

## Limitations & Gotchas

| Collection             | Limitation                                             | Workaround                              |
| ---------------------- | ------------------------------------------------------ | --------------------------------------- |
| **Vector**             | Concurrent inserts at arbitrary positions may conflict | Use for append-heavy workloads          |
| **UnorderedSet**       | Can't contain CRDTs (no update semantics)              | Use UnorderedMap instead                |
| **LwwRegister**        | Concurrent updates lose one (LWW)                      | Expected behavior for single values     |
| **All Collections**    | Root-level non-CRDTs use LWW                           | Use LwwRegister for explicit timestamps |

---

## Learn More

### For Developers (Practical Guides)

- **[Collections Guide](readme/collections.md)** - Complete API reference with examples
- **[Nesting Guide](readme/nesting.md)** - How to use nested CRDTs effectively
- **[Migration Guide](readme/migration.md)** - Upgrading from manual flattening

### For Architects (Deep Dives)

- **[Architecture](readme/architecture.md)** - How the system works internally
- **[Merge System](readme/merging.md)** - DAG vs explicit merge explained
- **[Performance](readme/performance.md)** - Benchmarks and optimization

### For Contributors

- **[TODO](../../../TODO.md)** - Planned features and enhancements
- **[Design Decisions](readme/design-decisions.md)** - Why we built it this way

---

## Examples

### E-Commerce Application

```rust
#[app::state(emits = OrderEvent)]
pub struct EcommerceApp {
    // Product inventory with counters
    inventory: UnorderedMap<String, Counter>,
    
    // User carts (nested maps!)
    carts: UnorderedMap<String, UnorderedMap<String, Counter>>,
    
    // Order history (append-only)
    orders: Vector<Order>,
    
    // Featured product IDs (unique set)
    featured: UnorderedSet<String>,
}

pub struct Order {
    id: String,
    user: LwwRegister<String>,
    total: LwwRegister<u64>,
    items: UnorderedMap<String, Counter>,
}
```

### Collaborative Document Editor

```rust
#[app::state(emits = EditorEvent)]
pub struct CollaborativeEditor {
    documents: UnorderedMap<String, Document>,
}

pub struct Document {
    content: ReplicatedGrowableArray,  // Text with character-level CRDT
    edit_count: Counter,                 // Total edits counter
    metadata: UnorderedMap<String, LwwRegister<String>>,  // Title, author, etc.
    viewers: UnorderedSet<String>,      // Active viewers
}
```

### Analytics Dashboard

```rust
#[app::state]
pub struct Analytics {
    // Time-series metrics
    hourly_views: Vector<Counter>,
    
    // Page-specific counters
    page_metrics: UnorderedMap<String, PageMetrics>,
    
    // Active sessions
    active_users: UnorderedSet<String>,
}

pub struct PageMetrics {
    views: Counter,
    clicks: Counter,
    time_spent: Counter,  // In seconds
}
```

---

## FAQ

**Q: Do I need to write merge code?**  
A: No! The `#[app::state]` macro generates it automatically.

**Q: What happens with concurrent updates?**  
A: It depends on the collection:
- Counter: Values sum
- LwwRegister: Latest timestamp wins
- Map/Set: Add-wins semantics
- Vector: Element-wise merge

**Q: Can I nest CRDTs arbitrarily deep?**  
A: Yes! `Map<K, Map<K2, Map<K3, V>>>` works fine.

**Q: What about performance?**  
A: Negligible impact. Merge happens rarely (< 1% of operations) and is fast (1-2ms) compared to network (50-200ms).

**Q: How do I migrate from manual flattening?**  
A: See [Migration Guide](readme/migration.md). TL;DR: Both patterns work, choose what's clearer.

**Q: What if I need custom conflict resolution?**  
A: Implement `Mergeable` manually. See [Architecture Guide](readme/architecture.md).

---

## Quick Reference

```rust
// Counter - auto-sum
let mut counter = Counter::new();
counter.increment()?;  // All increments sum

// LwwRegister - timestamp wins
let mut register = LwwRegister::new("initial");
register.set("updated");  // Latest timestamp wins

// Map - entry-wise merge
let mut map = UnorderedMap::new();
map.insert("key", value)?;  // Concurrent inserts to different keys preserved

// Vector - element-wise merge
let mut vec = Vector::new();
vec.push(value)?;  // Concurrent appends preserved

// Set - union merge
let mut set = UnorderedSet::new();
set.insert("value")?;  // All adds preserved (union)
```

---

## License

See root [LICENSE](../../LICENSE) file.

## Contributing

See root [CONTRIBUTING.md](../../CONTRIBUTING.md).

---