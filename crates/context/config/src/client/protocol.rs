pub mod ethereum;
pub mod icp;
pub mod mock_relayer;
pub mod near;
pub mod starknet;

pub trait Protocol {
    const PROTOCOL: &'static str;
}
