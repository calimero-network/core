use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{AuthoredMap, LwwRegister, UnorderedMap};

/// v2 DOWNGRADES `wiki` from an `AuthoredMap` (identity-gated, per-entry author)
/// to a plain `UnorderedMap`, dropping authorship network-wide. This is
/// intentionally UNSAFE — it exists only as the fixture the `calimero-abi diff`
/// CI guard must catch (`UNSAFE_IDENTITY_DOWNGRADE`). It is NOT run by merobox.
#[app::state]
pub struct ScenarioIdentityDowngradeV2 {
    wiki: UnorderedMap<String, LwwRegister<String>>,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioIdentityDowngradeV1 {
    wiki: AuthoredMap<String, LwwRegister<String>>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioIdentityDowngradeV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });
    let _old: ScenarioIdentityDowngradeV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| panic!("Migration failed: V1 deserialization error {:?}", e));

    // A real downgrade would copy entries into the plain map, discarding the
    // per-entry authorship — which is exactly why this is unsafe and why the
    // schema diff must block it. The body is irrelevant to the schema guard.
    ScenarioIdentityDowngradeV2 {
        wiki: UnorderedMap::new(),
    }
}

#[app::logic]
impl ScenarioIdentityDowngradeV2 {
    #[app::init]
    pub fn init() -> ScenarioIdentityDowngradeV2 {
        ScenarioIdentityDowngradeV2 {
            wiki: UnorderedMap::new(),
        }
    }

    pub fn put(&mut self, key: String, value: String) -> app::Result<()> {
        self.wiki.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.wiki.get(&key)?.map(|v| v.get().clone()))
    }
}
