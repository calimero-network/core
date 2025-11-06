#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

pub mod broadcast;
pub mod direct;

pub use broadcast::Message as BroadcastMessage;
pub use direct::{InitPayload, MessagePayload, StreamMessage};
