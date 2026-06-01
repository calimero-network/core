#![allow(clippy::len_without_is_empty)]

use calimero_sdk::{app, ContextId};
use calimero_storage::collections::Counter;

#[app::state(emits = Event)]
pub struct XCallExample {
    /// Counter for tracking pongs received.
    counter: Counter,
}

#[app::event]
pub enum Event {
    PingSent {
        to_context: ContextId,
    },
    PongReceived {
        from_context: ContextId,
        counter: u64,
    },
}

#[app::logic]
impl XCallExample {
    #[app::init]
    pub fn init() -> XCallExample {
        XCallExample {
            counter: Counter::new_with_field_name("counter"),
        }
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
    pub fn ping(&mut self, target_context: ContextId) -> app::Result<()> {
        // `target_context` arrives as a base58 string and is parsed into a
        // `ContextId` by the SDK — no hand-rolled bs58 decoding needed.
        let current_context = ContextId::from(calimero_sdk::env::context_id());

        app::log!(
            "Sending ping from context {} to context {}",
            current_context,
            target_context
        );

        // Prepare the parameters for the cross-context call
        #[derive(calimero_sdk::serde::Serialize)]
        #[serde(crate = "calimero_sdk::serde")]
        struct PongParams {
            from_context: ContextId,
        }

        let params = calimero_sdk::serde_json::to_vec(&PongParams {
            from_context: current_context,
        })?;

        // Make the cross-context call to the pong method
        calimero_sdk::env::xcall(target_context.as_ref(), "pong", &params);

        // Emit an event to notify that a ping was sent
        app::emit!(Event::PingSent {
            to_context: target_context,
        });

        app::log!("Ping sent successfully");

        Ok(())
    }

    /// Receive a pong from another context
    ///
    /// This function is called via xcall from other contexts.
    /// It increments the counter when a pong is received.
    ///
    /// # Arguments
    /// * `from_context` - The ID of the context sending the pong (base58 string over the wire)
    pub fn pong(&mut self, from_context: ContextId) -> app::Result<()> {
        let current_context = ContextId::from(calimero_sdk::env::context_id());

        app::log!(
            "Context {} received pong from {}",
            current_context,
            from_context
        );

        self.counter.increment()?;
        let counter = self.counter.value()?;

        app::emit!(Event::PongReceived {
            from_context,
            counter,
        });

        app::log!("Pong received! Counter is now: {}", counter);

        Ok(())
    }

    /// Get the current counter value
    pub fn get_counter(&self) -> app::Result<u64> {
        let value = self.counter.value()?;
        app::log!("Getting counter value: {}", value);
        Ok(value)
    }
}
