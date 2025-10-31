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
| **Option&lt;T&gt;**               | Optional CRDTs      | **Recursive if Some**     | ✅ Wrapper       |

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

### Pattern 5: Optional CRDT Fields

```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct UserProfile {
    name: LwwRegister<String>,
    bio: Option<LwwRegister<String>>,  // Optional field that merges when present
    settings: LwwRegister<Option<Settings>>,  // LWW with optional value
}

// Option<T> is now Mergeable!
// - Option<LwwRegister<T>>: Optional field with LWW semantics when present
// - LwwRegister<Option<T>>: LWW decides which Option (Some/None) wins
// - Both Some: recursively merges inner values
// - One None, one Some: takes the Some value
```

### Pattern 6: Vector Search

```rust
#[app::state]
pub struct TaskList {
    tasks: Vector<Task>,
}

// Find first completed task
let completed = tasks.find(|t| t.is_complete)?
    .next();

// Get all high priority tasks
let high_priority: Vec<Task> = tasks
    .filter(|t| t.priority == Priority::High)?
    .collect();

// Count pending tasks
let pending_count = tasks
    .filter(|t| !t.is_complete)?
    .count();
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
| **Primitives**         | Use simple LWW without timestamps                      | Use LwwRegister for proper timestamps   |
| **All Collections**    | Root-level non-CRDTs use LWW                           | Use LwwRegister for explicit timestamps |

### ⚠️ CRITICAL: Primitives Can Cause State Divergence!

**Basic types like `String`, `u64`, `bool` ARE mergeable but with a serious flaw:**

```rust
#[derive(Mergeable)]
pub struct UserProfile {
    name: String,  // ⚠️ DANGER: Can cause state divergence!
    age: u64,      // ⚠️ DANGER: Nodes may not converge!
}

// What happens with concurrent updates:
// Node A: name = "Alice"
// Node B: name = "Bob" (concurrent)

// After sync:
// Node A receives "Bob": "Alice".merge(&"Bob") → "Bob"
// Node B receives "Alice": "Bob".merge(&"Alice") → "Alice"

// RESULT: State divergence! ❌
// Node A has name="Bob", Node B has name="Alice"
// Nodes never converge to same state!
```

**Why this is broken:**
- ❌ No timestamp comparison (can't tell which is "latest")
- ❌ Merge is NOT commutative: merge(A,B) ≠ merge(B,A)
- ❌ **Violates CRDT property**: nodes don't converge!
- ❌ State divergence leads to inconsistent application behavior

**Better approach with LwwRegister:**

```rust
#[derive(Mergeable)]
pub struct UserProfile {
    name: LwwRegister<String>,  // ✅ Has timestamps!
    age: LwwRegister<u64>,      // ✅ Proper LWW semantics
}

// What happens:
// Node A @ T1: name = "Alice"
// Node B @ T2: name = "Alicia" (T2 > T1)
// Merge: name = "Alicia" ✅ (T2 wins deterministically!)
```

**When primitives are SAFE to use:**
- ✅ **Single-writer fields**: Only one node ever updates this field
- ✅ **Immutable fields**: Set once, never changed (IDs, creation timestamps)
- ✅ **Different fields per node**: Each node updates its own set of fields

**When you MUST use LwwRegister:**
- ⚠️ **Multiple writers**: Any field that multiple nodes might update
- ⚠️ **Concurrent updates possible**: Even if rare, divergence is catastrophic
- ⚠️ **Production applications**: State divergence breaks distributed apps
- ✅ **Any field with >1 potential writer**: Use LwwRegister to be safe

**Real-world impact:**
```rust
// ❌ UNSAFE in production:
#[derive(Mergeable)]
pub struct Document {
    title: String,     // Two users edit → divergence
    content: String,   // Concurrent edits → different states per node
}

// ✅ SAFE - single writer or immutable:
#[derive(Mergeable)]
pub struct SystemConfig {
    node_id: String,        // Immutable, set once
    region: String,         // Only this node updates its region
    created_at: u64,        // Immutable timestamp
}

// ✅ PRODUCTION-SAFE with LwwRegister:
#[derive(Mergeable)]
pub struct Document {
    title: LwwRegister<String>,      // Deterministic convergence
    content: ReplicatedGrowableArray, // Proper CRDT for text
    author: LwwRegister<String>,     // Latest update wins
}
```

---

## Documentation

For complete documentation, see the **[Documentation Index](readme/DOCUMENTATION_INDEX.md)** (if available).

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

**Q: Can I use Option with CRDTs?**  
A: Yes! `Option<T>` is now `Mergeable` where `T` is `Mergeable`. Use `Option<LwwRegister<T>>` for optional fields that merge when both are `Some`, or `LwwRegister<Option<T>>` for LWW semantics on optional values.

**Q: How do I search a Vector?**  
A: Use `find(predicate)` to get the first match, or `filter(predicate)` to get all matches. Both return iterators for efficient lazy evaluation.

**Q: Can I use LwwRegister values directly without calling `.get()`?**  
A: Yes! `LwwRegister` implements `Deref`, `AsRef`, `Borrow`, and `From` for seamless type conversions. Use `&*reg`, `reg.as_ref()`, or pass to functions expecting `&T` directly.

**Q: Can I use regular types like String, u64, bool in my structs?**  
A: **DANGER!** Primitives can cause **permanent state divergence** where nodes never converge. The merge is not commutative, violating CRDT properties. Only safe for truly immutable fields or guaranteed single-writer scenarios. **For production apps, always use `LwwRegister<T>`** for any field that might have concurrent updates. The risk of state divergence is not worth the slight syntax convenience.

**Q: What's the difference between `String` and `LwwRegister<String>`?**  
A: **CRITICAL:** `String` can cause **state divergence** (nodes end up with different values permanently) because merge is not commutative. `LwwRegister<String>` uses timestamps for deterministic convergence. **Never use primitives if concurrent updates are possible!** Always use `LwwRegister<T>` in production unless the field is truly immutable or single-writer.

---

## Quick Reference

```rust
// Counter - auto-sum
let mut counter = Counter::new();
counter.increment()?;  // All increments sum

// LwwRegister - timestamp wins with ergonomic conversions
let mut register = LwwRegister::new("initial");
register.set("updated");  // Latest timestamp wins
let s: &str = &*register;  // Deref to inner value
let len = process(register.as_ref());  // AsRef for function calls
let reg: LwwRegister<u64> = 42.into();  // From/Into for creation

// Map - entry-wise merge
let mut map = UnorderedMap::new();
map.insert("key", value)?;  // Concurrent inserts to different keys preserved

// Vector - element-wise merge with search
let mut vec = Vector::new();
vec.push(value)?;  // Concurrent appends preserved
let first = vec.find(|v| v.id == "target")?;  // Find first match
let matches: Vec<_> = vec.filter(|v| v.score > 100)?.collect();  // Filter all

// Set - union merge
let mut set = UnorderedSet::new();
set.insert("value")?;  // All adds preserved (union)

// Option<T> - recursive merge
let mut opt: Option<LwwRegister<String>> = Some(LwwRegister::new("value"));
opt.merge(&other)?;  // Merges inner values when both Some
```

---

## License

See root [LICENSE](../../LICENSE) file.

## Contributing

See root [CONTRIBUTING.md](../../CONTRIBUTING.md).

---