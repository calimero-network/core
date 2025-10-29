# Nesting Guide - Using CRDTs Inside CRDTs

Complete guide to nesting collections for complex data structures.

---

## What is Nesting?

**Nesting** means putting one CRDT inside another:

```rust
// This is nesting:
UnorderedMap<String, Counter>          // Map contains Counter
Vector<LwwRegister<T>>                  // Vector contains LwwRegister
Map<K, Map<K2, V>>                      // Map contains Map!
```

**Why it's powerful:**
- ✅ Natural data structures
- ✅ Automatic conflict resolution at every level
- ✅ No manual flattening needed

---

## Nesting Support Matrix

| Container | Can Contain | Examples |
|-----------|-------------|----------|
| **UnorderedMap** | ✅ Any CRDT | `Map<K, Counter>`, `Map<K, Map<K2, V>>` |
| **Vector** | ✅ Any CRDT | `Vector<Counter>`, `Vector<Map<K, V>>` |
| **UnorderedSet** | ❌ Values only | `Set<String>` ✅, `Set<Counter>` ❌ |

**Unlimited depth:** `Map<K, Map<K2, Map<K3, Counter>>>` works!

---

## Common Nesting Patterns

### Pattern 1: Map of Counters

**Use case:** Per-entity metrics

```rust
page_views: UnorderedMap<String, Counter>

// Usage:
page_views.insert("home", Counter::new())?;
let mut views = page_views.get(&"home")?.unwrap();
views.increment()?;
page_views.insert("home", views)?;

// Merge: Counters sum per page ✅
```

**Pros:**
- ✅ Simple and clear
- ✅ Automatic summation
- ✅ Perfect for analytics

**Cons:**
- None! This is the ideal pattern for counters.

---

### Pattern 2: Map of LwwRegisters

**Use case:** Per-entity single values with timestamps

```rust
user_names: UnorderedMap<String, LwwRegister<String>>

// Usage:
user_names.insert("alice", LwwRegister::new("Alice Smith"))?;

// Merge: Latest update wins per user ✅
```

**Pros:**
- ✅ Explicit timestamps
- ✅ Deterministic conflict resolution
- ✅ Clear semantics

**Cons:**
- Slightly more verbose than plain String
- **Alternative:** `Map<String, String>` (uses LWW without explicit timestamps)

---

### Pattern 3: Nested Maps (Two Levels)

**Use case:** Document metadata, structured configuration

```rust
document_metadata: UnorderedMap<String, UnorderedMap<String, String>>

// Usage:
let mut metadata = UnorderedMap::new();
metadata.insert("title", "My Doc")?;
metadata.insert("author", "Alice")?;
document_metadata.insert("doc-1", metadata)?;

// Merge: Field-level merge! ✅
// Node A updates title → preserved
// Node B updates author (concurrent) → preserved
// Result: BOTH updates in final state
```

**Pros:**
- ✅ Natural structure
- ✅ Field-level merge
- ✅ No data loss

**Cons:**
- None! This is what nested CRDTs were built for.

---

### Pattern 4: Map of Vectors

**Use case:** Per-entity ordered lists

```rust
user_activities: UnorderedMap<String, Vector<Activity>>

// Usage:
let mut activities = Vector::new();
activities.push(Activity { ... })?;
user_activities.insert("alice", activities)?;

// Merge: Per-user vectors merge element-wise ✅
```

**Pros:**
- ✅ Per-entity timelines
- ✅ Append-heavy patterns work great

**Cons:**
- Vector merge is element-wise (not full OT)
- Best for append-only patterns

---

### Pattern 5: Map of Sets

**Use case:** Per-entity unique collections

```rust
user_tags: UnorderedMap<String, UnorderedSet<String>>

// Usage:
let mut tags = UnorderedSet::new();
tags.insert("rust")?;
tags.insert("backend")?;
user_tags.insert("alice", tags)?;

// Merge: Sets union per user ✅
```

**Pros:**
- ✅ Unique membership per entity
- ✅ Union merge preserves all tags

**Cons:**
- Sets can't contain CRDTs (use Map for that)

---

### Pattern 6: Vector of Counters

**Use case:** Time-series metrics

```rust
hourly_metrics: Vector<Counter>

// Usage:
for hour in 0..24 {
    hourly_metrics.push(Counter::new())?;
}
hourly_metrics.get(current_hour)?.unwrap().increment()?;

// Merge: Counters sum per hour ✅
```

**Pros:**
- ✅ Natural time-series representation
- ✅ Element-wise sum

**Cons:**
- Fixed size better than dynamic growth for this use case

---

## Deep Nesting (3+ Levels)

### Example: Full Application State

```rust
#[app::state]
pub struct ComplexApp {
    // Map → Map → Counter (3 levels)
    team_project_tasks: UnorderedMap<
        String,                              // team_id
        UnorderedMap<
            String,                          // project_id
            Counter                          // task count
        >
    >,
    
    // Map → Vector → Map (3 levels)
    user_activity_metadata: UnorderedMap<
        String,                              // user_id
        Vector<
            UnorderedMap<String, String>     // activity fields
        >
    >,
}

// Access:
let team_tasks = app.team_project_tasks.get(&team_id)?.unwrap();
let mut task_count = team_tasks.get(&project_id)?.unwrap();
task_count.increment()?;
```

**Does this work?** ✅ YES! Unlimited nesting depth supported.

**Performance:** Each level adds O(1) lookup (map) or O(N) scan (vector).

---

## Anti-Patterns (What NOT to Do)

### ❌ Sets Containing CRDTs

```rust
// DON'T:
active_counters: UnorderedSet<Counter>

// Problem: Sets use equality, CRDTs need merging
// Solution: Use Map instead
active_counters: UnorderedMap<String, Counter>
```

### ❌ Non-CRDT Fields at Root

```rust
// AVOID:
#[app::state]
pub struct MyApp {
    owner: String,  // ← No timestamp!
    counter: u64,   // ← Not a CRDT!
}

// Problem: Concurrent root updates use LWW
// Solution: Use CRDT types
#[app::state]
pub struct MyApp {
    owner: LwwRegister<String>,  // ✅ Timestamps
    counter: Counter,             // ✅ CRDT
}
```

### ❌ Vectors for Arbitrary Edits

```rust
// AVOID:
text: Vector<char>

// Concurrent inserts at position 5 → may conflict
// Solution: Use RGA for text
text: ReplicatedGrowableArray
```

---

## Nesting Decision Tree

```
Need per-entity values?
├─> Single value?
│   ├─> With timestamp resolution?
│   │   └─> Map<K, LwwRegister<T>>
│   └─> Simple LWW okay?
│       └─> Map<K, V>
├─> Counter/metric?
│   └─> Map<K, Counter>
├─> Ordered list?
│   └─> Map<K, Vector<T>>
├─> Unique set?
│   └─> Map<K, Set<V>>
└─> Structured data?
    └─> Map<K, Map<K2, V>>
```

---

## Performance Considerations

### Access Complexity

| Nesting | Access Cost | Example |
|---------|-------------|---------|
| 1 level | O(1) | `map.get(key)` |
| 2 levels | O(1) + O(1) | `map.get(k1)?.get(k2)` |
| 3 levels | O(1)×3 | `map.get(k1)?.get(k2)?.get(k3)` |

**Vectors:** O(N) per level for index access

### Merge Complexity

| Nesting | Merge Cost | Frequency |
|---------|------------|-----------|
| Map<K, Counter> | O(N) | < 1% of ops |
| Map<K, Map<K2, V>> | O(N×M) | < 1% of ops |
| Vector<Counter> | O(N) | < 1% of ops |

**Key insight:** Merge is rare (< 1% of operations) and network-bound anyway!

---

## Best Practices

### 1. Use the Right Collection for the Job

```rust
// ✅ Good
user_scores: Map<UserId, Counter>      // Counters need sum semantics

// ❌ Bad
user_scores: Map<UserId, u64>          // Plain u64 uses LWW (loses increments)
```

### 2. Keep Root Structure Simple

```rust
// ✅ Good: Few root fields
#[app::state]
pub struct MyApp {
    documents: Map<String, Document>,  // 1 field
    counters: Map<String, Counter>,    // 2 fields
}

// ⚠️ Okay but verbose: Many root fields
#[app::state]
pub struct MyApp {
    field1: Map<...>,
    field2: Map<...>,
    // ... 20 more fields
}
```

**Why:** Merge iterates root fields (O(F)). Keeping F small keeps merge fast.

### 3. Nest Strategically

```rust
// ✅ Good: Logical grouping
teams: Map<TeamId, Map<ProjectId, Counter>>

// ⚠️ Questionable: Too many levels
data: Map<K1, Map<K2, Map<K3, Map<K4, V>>>>

// Consider: Flatter structure with composite keys
data: Map<CompositeKey, V>  // Key = "k1::k2::k3::k4"
```

**Why:** More levels = more iteration during merge. 2-3 levels is sweet spot.

---

## Migration Examples

### From Manual Flattening

**Before (composite keys):**
```rust
#[app::state]
pub struct MyApp {
    metadata: Map<String, String>,
}

// Keys: "doc-1:title", "doc-1:author", "doc-2:title", ...
fn set_title(&mut self, doc_id: String, title: String) {
    let key = format!("{}:title", doc_id);
    self.metadata.insert(key, title)?;
}
```

**After (nested maps):**
```rust
#[app::state]
pub struct MyApp {
    metadata: Map<String, Map<String, String>>,
}

// Natural structure!
fn set_title(&mut self, doc_id: String, title: String) {
    let mut doc_meta = self.metadata.get(&doc_id)?
        .unwrap_or_else(|| Map::new());
    doc_meta.insert("title", title)?;
    self.metadata.insert(doc_id, doc_meta)?;
}
```

**Both work!** Choose what's clearer for your use case.

---

## Testing Nested Structures

```rust
#[test]
fn test_nested_merge() {
    // Create state with nested map
    let mut app = MyApp {
        metadata: Map::new(),
    };
    
    let mut doc1_meta = Map::new();
    doc1_meta.insert("title", "Doc 1")?;
    app.metadata.insert("doc-1", doc1_meta)?;
    
    // Serialize
    let bytes1 = borsh::to_vec(&app)?;
    
    // Simulate Node 2
    let mut app2: MyApp = borsh::from_slice(&bytes1)?;
    let mut doc1_meta2 = app2.metadata.get(&"doc-1")?.unwrap();
    doc1_meta2.insert("author", "Alice")?;  // Add field
    app2.metadata.insert("doc-1", doc1_meta2)?;
    
    let bytes2 = borsh::to_vec(&app2)?;
    
    // Merge
    let merged_bytes = merge_root_state(&bytes1, &bytes2, 100, 100)?;
    let merged: MyApp = borsh::from_slice(&merged_bytes)?;
    
    // Verify: BOTH fields present
    let doc_meta = merged.metadata.get(&"doc-1")?.unwrap();
    assert_eq!(doc_meta.get(&"title")?, Some("Doc 1"));
    assert_eq!(doc_meta.get(&"author")?, Some("Alice"));
}
```

---

## See Also

- [Collections API](collections.md) - Full API reference
- [Architecture](architecture.md) - How nesting works internally
- [Performance](performance.md) - Optimization guide
