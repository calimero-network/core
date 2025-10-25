# Calimero Storage

A CRDT-based hierarchical storage system with automatic synchronization and conflict resolution.

## Quick Start

```rust
use calimero_storage::{Root, UnorderedMap, Vector};

// Define your data structures
#[derive(AtomicUnit, BorshSerialize, BorshDeserialize)]
struct Todo {
    title: String,
    completed: bool,
    #[storage]
    storage: Element,
}

// Use collections
let mut state = Root::new(|| UnorderedMap::new());
state.insert("todo-1".to_string(), Todo { 
    title: "Learn Calimero".to_string(), 
    completed: false,
    storage: Element::new(&path, None),
})?;
state.commit();
```

## Architecture Overview

### System Architecture

```mermaid
graph TB
    subgraph "Application Layer"
        A[User Code] -->|uses| B[Collections: Vector, Map, Set]
    end
    
    subgraph "Storage Layer"
        B -->|backed by| C[Interface API]
        C -->|manages| D[Entities: Element, Data]
        D -->|persists to| E[Index + Entry Storage]
    end
    
    subgraph "Sync Layer"
        C -->|generates| F[CRDT Actions]
        F -->|propagates| G[Remote Nodes]
        G -->|sends| H[Comparison Data]
        H -->|triggers| C
    end
    
    subgraph "Database"
        E -->|stored in| I[RocksDB]
        I -->|Key::Index| J[Metadata + Merkle Hashes]
        I -->|Key::Entry| K[Borsh-serialized Data]
    end
    
    style A fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style B fill:#FADBD8,stroke:#C0392B,stroke-width:2px
    style C fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style D fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style E fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style F fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style G fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style H fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style I fill:#E8DAEF,stroke:#7D3C98,stroke-width:2px
    style J fill:#E8DAEF,stroke:#7D3C98,stroke-width:2px
    style K fill:#E8DAEF,stroke:#7D3C98,stroke-width:2px
```

### Hybrid CRDT Model

```mermaid
sequenceDiagram
    participant User
    participant Local as Local Node
    participant Sync as Sync Layer
    participant Remote as Remote Node
    
    Note over Local,Remote: Primary: Operation-based (CmRDT)
    User->>Local: Modify data
    Local->>Local: Save + calculate hash
    Local->>Sync: Generate Action
    Sync->>Remote: Propagate Action
    Remote->>Remote: Apply action
    
    Note over Local,Remote: Fallback: Comparison (CvRDT)
    Remote->>Local: Request comparison
    Local->>Local: Generate comparison data
    Local->>Remote: Send hashes + metadata
    Remote->>Remote: Compare Merkle trees
    Remote->>Local: Send diff actions
    Local->>Local: Apply actions
```

Calimero uses a **hybrid approach** combining operation-based and state-based CRDTs:

1. **Operation-based (CmRDT)**: Local changes emit `Action`s that propagate to peers
2. **State-based (CvRDT)**: Merkle tree comparison for catch-up and reconciliation

This provides:
- ✅ Efficient operation propagation (no full state transfer)
- ✅ Reliable catch-up when nodes miss updates (offline, packet loss)
- ✅ Automatic conflict resolution via last-write-wins
- ✅ Partial replication support

### Merkle Hashing

```mermaid
graph TB
    Root["Root Element<br/>full_hash = H(H1 + H456 + H789)<br/>own_hash = H1"]
    
    Root -->|child| Coll1["Collection A<br/>full_hash = H(H4 + H41 + H42)<br/>own_hash = H4"]
    Root -->|child| Coll2["Collection B<br/>full_hash = H(H7 + H71)<br/>own_hash = H7"]
    
    Coll1 -->|child| Item1["Item 1<br/>full_hash = H41<br/>own_hash = H41"]
    Coll1 -->|child| Item2["Item 2<br/>full_hash = H42<br/>own_hash = H42"]
    
    Coll2 -->|child| Item3["Item 3<br/>full_hash = H71<br/>own_hash = H71"]
    
    Note1["Own Hash = SHA256(data)"]
    Note2["Full Hash = SHA256(own_hash + child_hashes)"]
    
    style Root fill:#FADBD8,stroke:#C0392B,stroke-width:2px
    style Coll1 fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style Coll2 fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style Item1 fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style Item2 fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style Item3 fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style Note1 fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style Note2 fill:#FCF3CF,stroke:#D68910,stroke-width:2px
```

Each entity maintains two hashes:
- **Own hash**: SHA-256 of immediate data
- **Full hash**: Combined hash including all descendants

This enables efficient tree comparison—only subtrees with differing hashes need examination.

### Storage Layout

```mermaid
graph LR
    subgraph "RocksDB Keys"
        K1["Key::Index(id)<br/>SHA256(0x00 + id)"]
        K2["Key::Entry(id)<br/>SHA256(0x01 + id)"]
    end
    
    subgraph "Stored Values"
        V1["EntityIndex {<br/>  parent_id,<br/>  children: Map,<br/>  full_hash,<br/>  own_hash,<br/>  metadata<br/>}"]
        V2["Borsh-serialized<br/>user data"]
    end
    
    K1 -.->|stores| V1
    K2 -.->|stores| V2
    
    style K1 fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style K2 fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style V1 fill:#FADBD8,stroke:#C0392B,stroke-width:2px
    style V2 fill:#D5F4E6,stroke:#229954,stroke-width:2px
```

**Two-tier key structure:**
- `Key::Index(id)` → Metadata, parent/child relationships, Merkle hashes
- `Key::Entry(id)` → Actual user data (Borsh-serialized)

**Deterministic IDs**: Collection items use `SHA256(parent_id + key)` for O(1) lookups:

```mermaid
graph LR
    A["map.get('user_123')"] --> B[Compute ID]
    B --> C["SHA256(collection_id + 'user_123')"]
    C --> D[Direct RocksDB lookup]
    D --> E[Return value]
    
    style A fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style B fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style C fill:#FADBD8,stroke:#C0392B,stroke-width:2px
    style D fill:#E8DAEF,stroke:#7D3C98,stroke-width:2px
    style E fill:#D5F4E6,stroke:#229954,stroke-width:2px
```

**Tradeoff**: Hashed IDs prevent RocksDB range scans. Iteration fetches child IDs from the index, then point-lookups each item.

## API Overview

### Core Interface

**Data Operations:**
```rust
Interface::save(&mut entity)                    // Save/update entity
Interface::find_by_id::<T>(id)                  // Direct lookup by ID
Interface::add_child_to(parent_id, coll, child) // Add to collection
Interface::remove_child_from(parent_id, coll, id) // Remove from collection
```

**Synchronization:**
```rust
Interface::apply_action(action)                        // Execute sync action
Interface::compare_trees(foreign_data, comparison_data) // Generate sync actions
```

**Queries:**
```rust
Interface::children_of(parent_id, collection)  // Get collection items
Interface::parent_of(child_id)                 // Navigate hierarchy
```

### Collections

Built-in persistent collections:

- **`Vector<T>`** - Ordered, index-based list
- **`UnorderedMap<K, V>`** - Key-value map with deterministic IDs
- **`UnorderedSet<T>`** - Unique values
- **`Root<T>`** - Special root state container

All collections:
- Serialize with Borsh
- Store metadata (timestamps, hashes)
- Support iteration and standard operations
- Auto-sync via CRDT actions

## Implementation Details

### CRDT Synchronization

**Direct Changes Flow:**

```mermaid
flowchart TD
    A[User modifies data] --> B[Mark element dirty]
    B --> C[Interface.save]
    C --> D[Serialize with Borsh]
    D --> E[Calculate own_hash = SHA256 data]
    E --> F[Update index + entry in DB]
    F --> G[Calculate full_hash recursively]
    G --> H[Generate Action Add/Update/Delete]
    H --> I[Push to sync queue]
    I --> J[Propagate to peers]
    
    style A fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style B fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style C fill:#FADBD8,stroke:#C0392B,stroke-width:2px
    style D fill:#FADBD8,stroke:#C0392B,stroke-width:2px
    style E fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style F fill:#E8DAEF,stroke:#7D3C98,stroke-width:2px
    style G fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style H fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style I fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style J fill:#D5F4E6,stroke:#229954,stroke-width:2px
```

**Comparison Flow:**

```mermaid
flowchart TD
    A[Receive comparison data] --> B{Compare full hashes}
    B -->|Match| Z[Done - in sync]
    B -->|Differ| C{Compare own hashes}
    C -->|Differ| D{Compare timestamps}
    D -->|Remote newer| E[Action: Update local]
    D -->|Local newer| F[Action: Update remote]
    C -->|Same| G[Check children]
    G --> H{Child hash differs?}
    H -->|Yes| I[Action: Compare child recursively]
    H -->|No| J[Check next child]
    I --> K[Apply actions]
    
    style A fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style B fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style C fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style D fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style E fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style F fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style G fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style H fill:#FCF3CF,stroke:#D68910,stroke-width:2px
    style I fill:#FADBD8,stroke:#C0392B,stroke-width:2px
    style J fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style K fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style Z fill:#D5F4E6,stroke:#229954,stroke-width:2px
```

**Conflict Resolution**:
- Last-write-wins based on `updated_at` timestamp
- Orphaned children (from out-of-order ops) stored temporarily
- Future comparison reconciles inconsistencies

### Entity Hierarchy

```mermaid
graph TD
    Root["🏠 Root<br/>ID: root_id<br/>Path: ::root"]
    
    CollA["📦 Collection A<br/>ID: random()<br/>Path: ::root::coll_a"]
    CollB["📦 Collection B<br/>ID: random()<br/>Path: ::root::coll_b"]
    
    Item1["📄 Item 1<br/>ID: SHA256(coll_a + key1)<br/>Path: ::root::coll_a::item1"]
    Item2["📄 Item 2<br/>ID: SHA256(coll_a + key2)<br/>Path: ::root::coll_a::item2"]
    
    Item3["📄 Item 3<br/>ID: SHA256(coll_b + key3)<br/>Path: ::root::coll_b::item3"]
    
    SubColl["📦 SubCollection<br/>ID: random()<br/>Path: ::root::coll_b::sub"]
    Item4["📄 Item 4<br/>ID: SHA256(sub + key4)<br/>Path: ::root::coll_b::sub::item4"]
    
    Root --> CollA
    Root --> CollB
    CollA --> Item1
    CollA --> Item2
    CollB --> Item3
    CollB --> SubColl
    SubColl --> Item4
    
    style Root fill:#FADBD8,stroke:#C0392B,stroke-width:3px
    style CollA fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style CollB fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style SubColl fill:#D6EAF8,stroke:#2874A6,stroke-width:2px
    style Item1 fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style Item2 fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style Item3 fill:#D5F4E6,stroke:#229954,stroke-width:2px
    style Item4 fill:#D5F4E6,stroke:#229954,stroke-width:2px
```

**Each entity stores:**
- Unique ID (32-byte) - Random for collections, deterministic for map/set items
- Parent ID (in EntityIndex)
- Children list (by collection name)
- Own hash (SHA256 of data)
- Full hash (SHA256 of own_hash + child_hashes)
- Metadata (created_at, updated_at timestamps)


## Background and Purpose

Within the Calimero Network we want to be able to share data between nodes as a
basic premise. Fundamentally this involves the implementation of checks to
ensure that data is legitimate, along with supportive data structures to aid in
the synchronisation of the data and merging of any changes, plus appropriate
mechanisms to share and propagate the data over the network.

### Features

- ✅ Intervention-free merging with automatic conflict resolution
- ✅ Full propagation of data across the Calimero Network
- ✅ Eventual consistency of general-purpose data
- ✅ Local storage of unshared personal data
- ✅ Partial sharing based on preference or permissions
- ✅ Hierarchical data organization
- ✅ Efficient partial replication

### Design Principles

- **Atomic elements**: Data items are indivisible units
- **Separate metadata**: System properties kept distinct from user data
- **Partial representation**: Support for incomplete data views
- **Hierarchical structure**: Tree-based organization with Merkle validation

## CRDT Theory

### Why Hybrid?

**State-based CRDTs (CvRDTs)**:
- ✅ Simple to implement
- ❌ Require full state transmission (inefficient)
- ❌ Prevent partial replication

**Operation-based CRDTs (CmRDTs)**:
- ✅ Efficient (only transmit operations)
- ✅ Support partial replication
- ❌ Require reliable, ordered delivery

**Calimero's Hybrid**:
- Primary: CmRDT for efficiency
- Fallback: CvRDT comparison for reliability
- Result: Best of both worlds

### CRDT Implementation

Calimero does **not** expose traditional academic CRDT types (GCounter, PNCounter, GSet, TwoPSet, ORSet). Instead, it provides **general-purpose collections** with CRDT semantics built-in:

**Implemented collections:**
- `Vector<T>` - Ordered list with LWW semantics
- `UnorderedMap<K, V>` - Key-value map with deterministic IDs
- `UnorderedSet<T>` - Unique values set
- `Root<T>` - Root state container

**CRDT properties:**
- Last-write-wins conflict resolution (timestamp-based)
- Unique IDs for elements (ORSet-style tagging)
- Merkle tree validation
- Automatic sync via Actions

The underlying mechanism is inspired by **LWWElementSet** but provides a more ergonomic API for application developers.

## Developer Interface

### Low-level Access

Direct element and collection manipulation:

```rust
// By ID (stable across moves)
let entity = Interface::find_by_id::<Todo>(id)?;

// By hierarchy
let children = Interface::children_of(parent_id, &collection)?;
```

### High-level Access

Macro-based structure mapping:

```rust
#[derive(AtomicUnit)]
struct Auction {
    owner_id: Id,

    #[collection]
    bids: Bids,
    
    #[storage]
    storage: Element,
}

#[derive(Collection)]
#[children(Bid)]
struct Bids;

#[derive(AtomicUnit)]
struct Bid {
    price: Decimal,
    time: DateTime<Utc>,
    #[storage]
    storage: Element,
}
```

The storage system handles:
- Serialization/deserialization
- Hierarchy management
- ID assignment
- Merkle hash calculation
- Sync action generation

## Performance Considerations

### Iteration

Collections use index-based iteration (not RocksDB scans due to hashed IDs):

```mermaid
sequenceDiagram
    participant Code as User Code
    participant Coll as Collection
    participant DB as RocksDB
    
    Code->>Coll: iter()
    
    Note over Coll: First call
    Coll->>DB: Get Index(collection_id)
    DB-->>Coll: EntityIndex with child IDs
    Coll->>Coll: Cache child IDs in IndexSet
    
    loop For each child ID
        Coll->>DB: Get Entry(child_id)
        DB-->>Coll: Item data
        Coll-->>Code: Yield item
    end
    
    Note over Coll: Subsequent calls
    Code->>Coll: iter() again
    Coll->>Coll: Use cached IDs
    
    loop For each cached ID
        Coll->>DB: Get Entry(child_id)
        DB-->>Coll: Item data
        Coll-->>Code: Yield item
    end
```

**Cost for 1000 items:**
- 1 index lookup + 1000 individual gets
- (vs. 1 scan with sequential keys)

**Mitigation**: Child IDs cached in memory after first access.

### Merkle Updates

When updating an entity:
```
1. Update own hash
2. Recalculate full hash (own + children)
3. Propagate changes up ancestor chain
```

Cost: O(depth) hash recalculations per update.

## Module Organization

```
storage/
├─ address.rs          # ID and Path types
├─ entities.rs         # Element, Data, AtomicUnit traits
├─ collections.rs      # Base Collection implementation
├─ collections/
│  ├─ vector.rs        # Vector<T> collection
│  ├─ unordered_map.rs # UnorderedMap<K,V> collection
│  ├─ unordered_set.rs # UnorderedSet<T> collection
│  └─ root.rs          # Root<T> state container
├─ interface.rs        # Main storage API
├─ index.rs            # EntityIndex and hierarchy management
├─ store.rs            # StorageAdaptor abstraction
└─ sync.rs             # CRDT action tracking
```

## Testing

```bash
# Run all storage tests
cargo test -p calimero-storage

# Run specific test module
cargo test -p calimero-storage --test interface

# With output
cargo test -p calimero-storage -- --nocapture
```

## Future Improvements

**Current TODOs**:
- [ ] Replace child_info Vec with proper index for better iteration
- [ ] Implement path-based queries (find_by_path)
- [ ] Add validation framework
- [ ] Handle edge case: child added offline while parent updated remotely
- [ ] Implement sharding for large child collections
- [ ] Add garbage collection for deleted entities

See inline comments and issues for details.

## Design Decisions

### Element-Data-AtomicUnit Relationship

**Problem**: How should user types (e.g., `Person`) relate to storage metadata (`Element`)?

**Considered approaches:**

1. **Generate wrapper structs** - Macro creates `PersonData` containing user fields
   - ❌ Leaky abstraction, painful to construct

2. **Element as trait** - User types implement Element directly  
   - ❌ Clutters user types with storage internals

3. **Circular reference** - Element ↔ Data with `Arc<Weak<T>>`
   - ❌ Forces Arc/Mutex, prevents Default impl

4. **Data contains Element** ✅ - User types own an Element field
   - ✅ Simple to use, full ownership
   - ✅ Abstracts storage internals
   - ✅ Supports Default and other traits
   - ✅ No circular references

5. **Generic Element<D>** - Element parameterized by Data type
   - ❌ Unnecessary complexity, phantom data needed

**Chosen**: Option 4 (Data contains Element)

Trade-off: Element can't directly access Data (must be passed in), but this keeps the user interface clean and avoids imposing constraints.

### Collection Implementation Approaches

**Problem**: How should parent-child relationships be expressed?

**Considered approaches:**

1. **Struct-based** - `#[derive(Collection)]` on struct, single child type
   - ❌ Can't have multiple child types

2. **Enum-based** - `enum ChildType { Page(Page), Author(Author) }`
   - ❌ Requires match statements, added complexity

3. **Field-based** ✅ - Fields annotated with `#[collection]`
   - ✅ Most flexible
   - ✅ Easy developer interface
   - ✅ Multiple collection types per parent

**Chosen**: Option 3 (Field-based collections)

Example:
```rust
#[derive(AtomicUnit)]
struct Book {
    title: String,
    
    #[collection]
    pages: Pages,
    
    #[collection]  
    authors: Authors,
    
    #[storage]
    storage: Element,
}
```

### Index vs. Embedded Child Lists

**Current implementation**: Children stored in parent's EntityIndex

**Future improvement**: Dedicated index table for path-based queries

**Rationale**:
- Path is primary addressing mechanism
- Maintaining child list is second point of maintenance
- Index enables better performance for large child sets
- Current approach prioritizes correctness over optimization

**Why no parent ID field?**
- Path is sufficient to determine parent
- Reduces maintenance burden
- Moving elements only requires path update
- Relational DB patterns (NestedSet, AdjacencyList) don't suit key-value stores

## License

See [COPYRIGHT](../../COPYRIGHT) and [LICENSE.md](../../LICENSE.md) in the repository root.
