#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};

#[app::state(emits = Event)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct XCallExample {
    /// Counter for tracking pongs received
    counter: u64,
}

#[app::event]
pub enum Event {
    PingSent {
        to_context: [u8; 32],
    },
    PongReceived {
        from_context: [u8; 32],
        counter: u64,
    },
}

#[app::logic]
impl XCallExample {
    #[app::init]
    pub fn init() -> XCallExample {
        XCallExample { counter: 0 }
    }

    /// Send a ping to another context via cross-context call
    ///
    /// # Arguments
    /// * `target_context` - The base58-encoded ID of the context to send the ping to
    ///
    /// # Example
    /// ```json
    /// {
    ///   "target_context": "AmxF5dVaqTTAWNbv4uDJhxdoQTEY1wfv6Ld8Gjbu6Zdk"
    /// }
    /// ```
    pub fn ping(&mut self, target_context: String) -> app::Result<()> {
        // Decode the base58 context ID to bytes
        let target_context_bytes: [u8; 32] = bs58::decode(&target_context)
            .into_vec()
            .map_err(|e| {
                calimero_sdk::types::Error::msg(format!("Failed to decode context ID: {}", e))
            })?
            .try_into()
            .map_err(|_| calimero_sdk::types::Error::msg("Context ID must be exactly 32 bytes"))?;

        let current_context = calimero_sdk::env::context_id();

        app::log!(
            "Sending ping from context {:?} to context {}",
            current_context,
            target_context
        );

        // Prepare the parameters for the cross-context call
        #[derive(calimero_sdk::serde::Serialize)]
        #[serde(crate = "calimero_sdk::serde")]
        struct PongParams {
            from_context: [u8; 32],
        }

        let params = calimero_sdk::serde_json::to_vec(&PongParams {
            from_context: current_context,
        })?;

        // Make the cross-context call to the pong method
        calimero_sdk::env::xcall(&target_context_bytes, "pong", &params);

        // Emit an event to notify that a ping was sent
        app::emit!(Event::PingSent {
            to_context: target_context_bytes,
        });

        app::log!("Ping sent successfully");

        Ok(())
    }

    /// Receive a pong from another context
    ///
    /// This function is called via xcall from other contexts
    /// It increments the counter when a pong is received
    ///
    /// # Arguments
    /// * `from_context` - The 32-byte ID of the context sending the pong
    pub fn pong(&mut self, from_context: [u8; 32]) -> app::Result<()> {
        let current_context = calimero_sdk::env::context_id();

        app::log!(
            "Context {:?} received pong from {:?}",
            current_context,
            from_context
        );

        // Increment the counter
        self.counter += 1;

        // Emit an event to notify that a pong was received
        app::emit!(Event::PongReceived {
            from_context,
            counter: self.counter,
        });

        app::log!("Pong received! Counter is now: {}", self.counter);

        Ok(())
    }

    /// Get the current counter value
    pub fn get_counter(&self) -> app::Result<u64> {
        app::log!("Getting counter value: {}", self.counter);
        Ok(self.counter)
    }

    /// Reset the counter to zero
    pub fn reset_counter(&mut self) -> app::Result<()> {
        app::log!("Resetting counter");
        self.counter = 0;
        Ok(())
    }
}
