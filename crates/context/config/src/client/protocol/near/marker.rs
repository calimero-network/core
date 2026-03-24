use crate::client::protocol::Protocol;

#[derive(Copy, Clone, Debug)]
pub enum Near {}

impl Protocol for Near {
    const PROTOCOL: &'static str = "near";
}
