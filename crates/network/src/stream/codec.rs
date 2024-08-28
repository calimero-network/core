#[cfg(test)]
#[path = "../tests/stream/codec.rs"]
mod tests;

use std::io::Error as IoError;

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use serde_json::{from_slice as from_json_slice, to_vec as to_json_vec, Error as JsonError};
use thiserror::Error as ThisError;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Message {
    pub data: Vec<u8>,
}

impl Message {
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

#[derive(Debug, ThisError)]
#[error("CodecError")]
#[non_exhaustive]
pub enum CodecError {
    StdIo(#[from] IoError),
    SerDe(JsonError),
}

#[derive(Debug)]
pub struct MessageJsonCodec {
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

        from_json_slice(&frame).map(Some).map_err(CodecError::SerDe)
    }
}

impl Encoder<Message> for MessageJsonCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let json = to_json_vec(&item).map_err(CodecError::SerDe)?;

        self.length_codec
            .encode(Bytes::from(json), dst)
            .map_err(CodecError::StdIo)
    }
}
