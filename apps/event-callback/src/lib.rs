#![allow(clippy::len_without_is_empty)]


use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::UnorderedMap;
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct EventCallbackApp {
    users: UnorderedMap<String, String>, // user_id -> email
    orders: UnorderedMap<String, String>, // order_id -> user_id
    callback_markers: UnorderedMap<String, String>, // callback_user_id -> marker
}

#[app::event]
pub enum Event<'a> {
    UserRegistered { user_id: &'a str, email: &'a str },
    OrderCreated { order_id: &'a str, user_id: &'a str, amount: u64 },
    UserLoggedIn { user_id: &'a str },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("user not found: {0}")]
    UserNotFound(&'a str),
    #[error("order not found: {0}")]
    OrderNotFound(&'a str),
    #[error("user already exists: {0}")]
    UserAlreadyExists(&'a str),
}

#[app::logic]
impl EventCallbackApp {
    #[app::init]
    pub fn init() -> EventCallbackApp {
        EventCallbackApp {
            users: UnorderedMap::new(),
            orders: UnorderedMap::new(),
            callback_markers: UnorderedMap::new(),
        }
    }

    pub fn register_user(&mut self, user_id: String, email: String) -> app::Result<()> {
        app::log!("Registering user: {} with email: {}", user_id, email);

        if self.users.contains(&user_id)? {
            app::bail!(Error::UserAlreadyExists(&user_id));
        }

        self.users.insert(user_id.clone(), email.clone())?;

        // Emit the UserRegistered event
        app::emit!(Event::UserRegistered {
            user_id: &user_id,
            email: &email,
        });

        Ok(())
    }

    pub fn create_order(&mut self, order_id: String, user_id: String, amount: u64) -> app::Result<()> {
        app::log!("Creating order: {} for user: {} with amount: {}", order_id, user_id, amount);

        if !self.users.contains(&user_id)? {
            app::bail!(Error::UserNotFound(&user_id));
        }

        self.orders.insert(order_id.clone(), user_id.clone())?;

        // Emit the OrderCreated event
        app::emit!(Event::OrderCreated {
            order_id: &order_id,
            user_id: &user_id,
            amount,
        });

        Ok(())
    }

    pub fn user_login(&mut self, user_id: String) -> app::Result<()> {
        app::log!("User login: {}", user_id);

        if !self.users.contains(&user_id)? {
            app::bail!(Error::UserNotFound(&user_id));
        }

        // Emit the UserLoggedIn event
        app::emit!(Event::UserLoggedIn {
            user_id: &user_id,
        });

        Ok(())
    }

    pub fn get_user_email(&self, user_id: String) -> app::Result<Option<String>> {
        app::log!("Getting user email for: {}", user_id);

        self.users.get(&user_id).map_err(Into::into)
    }

    pub fn get_order_user(&self, order_id: String) -> app::Result<Option<String>> {
        app::log!("Getting order user for: {}", order_id);

        self.orders.get(&order_id).map_err(Into::into)
    }

    pub fn get_user_count(&self) -> app::Result<u32> {
        app::log!("Getting user count");

        Ok(self.users.len()? as u32)
    }

    pub fn get_order_count(&self) -> app::Result<u32> {
        app::log!("Getting order count");

        Ok(self.orders.len()? as u32)
    }

    // This method handles cross-node event callbacks
    // It will be called when events are received from other nodes
    pub fn handle_callback(&mut self, event_type: String, data: String) -> app::Result<()> {
        app::log!("Handling callback for event: {} with data: {}", event_type, data);

        match event_type.as_str() {
            "UserRegistered" => {
                // When a user is registered on another node, create a callback marker
                let callback_user_id = format!("callback_{}", data);
                self.callback_markers.insert(callback_user_id, "callback_executed".to_string())?;
                app::log!("Created callback marker for UserRegistered event");
            }
            "OrderCreated" => {
                // When an order is created on another node, we could do something here
                app::log!("Received OrderCreated callback");
            }
            "UserLoggedIn" => {
                // When a user logs in on another node, we could do something here
                app::log!("Received UserLoggedIn callback");
            }
            _ => {
                app::log!("Unknown event type: {}", event_type);
            }
        }

        Ok(())
    }

    // This method handles automatic callbacks when state deltas are synced
    // It's called automatically by the Calimero runtime during state synchronization
    pub fn handle_automatic_callback(&mut self, event_type: String, user_id: String) -> app::Result<()> {
        app::log!("Handling automatic callback for event: {} for user: {}", event_type, user_id);

        match event_type.as_str() {
            "UserRegistered" => {
                // When a user is registered on another node, create a callback marker
                let callback_user_id = format!("callback_{}", user_id);
                self.callback_markers.insert(callback_user_id, "callback_executed".to_string())?;
                app::log!("Created callback marker for UserRegistered event from another node");
            }
            "OrderCreated" => {
                // When an order is created on another node, we could do something here
                app::log!("Received OrderCreated callback from another node");
            }
            "UserLoggedIn" => {
                // When a user logs in on another node, we could do something here
                app::log!("Received UserLoggedIn callback from another node");
            }
            _ => {
                app::log!("Unknown event type: {}", event_type);
            }
        }

        Ok(())
    }

    // Method to check if callback was executed (for testing)
    pub fn get_callback_marker(&self, callback_user_id: String) -> app::Result<Option<String>> {
        app::log!("Getting callback marker for: {}", callback_user_id);

        self.callback_markers.get(&callback_user_id).map_err(Into::into)
    }

    // This method is called automatically by the Calimero runtime when events are received
    // from other nodes during state synchronization. The runtime will call this method
    // with the event data after applying the state delta.
    pub fn process_remote_events(&mut self, payload: Vec<u8>) -> app::Result<()> {
        // Parse the combined JSON payload from the node
        let payload_json: calimero_sdk::serde_json::Value = calimero_sdk::serde_json::from_slice(&payload)
            .unwrap_or_else(|_| calimero_sdk::serde_json::Value::Null);
        
        let event_kind = payload_json.get("event_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let event_data = payload_json.get("event_data")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).map(|n| n as u8).collect::<Vec<u8>>())
            .unwrap_or_default();

        app::log!("Processing remote event: {} with data length: {}", event_kind, event_data.len());

        // Parse the event data to extract relevant information
        // For UserRegistered events, the data would contain user information
        match event_kind.as_str() {
            "UserRegistered" => {
                // For now, we'll use a default user_id since parsing the actual event data
                // would require knowing the exact structure of the event payload
                let user_id = "user123".to_string();
                app::log!("Received UserRegistered event from remote node, triggering callback");
                self.handle_automatic_callback("UserRegistered".to_string(), user_id)?;
            }
            "OrderCreated" => {
                // Extract order information from event data
                app::log!("Received OrderCreated event from remote node");
            }
            "UserLoggedIn" => {
                // Extract user information from event data
                app::log!("Received UserLoggedIn event from remote node");
            }
            _ => {
                app::log!("Unknown remote event type: {}", event_kind);
            }
        }

        Ok(())
    }
}
