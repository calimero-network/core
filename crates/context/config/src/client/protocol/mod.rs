use super::Error;

pub mod near;
pub mod starknet;

pub mod private {
    pub trait Protocol {}
}

pub trait Method<Params>: private::Protocol {
    type Returns;

    const METHOD: &'static str;

    fn encode(params: &Params) -> Result<Vec<u8>, Error>;
    fn decode(response: &[u8]) -> Result<Self::Returns, Error>;
}
