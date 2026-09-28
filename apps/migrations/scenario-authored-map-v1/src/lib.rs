use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, AccountId};
use calimero_storage::collections::{AuthoredMap, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state: an `AuthoredMap` (each entry remembers the executor that wrote
/// it) plus a plain `LwwRegister` title. The migrate scenario carries the
/// `AuthoredMap` through to v2 unchanged — the cross-node assertion is that
/// each entry's recorded owner survives the migration identically on every
/// node (authorship is part of the stored value, so it must round-trip).
#[app::state]
pub struct ScenarioAuthoredMapV1 {
    entries: AuthoredMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub entry_count: u64,
}

#[app::logic]
impl ScenarioAuthoredMapV1 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredMapV1 {
        ScenarioAuthoredMapV1 {
            entries: AuthoredMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Insert a new entry, stamping the current executor as its owner.
    pub fn put_entry(&mut self, key: String, value: String) -> app::Result<()> {
        self.entries.insert(key, value.into())?;
        Ok(())
    }

    /// The value at `key`, whoever wrote it: the lowest account's, if several
    /// hold the key.
    pub fn get_entry(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.holder(&key)?.map(|(_, v)| v.get().clone()))
    }

    /// Hex/display string of the recorded owner of `key`, if present.
    pub fn owner_of(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.holder(&key)?.map(|(owner, _)| owner.to_string()))
    }

    pub fn entry_count(&self) -> app::Result<u64> {
        Ok(self.entries.len()? as u64)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            title: self.title.get().clone(),
            entry_count: self.entries.len()? as u64,
        })
    }
}

impl ScenarioAuthoredMapV1 {
    /// The lowest account holding `key`, with its entry: the same pick on
    /// every node. Keys are per owner, so a key-only `get` or `owner_of` would
    /// read the caller's own entry, and the scenario reads another node's.
    fn holder(&self, key: &String) -> app::Result<Option<(AccountId, LwwRegister<String>)>> {
        Ok(self
            .entries
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

    /// The scenario reads node 1's entry from node 2 by key alone.
    #[test]
    fn another_account_reads_an_entry_and_its_owner_by_key() {
        let mut app = TestHost::new(ScenarioAuthoredMapV1::init);
        app.call_as_account(BOB, BOB, |s| s.put_entry("k1".into(), "bob".into()))
            .expect("bob puts");
        app.set_account(ALICE);
        assert_eq!(
            app.view(|s| s.get_entry("k1".into())).expect("get"),
            Some("bob".to_owned())
        );
        assert_eq!(
            app.view(|s| s.owner_of("k1".into())).expect("owner"),
            Some(AccountId::from(BOB).to_string())
        );

        // Alice's own entry at the key is a second entry, and the lower
        // account's is the one a key-only read takes.
        app.call_as_account(ALICE, ALICE, |s| s.put_entry("k1".into(), "alice".into()))
            .expect("alice puts her own");
        app.set_account(BOB);
        assert_eq!(
            app.view(|s| s.get_entry("k1".into())).expect("get"),
            Some("alice".to_owned())
        );
        assert_eq!(app.view(|s| s.entry_count()).expect("count"), 2);
    }
}
