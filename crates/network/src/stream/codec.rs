#[cfg(test)]
#[path = "codec_test.rs"]
mod tests;

use core::slice;
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
        let data = item.data.as_ref();
        let data = Bytes::from_static(
            // safety: `LengthDelimitedCodec: Encoder` must prepend the length, so it copies `data`
            unsafe { slice::from_raw_parts(data.as_ptr(), data.len()) },
        );
        self.length_codec
            .encode(data, dst)
            .map_err(CodecError::StdIo)
    }
}
