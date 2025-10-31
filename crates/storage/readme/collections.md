# Collections API Reference

Complete guide to all CRDT collections in Calimero Storage.

---

## Using Custom Structs: Implementing Mergeable

### Understanding the Mergeable Trait

```rust
pub trait Mergeable {
    /// Merge another instance into self
    fn merge(&mut self, other: &Self) -> Result<(), MergeError>;
}
```

**When is this called?**
- Only during **root-level concurrent updates** (rare ~1% of operations)
- NOT on local operations (those are O(1))
- NOT on different-key updates (DAG handles those)

**What it does:**
- Merges field-by-field when the same root entity is updated concurrently
- Enables proper CRDT semantics for nested structures
- Prevents state divergence

---

### Option 1: Derive Macro (Recommended) ✨

**Use when:** All fields are CRDT types, standard merge behavior

```rust
use calimero_storage_macros::Mergeable;

#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct TeamStats {
    wins: Counter,
    losses: Counter,
    draws: Counter,
}
// That's it! Zero boilerplate ✨
```

**What the macro generates:**
```rust
impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;
        Ok(())
    }
}
```

**Requirements:**
- ✅ All fields must implement `Mergeable`
- ✅ Struct must have named fields (not tuple struct)
- ✅ Must be a struct (not enum or union)

**Common errors:**
```rust
#[derive(Mergeable)]
pub struct MyStruct {
    name: String,  // ❌ ERROR: String doesn't implement Mergeable
}
// Fix: Use LwwRegister<String> instead
```

---

### Option 2: Manual Implementation (Custom Logic)

**Use when:** You need custom validation, logging, or business rules

```rust
use calimero_storage::collections::{Counter, Mergeable, MergeError};

#[derive(BorshSerialize, BorshDeserialize)]
pub struct TeamStats {
    wins: Counter,
    losses: Counter,
    draws: Counter,
}

impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // 1. Merge CRDT fields
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;
        
        // 2. Add custom validation
        let total = self.wins.value()? + self.losses.value()? + self.draws.value()?;
        if total > 1000 {
            return Err(MergeError::StorageError("Too many games!".to_string()));
        }
        
        // 3. Add logging
        eprintln!("Merged team stats: {} total games", total);
        
        Ok(())
    }
}
```

**When to use manual implementation:**
- ✅ Custom validation rules
- ✅ Logging/metrics/debugging
- ✅ Business logic constraints
- ✅ Conditional merging
- ✅ Complex merge strategies

---

### Option 3: Implementing for Non-CRDT Types

**Use case:** Types that are atomically replaced (whole-value LWW)

```rust
#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct FileMetadata {
    filename: String,
    size: u64,
    uploaded_at: u64,  // Timestamp for ordering
}

impl Mergeable for FileMetadata {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // LWW based on uploaded_at timestamp
        // This is safe because FileMetadata is written atomically
        if other.uploaded_at > self.uploaded_at {
            *self = other.clone();  // Replace entire struct
        }
        Ok(())
    }
}
```

**When this is safe:**
- ✅ Struct has an intrinsic timestamp field for ordering
- ✅ Entire struct is written atomically (not field-by-field)
- ✅ LWW semantics make sense for the domain

**When to avoid:**
- ❌ Fields updated independently
- ❌ No timestamp for ordering
- ❌ Need field-level merge (use CRDT fields instead)

---

### Comparison: Derive vs Manual

| Aspect              | Derive Macro           | Manual Implementation    |
| ------------------- | ---------------------- | ------------------------ |
| **Boilerplate**     | Zero                   | ~5-10 lines              |
| **Flexibility**     | Standard merge only    | Full control             |
| **Custom logic**    | Not possible           | Validation, logging, etc |
| **When to use**     | Most cases ✅           | Special requirements     |
| **Maintainability** | Auto-updates           | Manual updates needed    |
| **Type safety**     | Compile-time checks    | Compile-time checks      |

---

### Complete Examples

#### Example 1: Derive Macro (Simple)

```rust
use calimero_sdk::app;
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap};
use calimero_storage_macros::Mergeable;

#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct UserProfile {
    name: LwwRegister<String>,
    bio: LwwRegister<String>,
    post_count: Counter,
}

#[app::state]
pub struct SocialApp {
    users: UnorderedMap<String, UserProfile>,
}
```

#### Example 2: Manual Implementation (With Logging)

```rust
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Metrics {
    views: Counter,
    clicks: Counter,
}

impl Mergeable for Metrics {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Log before merge
        let before = self.views.value()?;
        
        // Merge
        self.views.merge(&other.views)?;
        self.clicks.merge(&other.clicks)?;
        
        // Log after merge
        let after = self.views.value()?;
        app::log!("Views merged: {} → {}", before, after);
        
        Ok(())
    }
}
```

#### Example 3: Conditional Merge

```rust
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Config {
    version: LwwRegister<u64>,
    settings: UnorderedMap<String, LwwRegister<String>>,
}

impl Mergeable for Config {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Only merge if versions compatible
        let our_ver = *self.version;
        let their_ver = *other.version;
        
        if (our_ver.max(their_ver) - our_ver.min(their_ver)) > 10 {
            return Err(MergeError::StorageError(
                "Version mismatch too large".to_string()
            ));
        }
        
        // Standard merge
        self.version.merge(&other.version)?;
        self.settings.merge(&other.settings)?;
        
        Ok(())
    }
}
```

---

### Best Practices

**DO:**
- ✅ Use `#[derive(Mergeable)]` for simple structs
- ✅ Make all fields CRDT types (Counter, LwwRegister, Map, etc.)
- ✅ Use `LwwRegister<T>` for primitive-like fields
- ✅ Call `.merge()` on every field (derive does this automatically)
- ✅ Add custom logic only when needed

**DON'T:**
- ❌ Use primitive types (String, u64) - use LwwRegister<T>
- ❌ Skip merging fields (will lose updates)
- ❌ Implement complex logic (keep it simple)
- ❌ Forget error handling (use `?`)

---

### Migration from Old Code

**Before (primitives - causes divergence):**
```rust
#[derive(Mergeable)]  // Won't compile anymore!
pub struct User {
    name: String,      // ❌ ERROR
    age: u64,          // ❌ ERROR
}
```

**After (CRDTs - compile-time safe):**
```rust
#[derive(Mergeable)]
pub struct User {
    name: LwwRegister<String>,  // ✅ Works
    age: LwwRegister<u64>,      // ✅ Works
}

// Usage with auto-casting:
user.name = "Alice".to_owned().into();  // .into() converts to LwwRegister
let s: &str = &*user.name;              // Deref for access
```

---

### Real-World Examples

**See these apps for complete examples:**
- `apps/team-metrics-macro` - Using `#[derive(Mergeable)]`
- `apps/team-metrics-custom` - Manual implementation with logging
- `apps/collaborative-editor` - Complex nested CRDTs
- `apps/nested-crdt-test` - All nesting patterns

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

// Ergonomic conversions (new!)
let s: &str = &*register;           // Deref to &T
let s: &String = register.as_ref(); // AsRef<T>
let borrowed: &String = register.borrow();  // Borrow<T>
let reg: LwwRegister<u64> = 42.into();  // From<T>
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
- ✅ **With Option:** `Option<LwwRegister<T>>` and `LwwRegister<Option<T>>` both work!

### Ergonomic Traits

`LwwRegister<T>` implements several traits for seamless usage:

**Deref** - Use like the inner type:
```rust
let reg = LwwRegister::new("Hello".to_owned());
let s: &str = &*reg;  // Deref to &String, then to &str
println!("Length: {}", reg.len());  // Call String methods directly
```

**AsRef<T>** - Pass to functions expecting `&T`:
```rust
fn process_string(s: &str) -> usize { s.len() }

let reg = LwwRegister::new("test".to_owned());
let len = process_string(reg.as_ref());  // Works!
```

**Borrow<T>** - Works with HashMap, BTreeMap:
```rust
use std::borrow::Borrow;
let reg = LwwRegister::new("key".to_owned());
let key: &String = reg.borrow();  // Compatible with std collections
```

**From<T>** - Create from values directly:
```rust
let reg: LwwRegister<String> = "hello".to_owned().into();
let num_reg: LwwRegister<u64> = 42.into();
```

**Display** - Format like the inner type:
```rust
let reg = LwwRegister::new("Hello");
println!("{}", reg);  // Prints: Hello
```

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

// Search methods (new!)
let first = vec.find(|item| predicate(item))?;   // Iterator with first match
let all = vec.filter(|item| predicate(item))?;   // Iterator with all matches
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

### Search Methods

`Vector<T>` provides efficient search capabilities:

**find(predicate)** - Get first matching element:
```rust
let tasks = Vector::<Task>::new();
// ... populate tasks ...

// Find first completed task
let completed = tasks.find(|t| t.is_complete)?
    .next();  // Returns Option<Task>

// Find by ID
let task = tasks.find(|t| t.id == "task-123")?
    .next()
    .ok_or("Not found")?;
```

**filter(predicate)** - Get all matching elements:
```rust
// Get all high priority tasks
let high_priority: Vec<Task> = tasks
    .filter(|t| t.priority == Priority::High)?
    .collect();

// Count pending tasks
let pending_count = tasks
    .filter(|t| !t.is_complete)?
    .count();

// Chain with other iterator methods
let urgent_ids: Vec<String> = tasks
    .filter(|t| t.is_urgent)?
    .map(|t| t.id.clone())
    .collect();
```

**Performance:**
- Both methods return iterators (lazy evaluation)
- `find()` stops at first match (O(k) where k is position)
- `filter()` checks all elements (O(n))
- Use `find()` when you need just one result
- Use `filter()` when you need all matches

### Best Practices

```rust
// ✅ Good: Append-heavy
metrics.push(Counter::new())?;  // Logs, time-series

// ✅ Good: Index-based updates
metrics.update(0, new_counter)?;  // Element-wise merge works!

// ✅ Good: Search with predicates
let item = vec.find(|x| x.id == "target")?.next();
let matches: Vec<_> = vec.filter(|x| x.value > 100)?.collect();

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

## Primitive Types (String, u64, bool, etc.)

**Use case:** Simple values in structs, when timestamp-based resolution isn't critical

### What Are Primitives?

Primitive types include: `String`, `u8`, `u16`, `u32`, `u64`, `u128`, `i8`, `i16`, `i32`, `i64`, `i128`, `bool`, `char`

### API

Primitives work directly in your structs:

```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct UserProfile {
    name: String,        // Primitive - simple LWW
    age: u64,            // Primitive - simple LWW
    is_verified: bool,   // Primitive - simple LWW
}
```

### Merge Behavior

**✅ COMPILE-TIME SAFETY: Primitives Do NOT Implement Mergeable**

Primitives (String, u64, bool, etc.) were **intentionally removed** from implementing `Mergeable`:

```rust
// This code is intentionally NOT included:
// impl Mergeable for String { ... }  // ❌ Removed!

// Users MUST use LwwRegister instead:
#[derive(Mergeable)]
pub struct App {
    name: LwwRegister<String>,  // ✅ Required!
}
```

**Why primitives were removed:**

Primitives would cause **permanent state divergence**:
```
Node A: name = "Alice", receives "Bob" → "Bob"
Node B: name = "Bob", receives "Alice" → "Alice"
Result: Nodes NEVER converge! ❌
```

Because simple LWW (`*self = other`) is NOT commutative:
- ❌ `merge(A, B) ≠ merge(B, A)`
- ❌ Violates CRDT convergence property
- ❌ Production applications would have inconsistent state

**Now prevented at compile time!**
```rust
#[derive(Mergeable)]
pub struct App {
    name: String,  // ❌ Compiler error!
}
// error[E0277]: the trait bound `String: Mergeable` is not satisfied
```

### When Can You Use Primitives?

**Only in non-synchronized contexts:**

1. **Private data** (node-local, not replicated):
```rust
#[app::private]  // NOT synchronized!
pub struct LocalCache {
    cache: UnorderedMap<String, String>,  // ✅ OK - primitives work here
    temp_data: String,                     // ✅ OK - not replicated
}
```

2. **Synchronized state** requires CRDTs:
```rust
#[app::state]  // Synchronized across nodes!
pub struct App {
    data: UnorderedMap<String, String>,  // ❌ ERROR: Won't compile!
}
// error[E0277]: the trait bound `String: Mergeable` is not satisfied

// Fix:
#[app::state]
pub struct App {
    data: UnorderedMap<String, LwwRegister<String>>,  // ✅ Works!
}
```

3. **Inside atomically-replaced structs:**
```rust
#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct FileMetadata {
    filename: String,     // OK - struct has LWW impl
    size: u64,
    uploaded_at: u64,
}

// Custom Mergeable that replaces whole struct
impl Mergeable for FileMetadata {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        if other.uploaded_at > self.uploaded_at {
            *self = other.clone();  // Atomic replacement
        }
        Ok(())
    }
}
```


**❌ NEVER use primitives when:**

1. **ANY possibility of concurrent updates** - Even rarely:
```rust
// ❌ Bad: Multiple nodes update same fields
pub struct SharedCounter {
    count: u64,  // Both nodes increment → one increment lost!
}

// ✅ Better: Use Counter
pub struct SharedCounter {
    count: Counter,  // Increments sum correctly!
}
```

2. **Need deterministic resolution** - Must know which update wins:
```rust
// ❌ Bad: Can't tell which is "latest"
pub struct Document {
    title: String,  // No timestamp - arbitrary winner
}

// ✅ Better: Use LwwRegister
pub struct Document {
    title: LwwRegister<String>,  // Timestamp tells us which is latest
}
```

3. **Production apps** - Multiple concurrent writers:
```rust
// ❌ Bad for production
pub struct UserProfile {
    name: String,       // Concurrent edits → arbitrary winner
    email: String,      // May lose updates!
}

// ✅ Better for production
pub struct UserProfile {
    name: LwwRegister<String>,   // Deterministic resolution
    email: LwwRegister<String>,  // Proper timestamps
}
```

### Primitives vs LwwRegister

| Aspect               | Primitive (String)      | LwwRegister<String>       |
| -------------------- | ----------------------- | ------------------------- |
| **Syntax**           | `name: String`          | `name: LwwRegister<String>` |
| **Timestamps**       | ❌ None                  | ✅ Hybrid Logical Clock    |
| **Merge logic**      | Take "other" value      | Compare timestamps        |
| **Deterministic**    | ❌ Arbitrary             | ✅ Latest wins             |
| **Concurrent safety**| ⚠️ May lose updates      | ✅ Correct resolution      |
| **When to use**      | Low contention          | Concurrent updates        |
| **Verbosity**        | Low                     | Slightly higher           |
| **Ergonomics**       | Natural                 | Deref, AsRef (seamless)   |

### Example: Migration from Primitives

**Before (prototyping):**
```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct Document {
    title: String,
    author: String,
    created_at: u64,
}

// Problem: Concurrent edits to title lose updates
```

**After (production-ready):**
```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct Document {
    title: LwwRegister<String>,    // Deterministic conflict resolution
    author: LwwRegister<String>,   // Latest update wins
    created_at: LwwRegister<u64>,  // Immutable in practice, but consistent
}

// Benefit: Concurrent edits resolve correctly with timestamps
```

### Implementation Details

Primitives implement `Mergeable` with a simple macro:

```rust
impl Mergeable for String {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // ⚠️ Warning: This loses information!
        // No timestamp comparison - just overwrites
        *self = other.clone();
        Ok(())
    }
}
```

**Supported primitives:**
- Integers: `u8`, `u16`, `u32`, `u64`, `u128`, `i8`, `i16`, `i32`, `i64`, `i128`
- Text: `String`, `char`
- Boolean: `bool`

### Performance

| Operation   | Complexity   | Notes                     |
| ----------- | ------------ | ------------------------- |
| merge()     | O(1)         | Simple clone operation    |
| No overhead | -            | No timestamp storage      |

**Trade-off:** Lower overhead vs less correctness

### Best Practices

```rust
// ✅ Good: Low-contention struct
#[derive(Mergeable)]
pub struct Config {
    server_url: String,      // Rarely changes, okay
    api_key: String,         // Set once, okay
    version: String,         // Read-only in practice, okay
}

// ⚠️ Risky: High-contention fields
#[derive(Mergeable)]
pub struct EditableDocument {
    title: String,           // Multiple users edit → use LwwRegister!
    content: String,         // High contention → use RGA!
}

// ✅ Best: Mix based on contention
#[derive(Mergeable)]
pub struct SmartDocument {
    id: String,                        // Immutable, primitive okay
    title: LwwRegister<String>,        // Editable, use LwwRegister
    content: ReplicatedGrowableArray,  // Text editing, use RGA
    created_at: u64,                   // Immutable, primitive okay
    view_count: Counter,               // Increments, use Counter
}
```

### Summary

**✅ Primitives NOT allowed in synchronized state - Compile-time safety!**
- ✅ **Prevented at compile time**: `String: Mergeable` not satisfied
- ✅ **No state divergence possible**: Can't compile broken code
- ✅ **Clear error messages**: Compiler tells you to use LwwRegister
- ✅ **Production-safe by default**: No way to accidentally break CRDTs

**For synchronized state fields:**
- ✅ **MUST use** `LwwRegister<T>` for string/numeric fields
- ✅ Use specialized CRDTs (`Counter` for incrementing, `RGA` for text)
- ✅ Use collections (`UnorderedMap`, `Vector`, `UnorderedSet`)
- ✅ Auto-casting with `.into()`: `"value".to_owned().into()` → `LwwRegister<String>`

**Primitives ONLY work in:**
- ✅ `#[app::private]` data (node-local, not synchronized)
- ✅ Inside custom `Mergeable` impls with proper timestamps
- ❌ NOT in `#[app::state]` or synchronized collections

**The golden rule:** All synchronized fields must be CRDT types!

---

## Option<T>

**Use case:** Optional CRDT fields, nullable values with merge semantics

### API

`Option<T>` is now `Mergeable` when `T` is `Mergeable`:

```rust
use calimero_storage::collections::LwwRegister;

// Option wrapping a CRDT
let mut opt1: Option<LwwRegister<String>> = Some(LwwRegister::new("Alice"));
let opt2: Option<LwwRegister<String>> = Some(LwwRegister::new("Bob"));
opt1.merge(&opt2)?;  // Inner LwwRegisters merge

// LwwRegister wrapping Option
let mut reg1 = LwwRegister::new(Some("value".to_owned()));
let reg2 = LwwRegister::new(None);
reg1.merge(&reg2);  // LWW semantics on the Option itself
```

### Merge Behavior

**Recursive merge when both are Some:**
```
opt1 = Some(LwwRegister("Alice" @ T1))
opt2 = Some(LwwRegister("Bob" @ T2))
Merge: Some(LwwRegister("Bob" @ T2)) ✅ (inner values merged using LWW)
```

**Take Some when one is None:**
```
opt1 = None
opt2 = Some(value)
Merge: Some(value) ✅ (takes the Some value)
```

**Keep Some when other is None:**
```
opt1 = Some(value)
opt2 = None
Merge: Some(value) ✅ (keeps existing Some)
```

**Both None - no change:**
```
opt1 = None
opt2 = None
Merge: None ✅
```

### Use Cases

**1. Optional fields that merge:**
```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct UserProfile {
    name: LwwRegister<String>,
    bio: Option<LwwRegister<String>>,  // Optional bio with LWW semantics
    avatar_url: Option<LwwRegister<String>>,  // Optional URL
}

// When both nodes set bio → LWW wins
// When one node sets bio → That value wins
```

**2. LWW semantics on optional values:**
```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct Config {
    theme: LwwRegister<Option<String>>,  // Latest timestamp decides Some/None
}

// Node A @ T1: theme = Some("dark")
// Node B @ T2: theme = None  (user cleared it)
// Merge: theme = None ✅ (T2 > T1, so None wins)
```

**3. Optional nested structures:**
```rust
pub struct Document {
    metadata: Option<UnorderedMap<String, LwwRegister<String>>>,
}

// metadata can be Some (mergeable map) or None
// When both Some → maps merge recursively
```

### Pattern Comparison

**Option<LwwRegister<T>>** - Optional field with LWW when present:
```rust
bio: Option<LwwRegister<String>>

// Both None → stays None
// One Some → takes that value
// Both Some → inner LwwRegisters merge (timestamp wins)
```

**LwwRegister<Option<T>>** - LWW decides the Option:
```rust
theme: LwwRegister<Option<String>>

// Timestamp always decides which Option wins
// Doesn't recursively merge - whole Option replaced by latest timestamp
```

### Performance

| Operation   | Complexity   | Notes                          |
| ----------- | ------------ | ------------------------------ |
| merge()     | O(M)         | M = cost of merging inner type |
| Some/None   | O(1)         | Checking variant               |

### Nesting

- ✅ **Anywhere:** `Option<T>` works wherever `T` would work
- ✅ **In Maps:** `Map<K, Option<LwwRegister<V>>>`
- ✅ **In Vectors:** `Vector<Option<Counter>>`
- ✅ **Wrapping collections:** `Option<UnorderedMap<K, V>>`

### Example

```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct UserProfile {
    name: LwwRegister<String>,
    bio: Option<LwwRegister<String>>,
    settings: LwwRegister<Option<Settings>>,
}

pub struct Settings {
    theme: String,
    notifications: bool,
}

impl UserProfile {
    pub fn set_bio(&mut self, bio: String) {
        self.bio = Some(LwwRegister::new(bio));
    }
    
    pub fn clear_bio(&mut self) {
        self.bio = None;
    }
    
    pub fn update_settings(&mut self, settings: Settings) {
        self.settings.set(Some(settings));
    }
}

// Concurrent bio updates:
// Node A: set_bio("Alice's bio")
// Node B: set_bio("Alice bio v2") (later timestamp)
// Merge: bio = Some("Alice bio v2") ✅

// Mixed operations:
// Node A: update_settings(...) @ T1
// Node B: clear by setting settings to None @ T2
// Merge: settings = None ✅ (T2 wins)
```

---

## Comparison Table

| Collection      | Best For        | Merge        | Nesting     | Performance   |
| --------------- | --------------- | ------------ | ----------- | ------------- |
| **Counter**     | Metrics, counts | Sum          | Leaf        | O(1) all ops  |
| **LwwRegister** | Single values   | LWW + time   | Leaf        | O(1) all ops  |
| **RGA**         | Text editing    | Char-level   | Leaf        | O(N) get      |
| **Map**         | Key-value       | Recursive    | ✅ Full      | O(1) get/set  |
| **Vector**      | Ordered lists   | Element-wise | ✅ Full      | O(N) get      |
| **Set**         | Membership      | Union        | Values only | O(1) ops      |
| **Option<T>**   | Optional fields | Recursive    | ✅ Wrapper   | O(M) merge    |
| **Primitives**  | Simple fields   | LWW no time  | Leaf        | O(1) all ops  |

---

## Quick Selection Guide

```
Counting things? → Counter
Single value with conflicts? → LwwRegister<T>
  - Low contention, prototyping? → String/u64/bool (primitives)
  - Production, concurrent writes? → LwwRegister<T> (timestamps!)
Text editing? → ReplicatedGrowableArray
Key-value pairs? → UnorderedMap<K, V>
Ordered list? → Vector<T>
  - Need to search? → Use find()/filter()
Unique membership? → UnorderedSet<T>
Optional CRDT field? → Option<T> where T: Mergeable
Optional value with LWW? → LwwRegister<Option<T>>
Simple struct fields? → Primitives okay for prototyping
  - ⚠️ Warning: No timestamps, arbitrary conflict resolution
```

---

## See Also

- [Nesting Guide](nesting.md) - How to combine collections
- [Architecture](architecture.md) - How collections work internally
- [Migration Guide](migration.md) - Upgrading existing apps
