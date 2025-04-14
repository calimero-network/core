pub mod ethereum;
pub mod icp;
pub mod near;
pub mod starknet;
pub mod stellar;
pub mod zksync;

pub trait Protocol {
    const PROTOCOL: &'static str;
}
