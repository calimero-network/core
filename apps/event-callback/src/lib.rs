#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::CallbackHandlers;
use calimero_storage::collections::UnorderedMap;
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct EventCallbackApp {
    users: UnorderedMap<String, String>,  // user_id -> email
    orders: UnorderedMap<String, String>, // order_id -> user_id
    callback_markers: UnorderedMap<String, String>, // callback_user_id -> marker
}

#[app::event]
#[derive(Debug, calimero_sdk::serde::Deserialize, CallbackHandlers)]
pub enum Event<'a> {
    UserRegistered {
        user_id: &'a str,
        email: &'a str,
    },
    OrderCreated {
        order_id: &'a str,
        user_id: &'a str,
        amount: u64,
    },
    UserLoggedIn {
        user_id: &'a str,
    },
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
        if self.users.contains(&user_id)? {
            app::bail!(Error::UserAlreadyExists(&user_id));
        }

        self.users.insert(user_id.clone(), email.clone())?;

        // Emit the UserRegistered event
        // This event will be captured by the execution system and broadcast to other nodes
        // via state delta synchronization. Other nodes will then process this event
        // through their process_remote_events method to execute callbacks.
        app::emit!(Event::UserRegistered {
            user_id: &user_id,
            email: &email,
        });

        Ok(())
    }

    pub fn create_order(
        &mut self,
        order_id: String,
        user_id: String,
        amount: u64,
    ) -> app::Result<()> {
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
        if !self.users.contains(&user_id)? {
            app::bail!(Error::UserNotFound(&user_id));
        }

        // Emit the UserLoggedIn event
        app::emit!(Event::UserLoggedIn { user_id: &user_id });

        Ok(())
    }

    pub fn get_user_email(&self, user_id: String) -> app::Result<Option<String>> {
        self.users.get(&user_id).map_err(Into::into)
    }

    pub fn get_order_user(&self, order_id: String) -> app::Result<Option<String>> {
        self.orders.get(&order_id).map_err(Into::into)
    }

    pub fn get_user_count(&self) -> app::Result<u32> {
        Ok(self.users.len()? as u32)
    }

    pub fn get_order_count(&self) -> app::Result<u32> {
        Ok(self.orders.len()? as u32)
    }

    // Warm-up method: perform a benign state mutation without emitting events.
    // Rationale:
    // - The first cross-node delta after context setup can cause a sync fallback
    //   on peers (e.g., missing sender key). During such fallbacks, bundled events
    //   are intentionally skipped to avoid double processing.
    // - Applying a benign mutation first seeds an initial artifact/delta and height,
    //   reducing the chance that the next event-emitting call is skipped on peers.
    // - Any first method call could serve this purpose, but then the initial callback
    //   might be skipped implicitly. Providing an explicit `warmup` makes this pattern
    //   clear and intentional to users reading the workflow or app.
    pub fn warmup(&mut self) -> app::Result<()> {
        // Write a dummy marker to guarantee a non-empty artifact with no events
        let _ = self
            .callback_markers
            .insert("_warmup".to_string(), "1".to_string())?;
        Ok(())
    }

    // Method to check if callback was executed (for testing)
    pub fn get_callback_marker(&self, callback_user_id: String) -> app::Result<Option<String>> {
        self.callback_markers
            .get(&callback_user_id)
            .map_err(Into::into)
    }
}

// Implement generated per-variant handlers for the app. Only override what you need.
impl CallbackHandlers for EventCallbackApp {
    fn on_user_registered(
        &mut self,
        user_id: ::std::string::String,
        email: ::std::string::String,
    ) -> app::Result<()> {
        let callback_user_id = format!("callback_{}", user_id);

        self.callback_markers
            .insert(callback_user_id.clone(), "callback_executed".to_string())?;

        // Also store the last callback key for diagnostics
        let _ = self
            .callback_markers
            .insert("last_callback".to_string(), callback_user_id)?;

        Ok(())
    }

    // Defaults for other events are no-ops.
}
