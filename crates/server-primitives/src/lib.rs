use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod admin;
pub mod jsonrpc;
pub mod ws;

#[derive(Clone, Copy, Debug, Deserialize, Error, Serialize)]
#[error("Infallible")]
#[allow(clippy::exhaustive_enums)]
pub enum Infallible {}
