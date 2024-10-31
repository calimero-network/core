pub mod near;
pub mod starknet;

pub trait Protocol {}

pub trait Method<Params>: Protocol {
    type Returns;

    const METHOD: &'static str;

    fn encode(params: &Params) -> eyre::Result<Vec<u8>>;
    fn decode(response: &[u8]) -> eyre::Result<Self::Returns>;
}
