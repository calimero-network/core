mod marker;
pub use marker::Near;

#[cfg(feature = "near_client")]
mod transport;
#[cfg(feature = "near_client")]
pub use transport::*;
