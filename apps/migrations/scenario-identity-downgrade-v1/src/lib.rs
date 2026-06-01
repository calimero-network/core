use calimero_sdk::app;
use calimero_storage::collections::{AuthoredMap, LwwRegister};

/// v1 baseline for the identity-downgrade guard. `wiki` is an `AuthoredMap` —
/// identity-gated, every entry records the executor that wrote it. v2 changes
/// this field to a plain `UnorderedMap`, dropping authorship; `calimero-abi diff`
/// must flag that as `UNSAFE_IDENTITY_DOWNGRADE`.
///
/// This pair is NOT run by merobox (a downgrade migration is intentionally unsafe
/// at runtime). It exists only so the schema-downgrade CI guard has a real,
/// emitter-produced `state-schema.json` pair to diff.
#[app::state]
pub struct ScenarioIdentityDowngradeV1 {
    wiki: AuthoredMap<String, LwwRegister<String>>,
}

#[app::logic]
impl ScenarioIdentityDowngradeV1 {
    #[app::init]
    pub fn init() -> ScenarioIdentityDowngradeV1 {
        ScenarioIdentityDowngradeV1 {
            wiki: AuthoredMap::new(),
        }
    }

    pub fn put(&mut self, key: String, value: String) -> app::Result<()> {
        self.wiki.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.wiki.get(&key)?.map(|v| v.get().clone()))
    }

    pub fn owner_of(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.wiki.owner_of(&key)?.map(|pk| pk.to_string()))
    }
}
