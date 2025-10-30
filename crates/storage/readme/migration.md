# Migration Guide

How to upgrade from manual flattening to automatic nested CRDT support.

---

## Do You Need to Migrate?

### If Your App Uses Manual Flattening

**Pattern: Composite keys for nested data**

```rust
// Old approach
#[app::state]
pub struct MyApp {
    metadata: UnorderedMap<String, String>,
}

// Keys like: "doc-1:title", "doc-1:author", "doc-2:title"
```

**You can:**
- ✅ **Keep it** - Works perfectly with auto-merge!
- ✅ **Migrate** - To natural nesting (optional)
- ✅ **Mix both** - Some fields flat, some nested

### If Your App Already Uses #[app::state]

**You already have automatic merge!** Just use natural nesting for new fields.

---

## Migration Strategies

### Strategy 1: Keep Current Structure (Recommended)

**No migration needed!** Your app already benefits from automatic merge.

```rust
// Current code:
#[app::state]
pub struct MyApp {
    metadata: UnorderedMap<String, String>,  // "doc-1:title" keys
}

// Still works! Auto-merge already enabled!
// You can keep using composite keys if you prefer.
```

**Pros:**
- ✅ Zero work
- ✅ No risk
- ✅ Already tested

**Cons:**
- ⚠️ Less intuitive structure
- ⚠️ Manual key management

---

### Strategy 2: Gradual Migration

**Add new fields with nesting, keep old fields flat:**

```rust
#[app::state]
pub struct MyApp {
    // Old fields (keep as-is)
    metadata: UnorderedMap<String, String>,  // Composite keys
    
    // New fields (use nesting)
    documents: UnorderedMap<String, Document>,  // Natural nesting
}

pub struct Document {
    content: ReplicatedGrowableArray,
    title: LwwRegister<String>,
}
```

**Pros:**
- ✅ Low risk
- ✅ Learn gradually
- ✅ Both patterns work

**Cons:**
- ⚠️ Mixed styles

---

### Strategy 3: Full Refactor

**Rewrite state to use natural nesting:**

```rust
// Before:
#[app::state]
pub struct MyApp {
    doc_titles: Map<String, String>,      // "doc-1" → "Title"
    doc_authors: Map<String, String>,     // "doc-1" → "Alice"
    doc_contents: Map<String, RGA>,       // "doc-1" → RGA
}

// After:
#[app::state]
pub struct MyApp {
    documents: Map<String, Document>,
}

pub struct Document {
    title: LwwRegister<String>,
    author: LwwRegister<String>,
    content: ReplicatedGrowableArray,
}
```

**Pros:**
- ✅ Cleaner code
- ✅ Better semantics
- ✅ Easier to understand

**Cons:**
- ⚠️ Requires data migration
- ⚠️ Need to rewrite methods
- ⚠️ Testing required

---

## Step-by-Step: Refactoring Example

### 1. Current State (Composite Keys)

```rust
#[app::state]
pub struct DocumentApp {
    metadata: UnorderedMap<String, String>,
}

impl DocumentApp {
    pub fn set_title(&mut self, doc_id: String, title: String) {
        self.metadata.insert(format!("{}:title", doc_id), title)?;
    }
    
    pub fn get_title(&self, doc_id: String) -> Option<String> {
        self.metadata.get(&format!("{}:title", doc_id))?
    }
}
```

### 2. Define Nested Structure

```rust
pub struct Document {
    title: LwwRegister<String>,
    author: LwwRegister<String>,
    created_at: u64,
}
```

### 3. Update App State

```rust
#[app::state]
pub struct DocumentApp {
    documents: UnorderedMap<String, Document>,
}
```

### 4. Implement Mergeable for Document

```rust
impl Mergeable for Document {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.title.merge(&other.title)?;
        self.author.merge(&other.author)?;
        // created_at: use LWW (or wrap in LwwRegister)
        if other.created_at > self.created_at {
            self.created_at = other.created_at;
        }
        Ok(())
    }
}
```

### 5. Update Methods

```rust
impl DocumentApp {
    pub fn set_title(&mut self, doc_id: String, title: String) {
        let mut doc = self.documents.get(&doc_id)?
            .unwrap_or_else(|| Document::default());
        doc.title.set(title);
        self.documents.insert(doc_id, doc)?;
    }
    
    pub fn get_title(&self, doc_id: String) -> Option<String> {
        self.documents.get(&doc_id)?
            .map(|doc| doc.title.get().clone())
    }
}
```

### 6. Data Migration

```rust
fn migrate_to_nested() -> Result<(), Error> {
    // Read old format
    let old_app: OldDocumentApp = load_state()?;
    
    // Convert to new format
    let mut new_app = DocumentApp {
        documents: UnorderedMap::new(),
    };
    
    // Group by document ID
    let mut docs = HashMap::new();
    for (composite_key, value) in old_app.metadata.entries()? {
        let (doc_id, field) = parse_composite_key(&composite_key);
        docs.entry(doc_id).or_insert_with(HashMap::new).insert(field, value);
    }
    
    // Create Document structs
    for (doc_id, fields) in docs {
        let doc = Document {
            title: LwwRegister::new(fields.get("title").cloned().unwrap_or_default()),
            author: LwwRegister::new(fields.get("author").cloned().unwrap_or_default()),
            created_at: fields.get("created_at").and_then(|s| s.parse().ok()).unwrap_or(0),
        };
        new_app.documents.insert(doc_id, doc)?;
    }
    
    // Save new format
    save_state(&new_app)?;
    Ok(())
}
```

---

## Migration Checklist

Before migrating:
- [ ] Read current state structure
- [ ] Design new nested structure
- [ ] Write Mergeable for nested types
- [ ] Update all methods
- [ ] Write data migration script
- [ ] Test migration with sample data
- [ ] Test merge behavior
- [ ] Backup production data
- [ ] Run migration
- [ ] Verify no divergence

---

## Testing Migration

```rust
#[test]
fn test_migration_preserves_data() {
    // Create old format
    let mut old_app = OldApp { metadata: Map::new() };
    old_app.metadata.insert("doc-1:title", "Title 1")?;
    old_app.metadata.insert("doc-1:author", "Alice")?;
    
    // Migrate
    let new_app = migrate(old_app)?;
    
    // Verify
    let doc = new_app.documents.get(&"doc-1")?.unwrap();
    assert_eq!(doc.title.get(), "Title 1");
    assert_eq!(doc.author.get(), "Alice");
}

#[test]
fn test_migration_merge_still_works() {
    let app1 = migrate(old_app_1)?;
    let app2 = migrate(old_app_2)?;
    
    app1.merge(&app2)?;
    
    // Verify: No divergence
    let bytes1 = borsh::to_vec(&app1)?;
    let bytes2 = borsh::to_vec(&app2)?;
    assert_eq!(compute_hash(&bytes1), compute_hash(&bytes2));
}
```

---

## Common Migration Patterns

### From Separate Maps to Nested Structure

**Before:**
```rust
titles: Map<String, String>,
authors: Map<String, String>,
contents: Map<String, RGA>,
```

**After:**
```rust
documents: Map<String, Document>,

struct Document {
    title: LwwRegister<String>,
    author: LwwRegister<String>,
    content: RGA,
}
```

### From Flat Keys to Nested Maps

**Before:**
```rust
data: Map<String, String>,
// Keys: "user-1:setting-1", "user-1:setting-2"
```

**After:**
```rust
data: Map<String, Map<String, String>>,
// Outer key: "user-1", Inner keys: "setting-1", "setting-2"
```

---

## Rollback Plan

If something goes wrong:

### Option 1: Keep Both Versions

```rust
#[app::state]
pub struct MyApp {
    // Old format (fallback)
    metadata_v1: Map<String, String>,
    
    // New format (primary)
    documents_v2: Map<String, Document>,
    
    version: u32,  // Track which to use
}
```

### Option 2: Feature Flag

```rust
#[cfg(feature = "nested-crdts")]
type AppState = NewAppState;

#[cfg(not(feature = "nested-crdts"))]
type AppState = OldAppState;
```

### Option 3: Restore from Backup

```bash
# Before migration
cargo mero backup --context <context-id>

# If migration fails
cargo mero restore --context <context-id> --from backup.dat
```

---

## Post-Migration Validation

### 1. Verify Data Integrity

```rust
// Check all documents migrated
assert_eq!(new_app.documents.len()?, old_app.count_documents());

// Spot-check values
for doc_id in sample_ids {
    let old_title = old_app.get_title(doc_id)?;
    let new_title = new_app.documents.get(&doc_id)?.map(|d| d.title.get());
    assert_eq!(old_title, new_title);
}
```

### 2. Monitor for Divergence

```bash
# Check logs for divergence errors
grep "DIVERGENCE" logs/*.log

# Should be ZERO with proper CRDT merge!
```

### 3. Performance Testing

```rust
// Measure merge performance
let start = Instant::now();
for _ in 0..100 {
    simulate_concurrent_update();
    app1.merge(&app2)?;
}
let avg_merge = start.elapsed() / 100;

// Should be < 10ms for most apps
assert!(avg_merge < Duration::from_millis(10));
```

---

## FAQ

**Q: Do I have to migrate?**  
A: No! Both patterns work. Migrate if you want cleaner code.

**Q: Will migration cause downtime?**  
A: Depends on your deployment. Plan for a maintenance window.

**Q: What if migration fails?**  
A: Restore from backup. Test thoroughly before production migration.

**Q: Can I migrate gradually?**  
A: Yes! Use Strategy 2 (keep old fields, add new nested fields).

**Q: Will this break my app?**  
A: Not if you test! The CRDT semantics are the same, just better structured.

---

## Success Stories

### Example: Collaborative Editor

**Before:** 150 lines of key management code  
**After:** 50 lines of natural structure  
**Divergence:** 0 (down from occasional issues)  
**Merge time:** < 2ms (acceptable)

### Example: Analytics Dashboard

**Before:** Flat structure with 50+ root fields  
**After:** Nested structure with 5 root fields  
**Merge time:** 10ms → 2ms (5× faster!)

---

## Need Help?

- Open an issue with your current structure
- We can suggest migration path
- Community examples available

---

## See Also

- [Collections API](collections.md) - API reference
- [Nesting Guide](nesting.md) - How to use nested structures
- [Architecture](architecture.md) - How merging works

