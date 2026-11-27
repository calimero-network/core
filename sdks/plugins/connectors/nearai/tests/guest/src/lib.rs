//! Test guest for `tests/harness.rs`: an app whose action asks NEAR AI through the connector.

use calimero_connector_nearai::{chat, Answer, Prompt};
use calimero_sdk::prelude::*;

#[app::state(version = 1)]
pub struct Guest {}

#[app::logic]
impl Guest {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// NEAR AI's answer to `prompt`.
    #[app::action]
    pub fn ask(ctx: &mut ActionCtx, prompt: Prompt) -> app::Result<Answer> {
        Ok(chat(ctx, &prompt)?)
    }
}
