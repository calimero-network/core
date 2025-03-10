pub mod evm;
pub mod icp;
pub mod near;
pub mod starknet;
pub mod stellar;

pub trait Protocol {
    const PROTOCOL: &'static str;
}
