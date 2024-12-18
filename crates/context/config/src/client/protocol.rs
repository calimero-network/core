pub mod evm;
pub mod icp;
pub mod near;
pub mod starknet;

pub trait Protocol {
    const PROTOCOL: &'static str;
}
