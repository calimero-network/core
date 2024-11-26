pub mod near;
pub mod starknet;
pub mod icp;

pub trait Protocol {
    const PROTOCOL: &'static str;
}
