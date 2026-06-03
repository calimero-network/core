#[cfg(test)]
#[path = "codec_test.rs"]
mod tests;

use std::borrow::Cow;
use std::io::Error as IoError;

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Message<'a> {
    pub data: Cow<'a, [u8]>,
}

impl<'a> Message<'a> {
    #[must_use]
    pub fn new<T: Into<Cow<'a, [u8]>>>(data: T) -> Self {
        Self { data: data.into() }
    }
}

#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum CodecError {
    #[error(transparent)]
    StdIo(#[from] IoError),
}

#[derive(Debug)]
pub struct MessageCodec {
    length_codec: LengthDelimitedCodec,
}

impl MessageCodec {
    pub fn new(max_message_size: usize) -> Self {
        let mut length_codec = LengthDelimitedCodec::new();
        length_codec.set_max_frame_length(max_message_size);
        Self { length_codec }
    }
}

impl Decoder for MessageCodec {
    type Item = Message<'static>;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(frame) = self.length_codec.decode(src)? else {
            return Ok(None);
        };

        Ok(Some(Message {
            data: Cow::Owned(frame.into()),
        }))
    }
}

impl<'a> Encoder<Message<'a>> for MessageCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Message<'a>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // Hand the length codec owned bytes: an already-owned payload moves in for free,
        // a borrowed one is copied once (which the length-prefix codec would do anyway).
        // This avoids fabricating a `'static` borrow whose soundness would hinge on
        // `LengthDelimitedCodec` copying before it returns — an unenforced cross-crate invariant.
        let data = match item.data {
            Cow::Borrowed(data) => Bytes::copy_from_slice(data),
            Cow::Owned(data) => Bytes::from(data),
        };
        self.length_codec
            .encode(data, dst)
            .map_err(CodecError::StdIo)
    }
}
