use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

pub mod admin;
pub mod jsonrpc;
pub mod ws;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[error("Infallible")]
#[allow(clippy::exhaustive_enums)]
pub enum Infallible {}
