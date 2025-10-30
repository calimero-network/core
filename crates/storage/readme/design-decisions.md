# Design Decisions

Why we built the CRDT system the way we did.

---

## Decision 1: Element IDs Over Operational Transformation

### What We Chose

**Element-based storage:** Every value gets a unique ID

```
map.insert("key", value)
  → Create Element { id: hash(map_id + "key"), data: value }
```

### Alternatives Considered

**Full OT (Operational Transformation):**
- Track every operation
- Transform operations on concurrent edits
- Complex algorithm

**Why we didn't:** Too complex, element IDs solve 95% of conflicts with O(1)

### Trade-offs

| Aspect          | Element IDs         | Full OT           |
| --------------- | ------------------- | ----------------- |
| **Complexity**  | Simple              | Complex           |
| **Performance** | O(1) for most       | O(N) always       |
| **Storage**     | More (IDs+metadata) | Less              |
| **Correctness** | Proven              | Hard to get right |

**Verdict:** Element IDs win for simplicity and performance.

---

## Decision 2: Automatic Merge Via Macro

### What We Chose

**Macro-generated merge code:**

```rust
#[app::state]  // ← Just this annotation
pub struct MyApp { ... }

// Macro generates Mergeable impl automatically
```

### Alternatives Considered

**Manual merge implementation:**
```rust
impl Mergeable for MyApp {
    fn merge(&mut self, other: &Self) {
        // Developer writes this
    }
}
```

**Why we didn't:** Too much boilerplate, error-prone

**Derive macro:**
```rust
#[derive(Mergeable)]  // Separate derive
pub struct MyApp { ... }
```

**Why we didn't:** Already have `#[app::state]`, why add another?

### Trade-offs

| Approach                 | Code     | Flexibility   | Errors   |
| ------------------------ | -------- | ------------- | -------- |
| **Manual**               | Lots     | Full          | Common   |
| **Derive**               | Medium   | Medium        | Rare     |
| **Auto (#[app::state])** | **Zero** | **Good**      | **Rare** |

**Verdict:** Auto-generation via existing macro wins for DX.

---

## Decision 3: Element-Wise Vector Merge (Not Full OT)

### What We Chose

**Simple element-wise merge:**

```rust
vec1 = [Counter(2), Counter(3)]
vec2 = [Counter(5), Counter(7)]
Merge: [Counter(7), Counter(10)]  // Element-wise sum
```

### Alternatives Considered

**Full positional OT:**
- Track insertion positions with vector clocks
- Adjust indices on concurrent edits
- Complex transformation functions

**Why we didn't:** 95% of vector usage is append-heavy (logs, metrics)

### Trade-offs

| Approach         | Complexity   | Use Cases       | Performance   |
| ---------------- | ------------ | --------------- | ------------- |
| **Element-wise** | Simple       | Append-heavy    | O(N)          |
| **Full OT**      | Complex      | Arbitrary edits | O(N×M)        |

**Verdict:** Element-wise for now, OT if needed later.

### When Element-Wise Works

✅ Logs and time-series (append-only)  
✅ Metrics per time period (index-based updates)  
✅ Activity streams (mostly append)

### When It Doesn't

❌ Collaborative text editing (use RGA)  
❌ Arbitrary insertions/deletions  

**Solution:** We provide RGA for text, element-wise Vector for lists.

---

## Decision 4: Derive Macro for Custom Structs

### What We Chose

**Provide `#[derive(Mergeable)]` for custom structs:**

```rust
use calimero_storage_macros::Mergeable;

#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct TeamStats {
    wins: Counter,
    losses: Counter,
}
// Macro auto-generates merge() implementation
```

### Alternatives Considered

**No derive - only manual impl:**
```rust
// Force developers to always write:
impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        Ok(())
    }
}
```
**Why we didn't:** Too much boilerplate for simple cases

**Auto-detect and merge all fields without derive:**
- Use reflection/type introspection
- **Why we didn't:** Doesn't work in Rust (no runtime reflection)

### Trade-offs

| Approach | Boilerplate | Flexibility | When to Use |
|----------|-------------|-------------|-------------|
| **#[derive(Mergeable)]** | Zero | Standard | Most cases ✅ |
| **Manual impl** | ~5 lines | Full control | Custom logic |

### Verdict

**Both options available:**
- ✅ Derive macro for simple cases (zero boilerplate)
- ✅ Manual impl when you need custom logic (full control)

**Examples:**
- `apps/team-metrics-macro` - Shows derive approach
- `apps/team-metrics-custom` - Shows manual approach
- Both have identical functionality, different implementation

---

## Decision 5: Sets Can't Contain CRDTs

### What We Chose

```rust
impl CrdtMeta for UnorderedSet {
    fn can_contain_crdts() -> bool {
        false  // By design
    }
}
```

### Alternatives Considered

**Allow CRDTs in sets:**
```rust
Set<Counter>  // Possible?
```

**Why we didn't:** 
- Sets test equality: `contains(value)`
- CRDTs need merging, not equality
- Ambiguous semantics

### The Right Pattern

```rust
// ❌ Don't: Set<Counter>
// How would you test membership? counter == other_counter?

// ✅ Do: Map<K, Counter>
// Clear identity (key) + mergeable value
```

**Verdict:** Sets for simple values, Maps for CRDTs.

---

## Decision 5: LWW for Primitives

### What We Chose

```rust
// String, u64, etc. get LWW merge:
impl Mergeable for String {
    fn merge(&mut self, other: &Self) {
        *self = other.clone();  // Just take the other value
    }
}
```

### Alternatives Considered

**Require LwwRegister everywhere:**
```rust
// Force developers to use:
field: LwwRegister<String>

// Instead of:
field: String
```

**Why we didn't:** Too verbose, LWW is reasonable default

### Trade-offs

| Approach                | Verbosity   | Timestamps   | Correctness   |
| ----------------------- | ----------- | ------------ | ------------- |
| **Auto LWW**            | Low         | Implicit     | Good enough   |
| **Require LwwRegister** | High        | Explicit     | Better        |

**Verdict:** Auto LWW now, can be improved in Phase 3.

### Future Enhancement

Phase 3: Auto-wrap non-CRDTs in LwwRegister
```rust
// Developer writes: field: String
// Macro transforms to: field: LwwRegister<String>
// API stays same via Deref
```

---

## Decision 6: Registration Hook on WASM Load

### What We Chose

```rust
// Macro generates:
#[no_mangle]
pub extern "C" fn __calimero_register_merge() {
    register_crdt_merge::<MyApp>();
}

// Runtime calls on WASM load:
if let Ok(hook) = instance.get_function("__calimero_register_merge") {
    hook.call()?;
}
```

### Alternatives Considered

**Compile-time registration:**
- Generate Rust code that imports all apps
- Register at compile time
- **Problem:** Can't work across WASM boundary

**Manual registration:**
```rust
fn init() {
    register_crdt_merge::<MyApp>();  // Developer writes this
    // ... actual init
}
```
- **Problem:** Easy to forget, boilerplate

**Static constructor:**
```rust
#[ctor]
fn register() {
    register_crdt_merge::<MyApp>();
}
```
- **Problem:** Doesn't work in WASM

### Verdict

Runtime hook is the only approach that:
- ✅ Works in WASM
- ✅ Automatic (no developer code)
- ✅ Happens exactly once
- ✅ Backward compatible (optional)

---

## Decision 7: Merge Registry Over Trait Objects

### What We Chose

**Global registry with type-erased merge functions:**

```rust
static MERGE_REGISTRY: RwLock<HashMap<
    TypeId,
    fn(&[u8], &[u8], u64, u64) -> Result<Vec<u8>>
>> = ...;
```

### Alternatives Considered

**Trait objects:**
```rust
dyn Mergeable
```
**Problem:** Can't deserialize to dyn trait

**Generic storage:**
```rust
fn merge<T: Mergeable>(ours: &[u8], theirs: &[u8]) -> Result<Vec<u8>>
```
**Problem:** Type information lost at runtime

### Verdict

Registry is the only approach that works with:
- ✅ Runtime type dispatch
- ✅ Serialized bytes (no type info)
- ✅ WASM boundary

---

## Decision 8: Fallback to LWW

### What We Chose

```rust
fn merge_root_state(ours: &[u8], theirs: &[u8]) -> Result<Vec<u8>> {
    // Try CRDT merge first
    if let Some(merged) = try_merge_registered(ours, theirs)? {
        return Ok(merged);
    }
    
    // Fallback to LWW
    if their_timestamp >= our_timestamp {
        Ok(theirs.to_vec())
    } else {
        Ok(ours.to_vec())
    }
}
```

### Alternatives Considered

**Require registration:**
- Panic if merge not registered
- Force all apps to have Mergeable

**Why we didn't:** Backward compatibility!

**Manual conflict resolution:**
- Stop sync, ask developer to resolve
- **Why we didn't:** CRDTs are about automatic resolution

### Verdict

Graceful fallback enables:
- ✅ Backward compatibility
- ✅ Gradual migration
- ✅ Old apps keep working

---

## Decision 9: Field Detection by Type Name

### What We Chose

**String matching on type paths:**

```rust
fn is_crdt_type(ty: &Type) -> bool {
    let type_str = quote!(#ty).to_string();
    type_str.contains("UnorderedMap")
|  |
|  |
        // ...
}
```

### Alternatives Considered

**Trait bounds:**
```rust
#[app::state]
pub struct MyApp {
    #[crdt]  // ← Explicit annotation
    data: UnorderedMap<...>,
}
```
**Why we didn't:** More boilerplate

**Proc macro helper attributes:**
```rust
#[derive(AppState)]
#[merge(field1, field2)]  // Which fields to merge
```
**Why we didn't:** Redundant, all CRDTs should merge

### Verdict

String matching works:
- ✅ Zero annotations
- ✅ Works for all CRDT types
- ⚠️ Fragile if types renamed (low risk)

---

## Decision 10: Unlimited Nesting Depth

### What We Chose

**No limit on nesting depth:**

```rust
Map<K, Map<K2, Map<K3, Map<K4, V>>>>  // Works!
```

### Alternatives Considered

**Limit to 3 levels:**
- Prevent overly complex structures
- Faster merge

**Why we didn't:** Let developers decide structure

### Verdict

Unlimited depth:
- ✅ Flexibility
- ✅ Handles any use case
- ⚠️ Can be slow if too deep (developer's choice)

**Guidance:** 2-3 levels is sweet spot, but we don't enforce it.

---

## Lessons Learned

### 1. Simple Beats Complex

Element IDs are simpler than full OT and solve 95% of conflicts.

### 2. Automatic Beats Manual

Zero-burden DX is worth the macro complexity.

### 3. Graceful Degradation

Fallback to LWW enables backward compatibility.

### 4. Test Everything

268 tests caught edge cases that would've been bugs.

### 5. Document The "Why"

This document exists so future maintainers understand our choices.

---

## Decision 11: Single Collection Per Entity

### The Design

**Each entity stores at most one children list:**

```rust
pub struct EntityIndex {
    id: Id,
    parent_id: Option<Id>,
    children: Option<Vec<ChildInfo>>,  // Simple list, no names
    full_hash: [u8; 32],
    own_hash: [u8; 32],
    //...
}
```

### Why This Works

In production, entities have exactly one type of children:

```rust
// Library collections store their entries:
UnorderedMap → has children (map entries)
Vector → has children (vector elements)  
Counter → has children (per-node counts)

// App state stores its CRDTs:
MyApp → has children (the root-level collections)

// Nested structures:
MyApp.documents (Map) → has children (document entries)
Document.metadata (Map) → has children (metadata entries)
```

Each entity has ONE collection of children. No entity ever has both "paragraphs" AND "images" as child types.

### API Design

**Collection parameter exists but is ignored:**

```rust
// User code:
add_child_to(parent_id, &paragraphs_collection, &child)
get_children_of(parent_id, &paragraphs_collection)

// Internally:
// - "paragraphs" param ignored
// - Just add/get from the single children list
// - Kept for API backwards compatibility
```

### Benefits

- **Storage**: Minimal overhead (~1 byte for Option discriminant)
- **Performance**: Direct Vec access, no BTreeMap lookups
- **Correctness**: Matches actual production usage patterns
- **Simplicity**: No collection name bookkeeping

---

## Future Decisions

See [TODO.md](../../../TODO.md) for upcoming design decisions:

- Auto-wrap non-CRDT fields in LwwRegister?
- Implement full OT for vectors?
- Add schema validation?

---

## See Also

- [Architecture](architecture.md) - How the system works
- [Performance](performance.md) - Performance analysis
- [TODO.md](../../../TODO.md) - Future work
