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
use futures_util::stream::{SplitSink, SplitStream};
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

/// A persistent WebSocket session that keeps the socket open across multiple
/// `execute` calls — the bidirectional counterpart to one-shot HTTP JSON-RPC.
///
/// A one-off call gains nothing from WebSocket over HTTP (it pays the upgrade
/// handshake to send a single frame), so the socket only earns its keep when
/// reused: the interactive `call -i` shell connects once and runs many calls
/// through the same session, correlating each reply by its request `id`.
///
/// Only one request is ever in flight: [`Self::execute`] takes `&mut self`, so
/// the borrow checker forbids concurrent calls on the same session. The id
/// match in the read loop therefore guards against stray event pushes, not
/// against interleaved responses from a second caller.
pub struct WsSession {
    write: SplitSink<WsStream, WsMessage>,
    read: SplitStream<WsStream>,
    /// Monotonic correlation id, incremented per call so a reply can be matched
    /// to its request and distinguished from unsolicited event pushes.
    next_id: WsRequestId,
}

impl WsSession {
    /// Open a session by connecting to the node's `/ws` endpoint.
    pub async fn connect(client: &Client) -> Result<Self> {
        let (write, read) = connect(client).await?.split();
        Ok(Self {
            write,
            read,
            next_id: 1,
        })
    }

    /// Issue one `execute` (query/mutate) call and await its reply, returned in
    /// the same [`Response`] shape as the JSON-RPC path so callers render output
    /// identically regardless of transport.
    ///
    /// Replies are correlated by id: event pushes (which carry no `id`) and any
    /// reply for a different request are skipped until the matching one arrives.
    pub async fn execute(&mut self, request: ExecutionRequest) -> Result<Response> {
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let ws_request = WsRequest {
            id: Some(request_id),
            payload: WsRequestPayload::Execute(request),
        };
        let payload = serde_json::to_string(&ws_request)?;
        self.write
            .send(WsMessage::Text(payload))
            .await
            .wrap_err("failed to send execute request over WebSocket")?;

        while let Some(message) = self.read.next().await {
            match message.wrap_err("error reading from WebSocket")? {
                // `None` is an event push or a reply for another id — keep waiting.
                WsMessage::Text(text) => {
                    if let Some(response) = match_text_reply(&text, request_id)? {
                        return Ok(response);
                    }
                }
                // Keep the connection alive while waiting for the reply.
                WsMessage::Ping(payload) => self.write.send(WsMessage::Pong(payload)).await?,
                WsMessage::Close(_) => {
                    bail!("WebSocket closed before execute response was received")
                }
                _ => {}
            }
        }

        bail!("WebSocket stream ended before execute response was received")
    }

    /// Service the socket while otherwise idle — answering server pings and
    /// detecting a server-side close — so the connection survives long pauses
    /// at an interactive prompt. The node closes a socket that misses pings
    /// (~`ping_interval + pong_timeout`, 40s by default), which would otherwise
    /// kill the shell between commands.
    ///
    /// This never resolves to `Ok`: it loops until the stream errors or closes,
    /// then returns `Err`. Callers race it against their input source (e.g. via
    /// `tokio::select!`) and drop it once a command is ready — sound because
    /// `StreamExt::next` is cancel-safe and no request is outstanding while
    /// idle, so any text frame seen here is an unsolicited event push.
    pub async fn keepalive(&mut self) -> Result<()> {
        while let Some(message) = self.read.next().await {
            match message.wrap_err("error reading from WebSocket")? {
                WsMessage::Ping(payload) => self.write.send(WsMessage::Pong(payload)).await?,
                WsMessage::Close(_) => bail!("WebSocket closed by server"),
                // Ignore event pushes / pongs / binary frames while idle.
                _ => {}
            }
        }

        bail!("WebSocket stream ended")
    }
}

/// Classify an inbound text frame for a call awaiting `request_id`:
/// `Ok(Some(..))` is the matching reply, already adapted to a JSON-RPC
/// [`Response`]; `Ok(None)` means skip it — an unsolicited event push (no `id`)
/// or a reply for some other request.
fn match_text_reply(text: &str, request_id: WsRequestId) -> Result<Option<Response>> {
    let response =
        serde_json::from_str::<WsResponse>(text).wrap_err("failed to parse WebSocket response")?;

    if response.id != Some(request_id) {
        return Ok(None);
    }

    Ok(Some(into_jsonrpc(RequestId::Number(request_id), response)?))
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

    use super::{into_jsonrpc, match_text_reply, RequestId};

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

    #[test]
    fn matching_id_reply_is_returned() {
        let reply = WsResponse {
            id: Some(1),
            body: WsResponseBody::Result(json!({ "output": 42 })),
        };
        let text = serde_json::to_string(&reply).unwrap();

        let response = match_text_reply(&text, 1)
            .unwrap()
            .expect("matching reply should be returned");
        match response.body {
            ResponseBody::Result(result) => assert_eq!(result.0, json!({ "output": 42 })),
            other => panic!("expected result body, got {other:?}"),
        }
    }

    #[test]
    fn reply_for_other_id_is_skipped() {
        let other = WsResponse {
            id: Some(2),
            body: WsResponseBody::Result(json!({ "output": 1 })),
        };
        let text = serde_json::to_string(&other).unwrap();

        assert!(
            match_text_reply(&text, 1).unwrap().is_none(),
            "a reply for a different request id must be skipped"
        );
    }

    #[test]
    fn event_push_without_id_is_skipped() {
        let push = WsResponse {
            id: None,
            body: WsResponseBody::Result(json!({ "event": "state_mutation" })),
        };
        let text = serde_json::to_string(&push).unwrap();

        assert!(
            match_text_reply(&text, 1).unwrap().is_none(),
            "an unsolicited event push (no id) must be skipped"
        );
    }
}
