use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use tokio_test::io::Builder;
    use tokio_util::codec::FramedRead;

    use super::*;

    #[test]
    fn test_my_frame_encoding_decoding() {
        let request = Message {
            data: "Hello".bytes().collect(),
        };
        let response = Message {
            data: "World".bytes().collect(),
        };

        let mut buffer = BytesMut::new();
        let mut codec = MessageJsonCodec::new();
        codec.encode(request.clone(), &mut buffer).unwrap();
        codec.encode(response.clone(), &mut buffer).unwrap();

        let decoded_request = codec.decode(&mut buffer).unwrap();
        assert_eq!(decoded_request, Some(request));

        let decoded_response = codec.decode(&mut buffer).unwrap();
        assert_eq!(decoded_response, Some(response));
    }

    #[tokio::test]
    async fn test_multiple_objects_stream() {
        let request = Message {
            data: "Hello".bytes().collect(),
        };
        let response = Message {
            data: "World".bytes().collect(),
        };

        let mut buffer = BytesMut::new();
        let mut codec = MessageJsonCodec::new();
        codec.encode(request.clone(), &mut buffer).unwrap();
        codec.encode(response.clone(), &mut buffer).unwrap();

        let mut stream = Builder::new().read(&buffer.freeze()).build();
        let mut framed = FramedRead::new(&mut stream, MessageJsonCodec::new());

        let decoded_request = framed.next().await.unwrap().unwrap();
        assert_eq!(decoded_request, request);

        let decoded_response = framed.next().await.unwrap().unwrap();
        assert_eq!(decoded_response, response);

        let decoded3 = framed.next().await;
        assert_eq!(decoded3.is_none(), true);
    }
}
