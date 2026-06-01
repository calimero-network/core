use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::LwwRegister;

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

#[app::state(emits = for<'a> Event<'a>)]
pub struct ScenarioFieldSplitV2 {
    name: LwwRegister<String>,
    street: LwwRegister<String>,
    city: LwwRegister<String>,
    postcode: LwwRegister<String>,
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
    pub street: String,
    pub city: String,
    pub postcode: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioFieldSplitV1 {
    name: LwwRegister<String>,
    address: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioFieldSplitV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioFieldSplitV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    let raw_address = old_state.address.get().clone();
    let parts: Vec<&str> = raw_address.split(", ").collect();
    let (street, city, postcode) = if parts.len() == 3 {
        (
            parts[0].to_owned(),
            parts[1].to_owned(),
            parts[2].to_owned(),
        )
    } else {
        (raw_address, String::new(), String::new())
    };

    ScenarioFieldSplitV2 {
        name: old_state.name,
        street: LwwRegister::new(street),
        city: LwwRegister::new(city),
        postcode: LwwRegister::new(postcode),
    }
}

#[app::logic]
impl ScenarioFieldSplitV2 {
    #[app::init]
    pub fn init() -> ScenarioFieldSplitV2 {
        ScenarioFieldSplitV2 {
            name: LwwRegister::new(String::new()),
            street: LwwRegister::new(String::new()),
            city: LwwRegister::new(String::new()),
            postcode: LwwRegister::new(String::new()),
        }
    }

    pub fn set_name(&mut self, name: String) -> app::Result<()> {
        self.name.set(name);
        Ok(())
    }

    pub fn set_street(&mut self, street: String) -> app::Result<()> {
        self.street.set(street);
        Ok(())
    }

    pub fn set_city(&mut self, city: String) -> app::Result<()> {
        self.city.set(city);
        Ok(())
    }

    pub fn set_postcode(&mut self, postcode: String) -> app::Result<()> {
        self.postcode.set(postcode);
        Ok(())
    }

    pub fn get_street(&self) -> app::Result<String> {
        Ok(self.street.get().clone())
    }

    pub fn get_city(&self) -> app::Result<String> {
        Ok(self.city.get().clone())
    }

    pub fn get_postcode(&self) -> app::Result<String> {
        Ok(self.postcode.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            name: self.name.get().clone(),
            street: self.street.get().clone(),
            city: self.city.get().clone(),
            postcode: self.postcode.get().clone(),
        })
    }
}
