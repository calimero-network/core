# Schema Inference and Field Metadata

How Calimero Storage enables schema-free database inspection.

---

## Overview

Calimero Storage supports **schema inference** - the ability to inspect and visualize state databases without requiring an external schema file. This is achieved by storing **field names** in entity metadata.

---

## How It Works

### Field Name Storage

When you use `#[app::state]` to define your app state, the macro automatically generates a `Default` implementation that uses `new_with_field_name()` for each collection:

```rust
#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct MyApp {
    items: UnorderedMap<String, String>,      // field_name = "items"
    operation_count: Counter,                  // field_name = "operation_count"
    tags: UnorderedSet<String>,               // field_name = "tags"
}
```

**Generated `Default` implementation:**

```rust
impl Default for MyApp {
    fn default() -> Self {
        Self {
            items: UnorderedMap::new_with_field_name("items"),
            operation_count: Counter::new_with_field_name("operation_count"),
            tags: UnorderedSet::new_with_field_name("tags"),
        }
    }
}
```

### Metadata Structure

Each entity's metadata (`Metadata` struct) includes:

```rust
pub struct Metadata {
    pub created_at: u64,
    pub updated_at: UpdatedAt,
    pub storage_type: StorageType,
    pub crdt_type: Option<CrdtType>,   // Counter, UnorderedMap, Vector, etc.
    pub field_name: Option<String>,     // "items", "tags", etc.
}
```

### EntityIndex Storage

The `field_name` is persisted in the `EntityIndex` for each collection root:

```
EntityIndex {
    id: <collection_id>,
    parent_id: <root_state_id>,
    metadata: {
        crdt_type: Some(UnorderedMap),
        field_name: Some("items"),
        ...
    },
    ...
}
```

---

## Using Schema Inference

### With merodb GUI

The `merodb gui` tool can now visualize state **without a schema file**:

```bash
# Start the GUI - schema file is optional!
merodb gui
```

When no schema file is provided, merodb:
1. Scans the database for `EntityIndex` entries
2. Identifies root-level fields by checking `parent_id`
3. Reads `field_name` and `crdt_type` from metadata
4. Builds a schema dynamically

### With CLI Export

```bash
# Schema file is now optional
merodb export --db-path /path/to/data --context-id <id>

# Or specify schema explicitly (takes precedence)
merodb export --db-path /path/to/data --context-id <id> --state-schema-file schema.json
```

---

## Benefits

### 1. Zero-Configuration Inspection

Developers can inspect any Calimero database without needing the original app's schema:

```bash
# Just point to the database
merodb gui
# → Select database path
# → Select context
# → View state tree!
```

### 2. Migration Support

Field names enable safe schema migrations:

- **Identify fields:** Know what each entity represents
- **Track changes:** Detect added/removed fields
- **Validate migrations:** Ensure data integrity

### 3. Debugging

Better debugging experience:

- **Clear labels:** See "items" instead of truncated hashes
- **Type information:** Know if a field is a Counter vs Map
- **Structure visualization:** Understand the state tree hierarchy

---

## Backward Compatibility

### Old Data (No field_name)

Data written before `field_name` was added deserializes correctly:

```rust
// Old format: field_name defaults to None
let deserialized: Metadata = borsh::from_slice(&old_bytes)?;
assert_eq!(deserialized.field_name, None);  // Safe default
```

### Mixed Environments

- **New collections:** Have `field_name` set
- **Old collections:** `field_name` is `None`
- **Schema inference:** Falls back to sequential matching for old data

---

## Deterministic Collection IDs

Collections created with `new_with_field_name()` get **deterministic IDs**:

```rust
fn compute_collection_id(parent_id: Option<Id>, field_name: &str) -> Id {
    let mut hasher = Sha256::new();
    if let Some(parent) = parent_id {
        hasher.update(parent.as_bytes());
    }
    hasher.update(field_name.as_bytes());
    Id::new(hasher.finalize().into())
}
```

**Benefits:**
- Same collection gets same ID across all nodes
- Enables reliable sync without random IDs
- Predictable for testing and debugging

---

## Collection Types with field_name

All CRDT collections support `new_with_field_name()`:

| Collection | Method | CRDT Type Stored |
|------------|--------|------------------|
| `UnorderedMap` | `new_with_field_name("items")` | `CrdtType::UnorderedMap` |
| `Vector` | `new_with_field_name("history")` | `CrdtType::Vector` |
| `UnorderedSet` | `new_with_field_name("tags")` | `CrdtType::UnorderedSet` |
| `Counter` | `new_with_field_name("count")` | `CrdtType::Counter` |
| `ReplicatedGrowableArray` | `new_with_field_name("text")` | `CrdtType::Rga` |
| `UserStorage` | `new_with_field_name("user_data")` | `CrdtType::UserStorage` |
| `FrozenStorage` | `new_with_field_name("frozen_data")` | `CrdtType::FrozenStorage` |

---

## Advanced: Manual Field Names

For advanced users who want custom field names:

```rust
// Don't derive Default - implement manually
impl MyApp {
    pub fn new() -> Self {
        Self {
            // Custom field name
            items: UnorderedMap::new_with_field_name("custom_items_name"),
            // Regular creation (no field_name)
            temp_data: UnorderedMap::new(),
        }
    }
}
```

**Note:** When using manual implementation, ensure you call `new_with_field_name()` for collections you want to be discoverable by schema inference.

---

## Limitations

### 1. Type Parameters Not Inferred

Schema inference knows `items` is an `UnorderedMap`, but cannot determine:
- Key type (`String`, `u64`, etc.)
- Value type (`LwwRegister<String>`, custom struct, etc.)

**Workaround:** Values are displayed as best-effort decoded data.

### 2. Inline Types (LwwRegister)

`LwwRegister` fields don't create separate `EntityIndex` entries - they're serialized inline with the parent. This means:
- `LwwRegister` fields won't appear in schema inference
- Their values are part of the parent entity's data

---

## See Also

- [Collections API](collections.md) - All collection types
- [Architecture](architecture.md) - How storage works internally
- [Migration Guide](migration.md) - Upgrading existing apps

---

**Last Updated:** 2026-02-04  
**Version:** 0.12.0
