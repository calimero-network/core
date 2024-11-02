use super::protocol::Protocol;

pub mod config;
// pub mod proxy;

pub trait Method<P: Protocol> {
    type Returns;

    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}
