use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

pub mod admin;
pub mod blob;
pub mod jsonrpc;
<<<<<<< HEAD

=======
pub mod sse;
>>>>>>> cb545aaa (feat(server/sse): added sse request and node connection)
pub mod ws;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[error("Infallible")]
#[expect(clippy::exhaustive_enums, reason = "This will never have any variants")]
pub enum Infallible {}
