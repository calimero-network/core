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
