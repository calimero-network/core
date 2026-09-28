use calimero_sdk::abi::AbiType;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, AccountId};
use calimero_storage::collections::{AuthoredMap, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `AuthoredMap` through the migrate (preserving each note's
/// owner stamp byte-for-byte) and adds a plain `migration_note`. `version = 2`
/// raises the identity-gated target: each carried note is still stamped at 1,
/// so its owner's next signed write — or one tap of the generated
/// `migrate_my_entries()` — re-stamps it to 2. The generated export is emitted
/// automatically because the state has an `AuthoredMap` field.
#[app::state(version = 2, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = ScenarioAuthoredMigrateUxV1,
    emit = Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    }
)]
pub struct ScenarioAuthoredMigrateUxV2 {
    notes: AuthoredMap<String, LwwRegister<String>>,
    #[migrate(new = LwwRegister::new("migrated-v1-to-v2".to_owned()))]
    migration_note: LwwRegister<String>,
}

#[app::event]
pub enum Event<'a> {
    Migrated {
        from_version: &'a str,
        to_version: &'a str,
    },
}

#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub note_count: u64,
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioAuthoredMigrateUxV1 {
    notes: AuthoredMap<String, LwwRegister<String>>,
}

#[app::logic]
impl ScenarioAuthoredMigrateUxV2 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredMigrateUxV2 {
        ScenarioAuthoredMigrateUxV2 {
            notes: AuthoredMap::new(),
            migration_note: LwwRegister::new(String::new()),
        }
    }

    pub fn set_note(&mut self, key: String, text: String) -> app::Result<()> {
        if self.notes.contains(&key)? {
            self.notes.update(&key, text.into())?;
        } else {
            self.notes.insert(key, text.into())?;
        }
        Ok(())
    }

    /// The caller's own note at `key`. Keys are per owner, so another
    /// account's note at the same key is not this one.
    pub fn my_note(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.notes.get(&key)?.map(|v| v.get().clone()))
    }

    /// The lowest account holding a note at `key`.
    pub fn owner_of(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.holder(&key)?.map(|(owner, _)| owner.to_string()))
    }

    pub fn note_count(&self) -> app::Result<u64> {
        Ok(self.notes.len()? as u64)
    }

    /// The note at `key`, whoever wrote it: the lowest account's, if several
    /// hold the key. Lets the e2e read another node's note by key alone.
    pub fn note(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.holder(&key)?.map(|(_, v)| v.get().clone()))
    }

    /// The stored `schema_version` of the note at `key` (the lowest holder's,
    /// as `note` reads it) — `Some(1)` before convert, `Some(2)` after. Lets
    /// the e2e assert the one-tap convert actually re-stamped it, from any
    /// node.
    pub fn note_schema_version(&self, key: String) -> app::Result<Option<u32>> {
        let Some((owner, _)) = self.holder(&key)? else {
            return Ok(None);
        };
        Ok(self.notes.entry_schema_version_by(&owner, &key)?)
    }

    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            note_count: self.notes.len()? as u64,
            migration_note: self.migration_note.get().clone(),
        })
    }
}

impl ScenarioAuthoredMigrateUxV2 {
    /// The lowest account holding `key`, with its entry: the same pick on
    /// every node. Keys are per owner, so a key-only `get` or `owner_of` would
    /// read the caller's own entry, and the scenario reads another node's.
    fn holder(&self, key: &String) -> app::Result<Option<(AccountId, LwwRegister<String>)>> {
        Ok(self
            .notes
            .entries_at(key)?
            .into_iter()
            .min_by_key(|(owner, _)| *owner))
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    /// The scenario reads node 1's note and its schema version from node 2.
    #[test]
    fn another_account_reads_a_note_and_its_schema_version() {
        let mut app = TestHost::new(ScenarioAuthoredMigrateUxV2::init);
        app.call_as_account(ALICE, ALICE, |s| s.set_note("n1-a".into(), "hers".into()))
            .expect("alice writes");

        app.set_account(BOB);
        assert_eq!(app.view(|s| s.my_note("n1-a".into())).expect("mine"), None);
        assert_eq!(
            app.view(|s| s.note("n1-a".into())).expect("note"),
            Some("hers".to_owned())
        );
        assert_eq!(
            app.view(|s| s.owner_of("n1-a".into())).expect("owner"),
            Some(AccountId::from(ALICE).to_string())
        );
        // Bob holds no note there, so a caller-scoped read finds no version.
        assert_eq!(
            app.view(|s| s.notes.entry_schema_version(&"n1-a".to_owned()))
                .expect("bob's own"),
            None
        );
        assert!(app
            .view(|s| s.note_schema_version("n1-a".into()))
            .expect("version")
            .is_some());
    }
}
