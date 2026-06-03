//! Worked example: migrating app state and testing it in-process with `TestHost`.
//!
//! This crate is a documentation fixture for `#[app::migrate]`. It defines a v1
//! and a v2 state, a migration between them, and unit tests that run the
//! migration entirely in memory (`cargo test` — no Docker, no merobox),
//! including the cross-node **convergence** check the whole migration model
//! depends on: every node runs the migrate independently, so it must produce a
//! byte-identical v2 root.

use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap, Vector};

/// v1: a titled key/value document.
#[app::state]
pub struct DocV1 {
    entries: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::logic]
impl DocV1 {
    #[app::init]
    pub fn init() -> DocV1 {
        DocV1 {
            entries: UnorderedMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn put(&mut self, key: String, value: String) -> app::Result<()> {
        self.entries.insert(key, value.into())?;
        Ok(())
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }
}

/// v2 adds `tags`, a `Vector` seeded during migration from the sorted v1 keys.
#[app::state]
pub struct DocV2 {
    entries: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
    tags: Vector<LwwRegister<String>>,
}

/// Borsh-only shadow of the v1 layout — what `read_raw()` hands the migration.
#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct DocV1Data {
    entries: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> DocV2 {
    let old_bytes = read_raw().unwrap_or_else(|| panic!("migrate: no existing state"));
    let old: DocV1Data = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| panic!("migrate: v1 deserialize: {e:?}"));

    // Seed `tags` from the entry keys. Sort first: every node must build the
    // Vector in the same order, or the migrated roots diverge.
    let mut keys: Vec<String> = old
        .entries
        .entries()
        .unwrap_or_else(|e| panic!("migrate: iterate entries: {e:?}"))
        .map(|(k, _)| k)
        .collect();
    keys.sort();

    let mut tags: Vector<LwwRegister<String>> = Vector::new();
    for k in keys {
        tags.push(k.into())
            .unwrap_or_else(|e| panic!("migrate: seed tag: {e:?}"));
    }

    DocV2 {
        entries: old.entries,
        title: old.title,
        tags,
    }
}

#[app::logic]
impl DocV2 {
    #[app::init]
    pub fn init() -> DocV2 {
        DocV2 {
            entries: UnorderedMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
            tags: Vector::new(),
        }
    }

    pub fn tag_count(&self) -> app::Result<u64> {
        Ok(self.tags.len()? as u64)
    }

    pub fn title(&self) -> app::Result<String> {
        Ok(self.title.get().clone())
    }
}

/// The same v1->v2 shape (carry `entries`/`title`, add a new field) written with
/// `#[derive(Migrate)]` instead of a hand-written `#[app::migrate]`. The derive
/// generates `derived_migrate()`: `entries`/`title` are carried by name, `note`
/// is seeded. Compare to `migrate_v1_to_v2` above — same behaviour, no
/// read/deserialize/carry boilerplate.
#[app::state]
#[derive(app::Migrate)]
#[migrate(from = DocV1Data, method = derived_migrate)]
pub struct DocV2Derived {
    entries: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
    #[migrate(new = LwwRegister::new("seeded".to_owned()))]
    note: LwwRegister<String>,
}

#[app::logic]
impl DocV2Derived {
    #[app::init]
    pub fn init() -> DocV2Derived {
        DocV2Derived {
            entries: UnorderedMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
            note: LwwRegister::new(String::new()),
        }
    }

    pub fn entry_count(&self) -> app::Result<u64> {
        Ok(self.entries.len()? as u64)
    }

    pub fn note(&self) -> app::Result<String> {
        Ok(self.note.get().clone())
    }

    pub fn title(&self) -> app::Result<String> {
        Ok(self.title.get().clone())
    }
}

/// Exercises rename + drop: `title` is renamed to `heading`, and v1's `entries`
/// is dropped (simply absent from this struct — no annotation needed).
#[app::state]
#[derive(app::Migrate)]
#[migrate(from = DocV1Data, method = renamed_migrate)]
pub struct DocV2Renamed {
    #[migrate(from = title)]
    heading: LwwRegister<String>,
}

#[app::logic]
impl DocV2Renamed {
    #[app::init]
    pub fn init() -> DocV2Renamed {
        DocV2Renamed {
            heading: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn heading(&self) -> app::Result<String> {
        Ok(self.heading.get().clone())
    }
}

/// Exercises the `with` (transform a field) and `emit` (emit an event) hooks of
/// `#[derive(Migrate)]`: `title` is transformed to upper-case via `with`, and a
/// `Migrated` event is emitted via `emit`.
#[app::event]
pub enum MigrateEvent<'a> {
    Migrated { from: &'a str, to: &'a str },
}

fn uppercase(reg: LwwRegister<String>) -> LwwRegister<String> {
    LwwRegister::new(reg.get().to_uppercase())
}

#[app::state(emits = for<'a> MigrateEvent<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = DocV1Data,
    method = upper_migrate,
    emit = MigrateEvent::Migrated { from: "1.0.0", to: "2.0.0" }
)]
pub struct DocV2Upper {
    entries: UnorderedMap<String, LwwRegister<String>>,
    #[migrate(from = title, with = uppercase)]
    heading: LwwRegister<String>,
}

#[app::logic]
impl DocV2Upper {
    #[app::init]
    pub fn init() -> DocV2Upper {
        DocV2Upper {
            entries: UnorderedMap::new(),
            heading: LwwRegister::new(String::new()),
        }
    }

    pub fn heading(&self) -> app::Result<String> {
        Ok(self.heading.get().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_sdk::testing::{assert_migrate_converges, TestHost};

    /// Builds a v1 doc with two entries and a *distinctive* title (not the init
    /// default), deterministically — used as the shared starting point for the
    /// convergence checks (both "nodes" install this).
    fn install_v1_with_entries() -> DocV1 {
        // Build the title via `LwwRegister::new` (zeroed under merge mode), not
        // `set` — this builder runs inside the convergence helper's merge-mode
        // install, so a real-timestamp `set` would diverge the two runs.
        let mut s = DocV1 {
            entries: UnorderedMap::new(),
            title: LwwRegister::new("my-doc".to_owned()),
        };
        s.entries
            .insert("b".to_owned(), "2".to_owned().into())
            .unwrap();
        s.entries
            .insert("a".to_owned(), "1".to_owned().into())
            .unwrap();
        s
    }

    #[test]
    fn migrate_reads_v1_and_seeds_tags() {
        let mut app = TestHost::new(DocV1::init);
        // A distinctive title so the carry assertion is load-bearing (a broken
        // carry that produced the init default "untitled" would be caught).
        app.call(|s| s.set_title("my-doc".to_owned())).unwrap();
        app.call(|s| s.put("b".to_owned(), "2".to_owned())).unwrap();
        app.call(|s| s.put("a".to_owned(), "1".to_owned())).unwrap();

        // `read_raw()` inside the migrate body now observes the committed v1.
        let v2 = app.migrate(migrate_v1_to_v2);

        assert_eq!(v2.view(|s| s.tag_count().unwrap()), 2);
        assert_eq!(v2.view(|s| s.title().unwrap()), "my-doc");
    }

    #[test]
    fn migration_converges_across_nodes() {
        // The deterministic migration produces a byte-identical v2 root whether
        // it runs as node [1; 32] or node [2; 32].
        assert_migrate_converges::<DocV1, DocV2>(
            install_v1_with_entries,
            migrate_v1_to_v2,
            [1u8; 32],
            [2u8; 32],
        );
    }

    /// A deliberately non-deterministic migration: it bakes the running node's
    /// identity into the title, so two nodes produce different v2 roots.
    #[app::migrate]
    pub fn migrate_v1_to_v2_divergent() -> DocV2 {
        let old_bytes = read_raw().unwrap_or_else(|| panic!("no state"));
        let old: DocV1Data = BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap();
        // BUG: per-node value baked into the migrated state.
        let who = calimero_sdk::env::executor_id()[0];
        DocV2 {
            entries: old.entries,
            title: LwwRegister::new(format!("migrated-by-{who}")),
            tags: Vector::new(),
        }
    }

    #[test]
    #[should_panic(expected = "non-deterministic")]
    fn harness_catches_divergent_migration() {
        // The convergence assertion must fail loudly for the buggy migration —
        // this is what turns a silent production network fork into a test failure.
        assert_migrate_converges::<DocV1, DocV2>(
            install_v1_with_entries,
            migrate_v1_to_v2_divergent,
            [1u8; 32],
            [2u8; 32],
        );
    }

    /// Divergence baked into a *child collection* (the carried `entries` map),
    /// not a top-level field. This is the regression guard for comparing the
    /// merkle root hash: child-entry contents live under their own storage keys,
    /// so a comparison that looked only at the root struct's bytes would MISS
    /// this and report false convergence.
    #[app::migrate]
    pub fn migrate_v1_to_v2_divergent_child() -> DocV2 {
        let old_bytes = read_raw().unwrap_or_else(|| panic!("no state"));
        let old: DocV1Data = BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap();
        let mut entries = old.entries;
        // BUG: per-node value written into a CHILD map entry, not the root.
        let who = calimero_sdk::env::executor_id()[0];
        entries
            .insert("__migrated_by".to_owned(), format!("node-{who}").into())
            .unwrap();
        DocV2 {
            entries,
            title: old.title,
            tags: Vector::new(),
        }
    }

    #[test]
    #[should_panic(expected = "non-deterministic")]
    fn harness_catches_divergence_inside_a_child_collection() {
        assert_migrate_converges::<DocV1, DocV2>(
            install_v1_with_entries,
            migrate_v1_to_v2_divergent_child,
            [1u8; 32],
            [2u8; 32],
        );
    }

    #[test]
    fn panicking_init_does_not_poison_the_harness_slot() {
        // A `TestHost::new` whose build panics (a panicking `init` is routine in
        // TDD) must release the live slot on unwind, so the next `new` on this
        // pooled thread works instead of failing with "another TestHost is still
        // alive". Regression for the claim-before-build bug.
        let panicked = std::panic::catch_unwind(|| {
            let _ = TestHost::new(|| -> DocV1 { panic!("boom in init") });
        });
        assert!(panicked.is_err(), "the build panic should propagate");

        // Must not panic with the stuck-slot message:
        let app = TestHost::new(DocV1::init);
        drop(app);
    }

    // ---- #[derive(Migrate)] ----

    #[test]
    fn derived_migrate_carries_and_seeds() {
        let mut app = TestHost::new(DocV1::init);
        // Distinctive title so the carry assertion is load-bearing.
        app.call(|s| s.set_title("my-doc".to_owned())).unwrap();
        app.call(|s| s.put("b".to_owned(), "2".to_owned())).unwrap();
        app.call(|s| s.put("a".to_owned(), "1".to_owned())).unwrap();

        // `derived_migrate` is generated by `#[derive(Migrate)]` on DocV2Derived.
        let v2 = app.migrate(derived_migrate);

        assert_eq!(v2.view(|s| s.entry_count().unwrap()), 2);
        assert_eq!(v2.view(|s| s.note().unwrap()), "seeded");
        assert_eq!(v2.view(|s| s.title().unwrap()), "my-doc");
    }

    #[test]
    fn derived_migration_converges_across_nodes() {
        // The generated migration is deterministic — carried fields come from a
        // byte-identical v1 and the seeded field is built under merge mode.
        assert_migrate_converges::<DocV1, DocV2Derived>(
            install_v1_with_entries,
            derived_migrate,
            [1u8; 32],
            [2u8; 32],
        );
    }

    #[test]
    fn derived_migrate_renames_and_drops() {
        let mut app = TestHost::new(DocV1::init);
        // Set a distinctive title so the assertion proves `old.title -> heading`
        // actually wired through (init defaults to "untitled", which would pass
        // even for a broken rename).
        app.call(|s| s.set_title("renamed-doc".to_owned())).unwrap();
        app.call(|s| s.put("x".to_owned(), "1".to_owned())).unwrap();

        // `renamed_migrate` carries v1's `title` into `heading`; v1's `entries`
        // is dropped (absent from DocV2Renamed).
        let v2 = app.migrate(renamed_migrate);
        assert_eq!(v2.view(|s| s.heading().unwrap()), "renamed-doc");
    }

    #[test]
    fn derived_with_transforms_and_emit_fires() {
        let mut app = TestHost::new(DocV1::init);
        app.call(|s| s.set_title("hello".to_owned())).unwrap();
        app.take_events(); // discard pre-migration events

        let v2 = app.migrate(upper_migrate);

        // `with = uppercase` transformed title -> heading.
        assert_eq!(v2.view(|s| s.heading().unwrap()), "HELLO");
        // `emit = MigrateEvent::Migrated { .. }` fired during the migration.
        let events = v2.events();
        assert!(
            events.iter().any(|e| e.kind.contains("Migrated")),
            "migrate should have emitted a Migrated event, got {events:?}"
        );
    }
}
