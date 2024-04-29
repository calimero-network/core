use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod admin;
pub mod jsonrpc;
pub mod ws;

#[derive(Debug, Serialize, Deserialize, Error)]
#[error("Infallible")]
pub enum Infallible {}
