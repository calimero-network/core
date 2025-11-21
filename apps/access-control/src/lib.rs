use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env;
use calimero_sdk::serde::Serialize;
use calimero_sdk::PublicKey;

#[app::state]
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AccessControl {}

#[app::logic]
impl AccessControl {
    #[app::init]
    pub fn init() -> AccessControl {
        app::log!("Initializing AccessControl app");
        AccessControl {}
    }

    pub fn add_member(&self, public_key: PublicKey) -> app::Result<()> {
        app::log!("Adding member: {:?}", public_key);
        env::context_add_member(&*public_key);
        Ok(())
    }

    pub fn kick_member(&self, public_key: PublicKey) -> app::Result<()> {
        app::log!("Kicking member: {:?}", public_key);
        env::context_remove_member(&*public_key);
        Ok(())
    }

    pub fn is_member(&self, public_key: PublicKey) -> app::Result<bool> {
        app::log!("Checking membership for: {:?}", public_key);
        Ok(env::context_is_member(&*public_key))
    }

    pub fn get_all_members(&self) -> app::Result<Vec<PublicKey>> {
        app::log!("Listing all members");
        let members = env::context_members();
        // Convert raw [u8; 32] to PublicKey type for consistent serialization
        Ok(members.into_iter().map(PublicKey::from).collect())
    }
}
