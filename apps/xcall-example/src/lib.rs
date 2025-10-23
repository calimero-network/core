#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::Vector;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct XCallExample {
    /// Vector of received messages from other contexts
    messages: Vector<Message>,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Message {
    pub from_context: [u8; 32],
    pub content: String,
}

#[app::event]
pub enum Event<'a> {
    GreetingSent {
        to_context: [u8; 32],
        message: &'a str,
    },
    GreetingReceived {
        from_context: [u8; 32],
        message: &'a str,
    },
}

#[app::logic]
impl XCallExample {
    #[app::init]
    pub fn init() -> XCallExample {
        XCallExample {
            messages: Vector::new(),
        }
    }

    /// Send a greeting to another context via cross-context call
    ///
    /// # Arguments
    /// * `target_context` - The 32-byte ID of the context to send the greeting to
    /// * `message` - The greeting message to send
    ///
    /// # Example
    /// ```json
    /// {
    ///   "target_context": "0x1234...",
    ///   "message": "Hello from Context A!"
    /// }
    /// ```
    pub fn send_greeting(&mut self, target_context: [u8; 32], message: String) -> app::Result<()> {
        let current_context = calimero_sdk::env::context_id();
        
        app::log!(
            "Sending greeting from context {:?} to context {:?}: {}",
            current_context,
            target_context,
            message
        );

        // Prepare the parameters for the cross-context call
        // The parameters must be JSON-encoded to match the target function's signature
        #[derive(calimero_sdk::serde::Serialize)]
        #[serde(crate = "calimero_sdk::serde")]
        struct ReceiveGreetingParams {
            from_context: [u8; 32],
            message: String,
        }

        let params = calimero_sdk::serde_json::to_vec(&ReceiveGreetingParams {
            from_context: current_context,
            message: message.clone(),
        })?;

        // Make the cross-context call
        // This will execute the "receive_greeting" function on the target context
        // after this execution completes
        calimero_sdk::env::xcall(&target_context, "receive_greeting", &params);

        // Emit an event to notify that a greeting was sent
        app::emit!(Event::GreetingSent {
            to_context: target_context,
            message: &message,
        });

        app::log!("Cross-context call queued successfully");

        Ok(())
    }

    /// Receive a greeting from another context
    ///
    /// This function is called via xcall from other contexts
    ///
    /// # Arguments
    /// * `from_context` - The 32-byte ID of the context sending the greeting
    /// * `message` - The greeting message
    pub fn receive_greeting(
        &mut self,
        from_context: [u8; 32],
        message: String,
    ) -> app::Result<()> {
        let current_context = calimero_sdk::env::context_id();
        
        app::log!(
            "Context {:?} received greeting from {:?}: {}",
            current_context,
            from_context,
            message
        );

        // Store the message
        self.messages.push(Message {
            from_context,
            content: message.clone(),
        })?;

        // Emit an event to notify that a greeting was received
        app::emit!(Event::GreetingReceived {
            from_context,
            message: &message,
        });

        app::log!("Greeting stored successfully");

        Ok(())
    }

    /// Get all received messages
    ///
    /// Returns a vector of all messages received from other contexts
    pub fn get_messages(&self) -> app::Result<Vec<Message>> {
        app::log!("Retrieving all messages");

        let mut messages = Vec::new();
        for i in 0..self.messages.len()? {
            if let Some(msg) = self.messages.get(i)? {
                messages.push(msg);
            }
        }

        app::log!("Retrieved {} messages", messages.len());

        Ok(messages)
    }

    /// Get the number of received messages
    pub fn message_count(&self) -> app::Result<usize> {
        app::log!("Getting message count");
        
        Ok(self.messages.len()? as usize)
    }

    /// Clear all received messages
    pub fn clear_messages(&mut self) -> app::Result<()> {
        app::log!("Clearing all messages");
        
        self.messages.clear()?;

        Ok(())
    }
}

