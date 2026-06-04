//! Shared WebSocket helpers for meroctl.
//!
//! meroctl talks to a node's `/ws` endpoint for two things: streaming context
//! events (`context watch`) and issuing `execute` (query/mutate) calls over the
//! same persistent socket. Connection setup — deriving the `ws(s)://…/ws` URL
//! and attaching connection-level auth to the upgrade handshake — is shared
//! here so both paths stay in sync.
//!
//! Auth on WebSocket is connection-level: the bearer token is validated once at
//! the HTTP upgrade and individual messages are not re-authenticated. We attach
//! it to the upgrade request below; long-lived sockets that outlive token
//! expiry would need to reconnect.

use calimero_server_primitives::jsonrpc::{
    ExecutionRequest, RequestId, Response, ResponseBody, Version,
};
use calimero_server_primitives::ws::{
    Request as WsRequest, RequestId as WsRequestId, RequestPayload as WsRequestPayload,
    Response as WsResponse,
};
use eyre::{bail, Result, WrapErr};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::client::Client;

/// The connected WebSocket stream type returned by [`connect`].
pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Open a WebSocket connection to the node's `/ws` endpoint, attaching the
/// connection-level auth header when the node requires it.
pub async fn connect(client: &Client) -> Result<WsStream> {
    let url = client.ws_url()?;

    let mut request = url
        .as_str()
        .into_client_request()
        .wrap_err("failed to build WebSocket upgrade request")?;

    if let Some(header) = client.auth_header().await? {
        let value =
            HeaderValue::from_str(&header).wrap_err("auth token is not a valid header value")?;
        let _ = request.headers_mut().insert(AUTHORIZATION, value);
    }

    let (stream, _) = connect_async(request)
        .await
        .wrap_err("failed to connect to WebSocket endpoint")?;

    Ok(stream)
}

/// Issue a single `execute` (query/mutate) call over a fresh WebSocket
/// connection and return the result in the same [`Response`] shape as the
/// JSON-RPC path, so callers render output identically regardless of transport.
///
/// Responses are correlated by the request `id`: event pushes (which carry no
/// `id`) and any unrelated frames are skipped until the matching reply arrives.
pub async fn execute(
    client: &Client,
    id: RequestId,
    request: ExecutionRequest,
) -> Result<Response> {
    let stream = connect(client).await?;
    let (mut write, mut read) = stream.split();

    // Local correlation id on the socket. Only one call is in flight per
    // connection here, but the read loop still matches on it so additional
    // multiplexed calls (or interleaved event pushes) wouldn't be mistaken for
    // this reply.
    const WS_REQUEST_ID: WsRequestId = 1;

    let ws_request = WsRequest {
        id: Some(WS_REQUEST_ID),
        payload: WsRequestPayload::Execute(request),
    };
    let payload = serde_json::to_string(&ws_request)?;
    write
        .send(WsMessage::Text(payload))
        .await
        .wrap_err("failed to send execute request over WebSocket")?;

    while let Some(message) = read.next().await {
        match message.wrap_err("error reading from WebSocket")? {
            WsMessage::Text(text) => {
                let response = serde_json::from_str::<WsResponse>(&text)
                    .wrap_err("failed to parse WebSocket response")?;

                if response.id != Some(WS_REQUEST_ID) {
                    // Event push or an unrelated reply — keep waiting.
                    continue;
                }

                return into_jsonrpc(id, response);
            }
            // Keep the connection alive while waiting for the reply.
            WsMessage::Ping(payload) => write.send(WsMessage::Pong(payload)).await?,
            WsMessage::Close(_) => bail!("WebSocket closed before execute response was received"),
            _ => {}
        }
    }

    bail!("WebSocket stream ended before execute response was received")
}

/// Adapt the WebSocket [`WsResponse`] to the JSON-RPC [`Response`]. The two wire
/// envelopes share an identical body shape on the wire (same camelCase tags),
/// so the body round-trips cleanly through `serde_json::Value`; only the
/// outer framing (`jsonrpc` version + echoed `id`) differs.
fn into_jsonrpc(id: RequestId, response: WsResponse) -> Result<Response> {
    let body_value = serde_json::to_value(&response.body)?;
    let body = serde_json::from_value::<ResponseBody>(body_value)
        .wrap_err("failed to adapt WebSocket response body")?;

    Ok(Response::new(Version::TwoPointZero, id, body))
}

#[cfg(test)]
mod tests {
    use calimero_server_primitives::jsonrpc::{ResponseBody, ResponseBodyError};
    use calimero_server_primitives::ws::{
        Response as WsResponse, ResponseBody as WsResponseBody,
        ResponseBodyError as WsResponseBodyError, ServerResponseError as WsServerResponseError,
    };
    use serde_json::json;

    use super::{into_jsonrpc, RequestId};

    #[test]
    fn result_body_adapts_and_echoes_id() {
        let ws = WsResponse {
            id: Some(1),
            body: WsResponseBody::Result(json!({ "output": 42 })),
        };

        let response = into_jsonrpc(RequestId::Number(7), ws).unwrap();

        assert!(matches!(response.id, RequestId::Number(7)));
        match response.body {
            ResponseBody::Result(result) => assert_eq!(result.0, json!({ "output": 42 })),
            other => panic!("expected result body, got {other:?}"),
        }
    }

    #[test]
    fn handler_error_body_is_preserved() {
        let ws = WsResponse {
            id: Some(1),
            body: WsResponseBody::Error(WsResponseBodyError::HandlerError(
                json!({ "type": "FunctionCallError", "data": "boom" }),
            )),
        };

        let response = into_jsonrpc(RequestId::Null, ws).unwrap();

        match response.body {
            ResponseBody::Error(ResponseBodyError::HandlerError(value)) => {
                assert_eq!(
                    value,
                    json!({ "type": "FunctionCallError", "data": "boom" })
                );
            }
            other => panic!("expected handler error body, got {other:?}"),
        }
    }

    #[test]
    fn server_parse_error_maps_to_server_error() {
        let ws = WsResponse {
            id: Some(1),
            body: WsResponseBody::Error(WsResponseBodyError::ServerError(
                WsServerResponseError::ParseError("bad args".to_owned()),
            )),
        };

        let response = into_jsonrpc(RequestId::Null, ws).unwrap();

        assert!(matches!(
            response.body,
            ResponseBody::Error(ResponseBodyError::ServerError(_))
        ));
    }
}
