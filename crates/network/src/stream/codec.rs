#[cfg(test)]
#[path = "../tests/stream/codec.rs"]
mod tests;

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Message {
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
#[error("CodecError")]
pub enum CodecError {
    StdIo(#[from] std::io::Error),
    SerDe(serde_json::Error),
}

#[derive(Debug)]
pub(crate) struct MessageJsonCodec {
    length_codec: LengthDelimitedCodec,
}

impl MessageJsonCodec {
    pub fn new() -> Self {
        Self {
            length_codec: LengthDelimitedCodec::new(),
        }
    }
}

impl Decoder for MessageJsonCodec {
    type Item = Message;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(frame) = self.length_codec.decode(src)? else {
            return Ok(None);
        };

        serde_json::from_slice(&frame)
            .map(Some)
            .map_err(CodecError::SerDe)
    }
}

impl Encoder<Message> for MessageJsonCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let json = serde_json::to_vec(&item).map_err(CodecError::SerDe)?;

        self.length_codec
            .encode(Bytes::from(json), dst)
            .map_err(CodecError::StdIo)
    }
}
