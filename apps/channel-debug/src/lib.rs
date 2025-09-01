#![allow(clippy::len_without_is_empty)]

use std::collections::HashMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{UnorderedMap, Vector};
use thiserror::Error;

// Type aliases for clarity
pub type UserId = String;
pub type MessageId = String;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, PartialEq, Eq, Hash)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Channel {
    pub name: String,
}

impl AsRef<[u8]> for Channel {
    fn as_ref(&self) -> &[u8] {
        self.name.as_bytes()
    }
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Message {
    pub id: MessageId,
    pub content: String,
    pub sender: UserId,
    pub timestamp: u64,
}

#[derive(
    Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, calimero_sdk::serde::Deserialize,
)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub enum ChannelType {
    Public,
    Private,
    Default,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct ChannelMetadata {
    pub description: String,
    pub created_at: u64,
    pub created_by: UserId,
    pub links_allowed: bool,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct PublicChannelInfo {
    pub channel_type: ChannelType,
    pub read_only: bool,
    pub created_at: u64,
    pub created_by: UserId,
    pub created_by_username: String,
    pub links_allowed: bool,
    pub unread_count: u32,
    pub last_read_timestamp: u64,
    pub unread_mention_count: u32,
}

#[derive(Debug, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct ChannelInfo {
    pub messages: Vector<Message>,
    pub channel_type: ChannelType,
    pub read_only: bool,
    pub meta: ChannelMetadata,
    pub last_read: UnorderedMap<UserId, MessageId>,
}

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ChannelDebug {
    channels: UnorderedMap<Channel, ChannelInfo>,
    channel_members: UnorderedMap<Channel, Vector<UserId>>,
    member_usernames: UnorderedMap<UserId, String>,
}

#[app::event]
pub enum Event<'a> {
    ChannelAdded { channel: &'a Channel },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("channel already exists: {0}")]
    ChannelAlreadyExists(&'a str),
}

#[app::logic]
impl ChannelDebug {
    #[app::init]
    pub fn init() -> ChannelDebug {
        ChannelDebug {
            channels: UnorderedMap::new(),
            channel_members: UnorderedMap::new(),
            member_usernames: UnorderedMap::new(),
        }
    }

    /// Add a new channel to the debug app
    pub fn add_channel(
        &mut self,
        name: String,
        channel_type: ChannelType,
        description: String,
        created_by: UserId,
    ) -> app::Result<()> {
        app::log!("Adding channel: {:?}", name);

        let channel = Channel { name: name.clone() };

        // Create proper ChannelInfo
        let channel_info = ChannelInfo {
            messages: Vector::new(),
            channel_type,
            read_only: false,
            meta: ChannelMetadata {
                description,
                created_at: 0,
                created_by: created_by.clone(),
                links_allowed: true,
            },
            last_read: UnorderedMap::new(),
        };

        // Insert channel
        self.channels.insert(channel.clone(), channel_info)?;

        // Add member to channel
        let mut members = Vector::new();
        members.push(created_by.clone())?;
        self.channel_members.insert(channel.clone(), members)?;

        // Add username mapping
        self.member_usernames.insert(created_by, name.clone())?;

        app::log!("Channel inserted successfully");
        app::emit!(Event::ChannelAdded { channel: &channel });

        Ok(())
    }

    /// Add a new channel using string-based channel type (for workflow compatibility)
    pub fn add_channel_string(
        &mut self,
        name: String,
        channel_type_str: String,
        description: String,
        created_by: UserId,
    ) -> app::Result<String> {
        app::log!(
            "Adding channel with string type: {:?}, type: {:?}",
            name,
            channel_type_str
        );

        let channel_type = match channel_type_str.as_str() {
            "Public" => ChannelType::Public,
            "Private" => ChannelType::Private,
            "Default" => ChannelType::Default,
            _ => {
                app::log!(
                    "Invalid channel type: {}, defaulting to Public",
                    channel_type_str
                );
                ChannelType::Public
            }
        };

        let name_clone = name.clone();
        self.add_channel(name, channel_type, description, created_by)?;
        Ok(format!("Channel {} added successfully", name_clone))
    }

    /// Simple test method that just returns a string
    pub fn test_basic(&self) -> app::Result<String> {
        Ok("Basic test method works!".to_string())
    }

    /// Even simpler test method
    pub fn hello(&self) -> app::Result<String> {
        Ok("Hello World!".to_string())
    }

    /// Test method with same parameters as add_channel_string but no complex logic
    pub fn test_params(
        &self,
        name: String,
        channel_type_str: String,
        description: String,
        created_by: UserId,
    ) -> app::Result<String> {
        Ok(format!(
            "Received: name={}, type={}, desc={}, by={}",
            name, channel_type_str, description, created_by
        ))
    }

    /// Test method that does exactly what add_channel does but step by step
    pub fn test_add_channel_step_by_step(
        &mut self,
        name: String,
        channel_type: ChannelType,
        description: String,
        created_by: UserId,
    ) -> app::Result<String> {
        app::log!("🔍 Testing add_channel step by step");
        app::log!(
            "🔍 Parameters: name={:?}, type={:?}, desc={:?}, by={:?}",
            name,
            channel_type,
            description,
            created_by
        );

        // Step 1: Create channel
        let channel = Channel { name: name.clone() };
        app::log!("🔍 Step 1: Channel created: {:?}", channel);

        // Step 2: Create channel_info
        let channel_info = ChannelInfo {
            messages: Vector::new(),
            channel_type,
            read_only: false,
            meta: ChannelMetadata {
                description,
                created_at: 0,
                created_by: created_by.clone(),
                links_allowed: true,
            },
            last_read: UnorderedMap::new(),
        };
        app::log!("🔍 Step 2: ChannelInfo created: {:?}", channel_info);

        // Step 3: Insert channel
        app::log!("🔍 Step 3: About to insert channel into self.channels");
        self.channels.insert(channel.clone(), channel_info)?;
        app::log!("🔍 Step 3: Channel inserted successfully");

        // Step 4: Add member to channel
        let mut members = Vector::new();
        app::log!("🔍 Step 4a: Vector created");
        app::log!("🔍 Step 4b: About to push member: {:?}", created_by);
        members.push(created_by.clone())?;
        app::log!("🔍 Step 4b: Member pushed to vector successfully");
        app::log!("🔍 Step 4c: About to insert members into channel_members");
        self.channel_members.insert(channel.clone(), members)?;
        app::log!("🔍 Step 4c: Members inserted successfully");

        // Step 5: Add username mapping
        app::log!(
            "🔍 Step 5: About to insert username mapping: {:?} -> {:?}",
            created_by,
            name
        );
        self.member_usernames.insert(created_by, name.clone())?;
        app::log!("🔍 Step 5: Username mapping inserted successfully");

        app::log!("✅ All steps completed successfully");
        Ok(format!("Channel {} added step by step successfully", name))
    }

    /// Test method that does exactly what add_channel does but step by step (string-based)
    pub fn test_add_channel_step_by_step_string(
        &mut self,
        name: String,
        channel_type_str: String,
        description: String,
        created_by: UserId,
    ) -> app::Result<String> {
        app::log!("🔍 Testing add_channel step by step (string-based)");
        app::log!(
            "🔍 Parameters: name={:?}, type_str={:?}, desc={:?}, by={:?}",
            name,
            channel_type_str,
            description,
            created_by
        );

        // Convert string to enum
        let channel_type = match channel_type_str.as_str() {
            "Public" => ChannelType::Public,
            "Private" => ChannelType::Private,
            "Default" => ChannelType::Default,
            _ => {
                app::log!(
                    "❌ Invalid channel type: {}, defaulting to Public",
                    channel_type_str
                );
                ChannelType::Public
            }
        };

        // Step 1: Create channel
        let channel = Channel { name: name.clone() };
        app::log!("🔍 Step 1: Channel created: {:?}", channel);

        // Step 2: Create channel_info
        let channel_info = ChannelInfo {
            messages: Vector::new(),
            channel_type,
            read_only: false,
            meta: ChannelMetadata {
                description,
                created_at: 0,
                created_by: created_by.clone(),
                links_allowed: true,
            },
            last_read: UnorderedMap::new(),
        };
        app::log!("🔍 Step 2: ChannelInfo created: {:?}", channel_info);

        // Step 3: Insert channel
        app::log!("🔍 Step 3: About to insert channel into self.channels");
        self.channels.insert(channel.clone(), channel_info)?;
        app::log!("🔍 Step 3: Channel inserted successfully");

        // Step 4: Add member to channel
        let mut members = Vector::new();
        app::log!("🔍 Step 4a: Vector created");
        app::log!("🔍 Step 4b: About to push member: {:?}", created_by);
        members.push(created_by.clone())?;
        app::log!("🔍 Step 4b: Member pushed to vector successfully");
        app::log!("🔍 Step 4c: About to insert members into channel_members");
        self.channel_members.insert(channel.clone(), members)?;
        app::log!("🔍 Step 4c: Members inserted successfully");

        // Step 5: Add username mapping
        app::log!(
            "🔍 Step 5: About to insert username mapping: {:?} -> {:?}",
            created_by,
            name
        );
        self.member_usernames.insert(created_by, name.clone())?;
        app::log!("🔍 Step 5: Username mapping inserted successfully");

        app::log!("✅ All steps completed successfully");
        Ok(format!("Channel {} added step by step successfully", name))
    }

    /// Get all channels
    pub fn get_channels(&self) -> app::Result<Vec<(Channel, ChannelInfo)>> {
        app::log!("Getting all channels");

        Ok(self.channels.entries()?.collect())
    }

    /// Simple test method to check if basic operations work
    pub fn test_simple(&mut self, name: String) -> app::Result<()> {
        app::log!("Testing simple operation with name: {:?}", name);

        let channel = Channel { name: name.clone() };
        let simple_info = ChannelInfo {
            messages: Vector::new(),
            channel_type: ChannelType::Public,
            read_only: false,
            meta: ChannelMetadata {
                description: "test".to_string(),
                created_at: 0,
                created_by: "test".to_string(),
                links_allowed: true,
            },
            last_read: UnorderedMap::new(),
        };

        self.channels.insert(channel, simple_info)?;
        app::log!("Simple test completed successfully");

        Ok(())
    }

    /// Test raw storage operations to see if runtime is working
    pub fn test_raw_storage(&mut self) -> app::Result<String> {
        app::log!("🔍 Testing raw storage operations");

        // Test 1: Try to read a non-existent key
        app::log!("🔍 Test 1: Reading non-existent key");
        let test_key = b"test_key_123";
        let result = calimero_sdk::env::storage_read(test_key);
        app::log!("🔍 Storage read result: {:?}", result);

        // Test 2: Try to write a simple value
        app::log!("🔍 Test 2: Writing simple value");
        let test_value = b"test_value_456";
        let write_result = calimero_sdk::env::storage_write(test_key, test_value);
        app::log!("🔍 Storage write result: {}", write_result);

        // Test 3: Try to read it back
        app::log!("🔍 Test 3: Reading back the value");
        let read_back = calimero_sdk::env::storage_read(test_key);
        app::log!("🔍 Storage read back result: {:?}", read_back);

        app::log!("✅ Raw storage test completed");
        Ok("Raw storage test completed successfully".to_string())
    }

    /// Get channels in the format that matches the original curb implementation
    pub fn get_channels_curb(&self) -> HashMap<String, PublicChannelInfo> {
        let mut channels = HashMap::new();
        let executor_id = self.get_executor_id();

        if let Ok(entries) = self.channels.entries() {
            for (channel, channel_info) in entries {
                if let Ok(Some(members)) = self.channel_members.get(&channel) {
                    if members.contains(&executor_id).unwrap_or(false) {
                        // Get unread information for this user and channel
                        let (unread_count, last_read_timestamp) =
                            self.get_user_channel_unread_info(&executor_id, &channel);
                        let unread_mention_count =
                            self.get_user_channel_mention_count(&executor_id, &channel);

                        let created_by = channel_info.meta.created_by.clone();
                        let public_info = PublicChannelInfo {
                            channel_type: channel_info.channel_type,
                            read_only: channel_info.read_only,
                            created_at: channel_info.meta.created_at,
                            created_by: created_by.clone(),
                            created_by_username: self
                                .member_usernames
                                .get(&created_by)
                                .unwrap()
                                .unwrap(),
                            links_allowed: channel_info.meta.links_allowed,
                            unread_count,
                            last_read_timestamp,
                            unread_mention_count,
                        };
                        channels.insert(channel.name, public_info);
                    }
                }
            }
        }
        channels
    }

    /// Get the executor ID (simplified for testing)
    fn get_executor_id(&self) -> UserId {
        "test_executor".to_string()
    }

    /// Get unread information for a user and channel (simplified for testing)
    fn get_user_channel_unread_info(
        &self,
        _executor_id: &UserId,
        _channel: &Channel,
    ) -> (u32, u64) {
        (0, 0) // Simplified for testing
    }

    /// Get unread mention count for a user and channel (simplified for testing)
    fn get_user_channel_mention_count(&self, _executor_id: &UserId, _channel: &Channel) -> u32 {
        0 // Simplified for testing
    }
}
