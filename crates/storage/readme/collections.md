# Collections API Reference

Complete guide to all CRDT collections in Calimero Storage.

---

## Counter

**Use case:** Increment-only counters, metrics, view counts

### API

```rust
use calimero_storage::collections::Counter;

let mut counter = Counter::new();
counter.increment()?;              // Add 1
let value = counter.value()?;      // Get current value
```

### Merge Behavior

**Concurrent increments SUM:**
```
Node A: counter = 5
Node B: counter = 7 (concurrent)
Merge: counter = 12 ✅ (5 + 7, both increments preserved)
```

### Performance

| Operation   | Complexity   | Storage         |
| ----------- | ------------ | --------------- |
| new()       | O(1)         | Single element  |
| increment() | O(1)         | Update element  |
| value()     | O(1)         | Read element    |
| merge()     | O(N)         | N=other's value |

### Nesting

- ✅ **In Maps:** `Map<K, Counter>` - counters sum per key
- ✅ **In Vectors:** `Vector<Counter>` - counters sum per index
- ❌ **In Sets:** Use Map instead

### Example

```rust
#[app::state]
pub struct Analytics {
    page_views: UnorderedMap<String, Counter>,
}

impl Analytics {
    pub fn track_view(&mut self, page: String) {
        let mut counter = self.page_views
            .get(&page)?
            .unwrap_or_else(|| Counter::new());
        counter.increment()?;
        self.page_views.insert(page, counter)?;
    }
}

// Multiple nodes increment simultaneously → All increments preserved!
```

---

## LwwRegister<T>

**Use case:** Single values with timestamp-based conflict resolution

### API

```rust
use calimero_storage::collections::LwwRegister;

let mut register = LwwRegister::new("initial value");
register.set("updated value");
let value = register.get();  // &T
let owned = register.into_inner();  // T
```

### Merge Behavior

**Latest timestamp wins:**
```
Node A: register = "Alice" @ T1
Node B: register = "Bob" @ T2 (T2 > T1)
Merge: register = "Bob" ✅
```

**Tie-breaking:**
```
Node A: register = "Alice" @ T1, node_id=0xAAA
Node B: register = "Bob" @ T1 (same timestamp!), node_id=0xBBB
Merge: register = "Bob" ✅ (higher node_id wins)
```

### Performance

| Operation   | Complexity   | Storage           |
| ----------- | ------------ | ----------------- |
| new(value)  | O(1)         | Value + timestamp |
| set(value)  | O(1)         | Update both       |
| get()       | O(1)         | Read value        |
| merge()     | O(1)         | Timestamp compare |

### Nesting

- ✅ **In Maps:** `Map<K, LwwRegister<T>>` - timestamps per key
- ✅ **In Vectors:** `Vector<LwwRegister<T>>` - timestamps per index
- ✅ **Nested types:** `LwwRegister<CustomStruct>` works!

### Example

```rust
#[app::state]
pub struct UserManager {
    profiles: UnorderedMap<String, LwwRegister<UserProfile>>,
}

pub struct UserProfile {
    name: String,
    email: String,
    updated_at: u64,
}

impl UserManager {
    pub fn update_profile(&mut self, user_id: String, profile: UserProfile) {
        let register = LwwRegister::new(profile);
        self.profiles.insert(user_id, register)?;
    }
}

// Concurrent profile updates → Latest wins deterministically!
```

---

## ReplicatedGrowableArray (RGA)

**Use case:** Text editing, documents, collaborative content

### API

```rust
use calimero_storage::collections::ReplicatedGrowableArray;

let mut rga = ReplicatedGrowableArray::new();
rga.insert_str(0, "Hello")?;      // Insert at position
rga.insert_str(5, " World")?;     // Insert more
let text = rga.get_text()?;        // Get full text
rga.delete_range(0, 5)?;          // Delete range
```

### Merge Behavior

**Character-level CRDT with tombstones:**
```
Node A: insert "Hello" at 0
Node B: insert "World" at 0 (concurrent)
Merge: "HelloWorld" or "WorldHello" (deterministic by timestamp)
```

### Performance

| Operation             | Complexity   | Storage        |
| --------------------- | ------------ | -------------- |
| new()                 | O(1)         | Single element |
| insert_str(pos, text) | O(M)         | M=text length  |
| get_text()            | O(N)         | N=char count   |
| delete_range()        | O(K)         | K=range size   |

### Nesting

- ✅ **In Maps:** `Map<K, RGA>` - text per key
- ✅ **In Vectors:** `Vector<RGA>` - multiple documents
- ❌ **Nested RGA:** Not supported (use one RGA per document)

### Example

```rust
#[app::state]
pub struct CollaborativeEditor {
    documents: UnorderedMap<String, ReplicatedGrowableArray>,
}

impl CollaborativeEditor {
    pub fn insert_text(&mut self, doc_id: String, pos: usize, text: String) {
        let mut doc = self.documents.get(&doc_id)?.unwrap();
        doc.insert_str(pos, &text)?;
        self.documents.insert(doc_id, doc)?;
    }
}

// Multiple users type simultaneously → All edits preserved!
```

---

## UnorderedMap<K, V>

**Use case:** Key-value storage, dictionaries, lookups

### API

```rust
use calimero_storage::collections::UnorderedMap;

let mut map = UnorderedMap::new();
map.insert("key", "value")?;              // Insert/update
let value = map.get(&"key")?;             // Get Option<V>
let exists = map.contains(&"key")?;       // Check existence
let removed = map.remove(&"key")?;        // Remove key
let entries = map.entries()?;             // Iterator
map.clear()?;                             // Remove all
```

### Merge Behavior

**Entry-wise merge with recursive support:**
```
Node A: map["doc-1"] = Document { title: "A" }
Node B: map["doc-2"] = Document { title: "B" } (different key)
Merge: Both entries present ✅

Node A: map["doc-1"]["title"] = "Title A"
Node B: map["doc-1"]["owner"] = "Alice" (same key, different nested field)
Merge: Both fields present ✅ (recursive merge)
```

### Performance

| Operation    | Complexity   | Storage             |
| ------------ | ------------ | ------------------- |
| new()        | O(1)         | Single collection   |
| insert(k, v) | O(1)         | One element         |
| get(k)       | O(1)         | Lookup by ID        |
| remove(k)    | O(1)         | Mark deleted        |
| entries()    | O(N)         | Iterate all         |
| merge()      | O(N×M)       | N=entries, M=nested |

### Nesting

- ✅ **In Maps:** `Map<K, Map<K2, V>>` - unlimited depth!
- ✅ **In Vectors:** `Vector<Map<K, V>>` - maps per index
- ✅ **Values:** Any type implementing Mergeable

### Example

```rust
#[app::state]
pub struct DocumentStore {
    // Simple map
    titles: UnorderedMap<String, String>,
    
    // Map of counters (nested CRDT)
    edit_counts: UnorderedMap<String, Counter>,
    
    // Map of maps (double nested!)
    metadata: UnorderedMap<String, UnorderedMap<String, LwwRegister<String>>>,
}

// All concurrent updates merge correctly!
```

---

## Vector<T>

**Use case:** Ordered lists, logs, time-series, metrics

### API

```rust
use calimero_storage::collections::Vector;

let mut vec = Vector::new();
vec.push(item)?;                    // Append to end
let item = vec.get(index)?;         // Get by index
let item = vec.pop()?;              // Remove from end
let old = vec.update(index, item)?; // Replace at index
let len = vec.len()?;               // Get length
vec.clear()?;                       // Remove all
```

### Merge Behavior

**Element-wise merge:**
```
vec1 = [Counter(2), Counter(1)]
vec2 = [Counter(3), Counter(4)] (concurrent)
Merge: [Counter(5), Counter(5)] ✅ (element-wise sum)

vec1 = [Counter(2)]
vec2 = [Counter(3), Counter(5)] (longer)
Merge: [Counter(5), Counter(5)] ✅ (merge + append)
```

### Performance

| Operation    | Complexity   | Storage           |
| ------------ | ------------ | ----------------- |
| new()        | O(1)         | Single collection |
| push(v)      | O(1)         | One element       |
| get(i)       | O(N)         | Linear scan       |
| pop()        | O(1)         | Remove last       |
| update(i, v) | O(N)         | Find + update     |
| merge()      | O(min(N,M))  | Element-wise      |

### Merge Strategy Details

**Element-wise with LWW for length:**

1. **Same length:** Merge element-by-element at same indices (recursive for CRDTs)
2. **Different length:** Append extra elements from longer vector
3. **Nested CRDTs:** Properly merge (Counters sum, Registers use LWW, etc.)

**Example:**
```rust
// vec1 = [Counter(2), Counter(5)]
// vec2 = [Counter(3), Counter(7)]
// Merge: [Counter(5), Counter(12)]  // Element-wise sum!
```

**Limitations:**
- Concurrent inserts at arbitrary positions may conflict
- Best for: Append-heavy workloads (logs, timelines, metrics)
- Not ideal for: Arbitrary edits (use ReplicatedGrowableArray for text)

### Nesting

- ✅ **In Maps:** `Map<K, Vector<T>>` - vectors per key
- ✅ **In Vectors:** `Vector<Vector<T>>` - 2D arrays
- ✅ **Values:** `Vector<Counter>`, `Vector<LwwRegister<T>>`, etc.

### Best Practices

```rust
// ✅ Good: Append-heavy
metrics.push(Counter::new())?;  // Logs, time-series

// ✅ Good: Index-based updates
metrics.update(0, new_counter)?;  // Element-wise merge works!

// ⚠️ Caution: Arbitrary inserts
vec.insert_at(5, item)?;  // Position conflicts may occur
// Better: Use RGA for text/positional edits
```

### Example

```rust
#[app::state]
pub struct EventLog {
    events: Vector<Event>,
    hourly_metrics: Vector<Counter>,
}

pub struct Event {
    timestamp: u64,
    data: LwwRegister<String>,
}

impl EventLog {
    pub fn log_event(&mut self, event: Event) {
        self.events.push(event)?;  // Append-only ✅
    }
    
    pub fn increment_metric(&mut self, hour: usize) {
        if let Some(mut counter) = self.hourly_metrics.get(hour)? {
            counter.increment()?;
            self.hourly_metrics.update(hour, counter)?;
        }
    }
}

// Concurrent appends preserved, metrics sum correctly!
```

---

## UnorderedSet<T>

**Use case:** Unique membership, tags, flags

### API

```rust
use calimero_storage::collections::UnorderedSet;

let mut set = UnorderedSet::new();
set.insert("value")?;           // Add element
let has = set.contains(&"value")?;  // Check membership
set.remove(&"value")?;          // Remove element
let items = set.iter()?;        // Iterator
set.clear()?;                   // Remove all
```

### Merge Behavior

**Union (add-wins):**
```
set1 = {"alice", "bob"}
set2 = {"bob", "charlie"} (concurrent)
Merge: {"alice", "bob", "charlie"} ✅ (union)
```

### Performance

| Operation   | Complexity   | Storage           |
| ----------- | ------------ | ----------------- |
| new()       | O(1)         | Single collection |
| insert(v)   | O(1)         | One element       |
| contains(v) | O(1)         | Lookup by ID      |
| remove(v)   | O(1)         | Mark deleted      |
| iter()      | O(N)         | Iterate all       |
| merge()     | O(N)         | N=other's size    |

### Merge Strategy Details

**Union (add-wins) semantics:**

```rust
// set1 = {"alice", "bob"}
// set2 = {"bob", "charlie"}
// Merge: {"alice", "bob", "charlie"}  // Union, duplicates removed
```

**Why Sets Can't Contain CRDTs:**

Sets test membership via equality:
```rust
set.contains(&value)  // Uses value == other_value
```

CRDTs need merging, not equality testing:
```rust
// This doesn't make sense:
set.insert(Counter(5))?;
set.insert(Counter(7))?;  // Different element or update?

// Clear semantics with Map:
map.insert("counter1", Counter(5))?;
map.insert("counter1", Counter(7))?;  // Clear: update key "counter1"
```

**Better pattern:** Use `UnorderedMap<K, Counter>` for CRDT values.

### Nesting

- ✅ **In Maps:** `Map<K, Set<V>>` - sets per key
- ✅ **In Vectors:** `Vector<Set<V>>` - sets per index
- ❌ **Values:** Simple types only (String, UserId, etc.)
  - ❌ **Not CRDTs:** Sets can't contain Counters, Registers, etc.
  - **Why:** Sets test equality; CRDTs need merge, not equality
  - **Solution:** Use `Map<K, Counter>` instead of `Set<Counter>`

### Example

```rust
#[app::state]
pub struct UserManager {
    // Active sessions (unique user IDs)
    active_users: UnorderedSet<String>,
    
    // Tags per user (Map of Sets!)
    user_tags: UnorderedMap<String, UnorderedSet<String>>,
}

impl UserManager {
    pub fn add_tag(&mut self, user_id: String, tag: String) {
        let mut tags = self.user_tags
            .get(&user_id)?
            .unwrap_or_else(|| UnorderedSet::new());
        tags.insert(tag)?;
        self.user_tags.insert(user_id, tags)?;
    }
}

// Concurrent tag additions → Union (all tags preserved)!
```

---

## Comparison Table

| Collection      | Best For        | Merge        | Nesting     | Performance   |
| --------------- | --------------- | ------------ | ----------- | ------------- |
| **Counter**     | Metrics, counts | Sum          | Leaf        | O(1) all ops  |
| **LwwRegister** | Single values   | LWW          | Leaf        | O(1) all ops  |
| **RGA**         | Text editing    | Char-level   | Leaf        | O(N) get      |
| **Map**         | Key-value       | Recursive    | ✅ Full      | O(1) get/set  |
| **Vector**      | Ordered lists   | Element-wise | ✅ Full      | O(N) get      |
| **Set**         | Membership      | Union        | Values only | O(1) ops      |

---

## Quick Selection Guide

```
Counting things? → Counter
Single value with conflicts? → LwwRegister<T>
Text editing? → ReplicatedGrowableArray
Key-value pairs? → UnorderedMap<K, V>
Ordered list? → Vector<T>
Unique membership? → UnorderedSet<T>
```

---

## See Also

- [Nesting Guide](nesting.md) - How to combine collections
- [Architecture](architecture.md) - How collections work internally
- [Migration Guide](migration.md) - Upgrading existing apps
