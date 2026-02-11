pub mod mock_relayer;
pub mod near;

pub trait Protocol {
    const PROTOCOL: &'static str;
}
