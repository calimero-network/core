use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env;
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

    /// Creates (spawns) a new child context with the specified application.
    ///
    /// # Arguments
    /// * `protocol` - The protocol ID (e.g., "near").
    /// * `app_id_base58` - The Application ID to install in the new context (Base58 string).
    /// * `alias` - alias string for the new context.
    pub fn create_context_child(
        &self,
        protocol: String,
        app_id_base58: String,
        alias: String,
    ) -> app::Result<()> {
        app::log!(
            "Creating child context for app: {} with alias: {}",
            app_id_base58,
            alias
        );

        // Check if alias already exists to fail fast
        if env::context_resolve_alias(&alias).is_some() {
            app::bail!("Alias '{}' already exists", alias);
        }

        // Decode App ID from Base58
        let app_id_bytes: [u8; 32] = bs58::decode(&app_id_base58)
            .into_vec()
            .map_err(|e| app::err!(format!("Failed to app ID: {e}")))?
            .try_into()
            .map_err(|_| app::err!("App ID must be exactly 32 bytes"))?;

        // Default initialization arguments (empty JSON object)
        let init_args = b"{}".to_vec();

        // No seed passed (random generation by host)
        env::context_create(&protocol, &app_id_bytes, &init_args, Some(&alias));

        Ok(())
    }

    /// Helper to get the ID of a child context by its alias.
    pub fn get_child_id(&self, alias: String) -> app::Result<Option<String>> {
        let id = env::context_resolve_alias(&alias);
        Ok(id.map(|bytes| bs58::encode(bytes).into_string()))
    }

    /// Deletes a context.
    ///
    /// # Arguments
    /// * `context_id_base58` - The ID of the context to delete (Base58).
    ///                         If empty, deletes the current context (self-destruct).
    pub fn delete_context_child(&self, context_id_base58: String) -> app::Result<()> {
        if context_id_base58.is_empty() {
            app::log!("Deleting current context (self-destruct)");
            env::context_delete(&env::context_id());
            return Ok(());
        }

        app::log!("Deleting context: {}", context_id_base58);
        let context_id_to_delete: [u8; 32] = bs58::decode(&context_id_base58)
            .into_vec()
            .map_err(|e| app::err!(format!("Failed to decode context ID: {e}")))?
            .try_into()
            .map_err(|_| app::err!("Context ID must be exactly 32 bytes"))?;

        env::context_delete(&context_id_to_delete);

        Ok(())
    }
}
