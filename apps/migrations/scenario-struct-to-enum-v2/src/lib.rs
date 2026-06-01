use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::LwwRegister;

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

// v2 promotes `Status` from a struct (where `active = true` could
// illegally coexist with `reason = Some(_)`) to a tagged enum that
// makes the impossible state unrepresentable.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub enum Status {
    Active,
    Inactive(String),
}

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug)]
pub struct ScenarioStructToEnumV2 {
    name: LwwRegister<String>,
    status: LwwRegister<Status>,
}

#[app::event]
pub enum Event<'a> {
    Migrated {
        from_version: &'a str,
        to_version: &'a str,
    },
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub name: String,
    pub status_kind: String,
    pub status_reason: Option<String>,
}

// v1 layout, used solely to decode the legacy state during migration.
#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct LegacyStatus {
    active: bool,
    reason: Option<String>,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioStructToEnumV1 {
    name: LwwRegister<String>,
    status: LwwRegister<LegacyStatus>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioStructToEnumV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioStructToEnumV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    let legacy_status = old_state.status.get();
    let new_status = if legacy_status.active {
        // `active = true` collapses to `Active`, dropping any reason
        // payload that should never have been set in the first place.
        Status::Active
    } else {
        Status::Inactive(legacy_status.reason.clone().unwrap_or_default())
    };

    ScenarioStructToEnumV2 {
        name: old_state.name,
        status: LwwRegister::new(new_status),
    }
}

#[app::logic]
impl ScenarioStructToEnumV2 {
    #[app::init]
    pub fn init() -> ScenarioStructToEnumV2 {
        ScenarioStructToEnumV2 {
            name: LwwRegister::new("entity".to_owned()),
            status: LwwRegister::new(Status::Active),
        }
    }

    pub fn set_name(&mut self, name: String) -> app::Result<()> {
        self.name.set(name);
        Ok(())
    }

    pub fn set_status_active(&mut self) -> app::Result<()> {
        self.status.set(Status::Active);
        Ok(())
    }

    pub fn set_status_inactive(&mut self, reason: String) -> app::Result<()> {
        self.status.set(Status::Inactive(reason));
        Ok(())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        let (status_kind, status_reason) = match self.status.get().clone() {
            Status::Active => ("active".to_owned(), None),
            Status::Inactive(reason) => ("inactive".to_owned(), Some(reason)),
        };
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            name: self.name.get().clone(),
            status_kind,
            status_reason,
        })
    }
}
