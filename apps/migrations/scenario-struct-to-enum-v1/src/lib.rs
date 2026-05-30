use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::LwwRegister;

const SCHEMA_VERSION_V1: &str = "1.0.0";

// Convention: `active = true` implies `reason = None`; `active = false`
// implies `reason = Some(_)`. The impossible state — `active = true`
// combined with `reason = Some(_)` — is structurally allowed but
// domain-forbidden. v2 promotes `Status` to a tagged enum so this
// impossible state cannot be represented.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Status {
    pub active: bool,
    pub reason: Option<String>,
}

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioStructToEnumV1 {
    name: LwwRegister<String>,
    status: LwwRegister<Status>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub name: String,
    pub status_active: bool,
    pub status_reason: Option<String>,
}

#[app::logic]
impl ScenarioStructToEnumV1 {
    #[app::init]
    pub fn init() -> ScenarioStructToEnumV1 {
        ScenarioStructToEnumV1 {
            name: LwwRegister::new("entity".to_owned()),
            status: LwwRegister::new(Status {
                active: true,
                reason: None,
            }),
        }
    }

    pub fn set_name(&mut self, name: String) -> app::Result<()> {
        self.name.set(name);
        Ok(())
    }

    pub fn set_status_active(&mut self) -> app::Result<()> {
        self.status.set(Status {
            active: true,
            reason: None,
        });
        Ok(())
    }

    pub fn set_status_inactive(&mut self, reason: String) -> app::Result<()> {
        self.status.set(Status {
            active: false,
            reason: Some(reason),
        });
        Ok(())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        let status = self.status.get().clone();
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            name: self.name.get().clone(),
            status_active: status.active,
            status_reason: status.reason,
        })
    }
}
