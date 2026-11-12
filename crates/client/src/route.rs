//! What the generated route layer ([`crate::routes`]) calls: sending a route
//! through the [`Transport`] and reading its answer as the type the document
//! gives it. Hand-written, so the generator only names routes.

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::transport::{api_error, Request, Transport};
use crate::{retry_unavailable, Error, Passkey, Result};

/// The operator's signer: Ed25519 over the bytes a `428
/// OPERATOR_SIGNATURE_REQUIRED` names (`details.message`), returning the
/// 64-byte signature.
pub type OperatorSigner<'a> = dyn Fn(&[u8]) -> Vec<u8> + Sync + 'a;

/// One URL path segment: everything but unreserved characters escaped, so an
/// id cannot reach another route.
pub(crate) fn segment(s: &str) -> String {
    crate::path_segment(s)
}

/// A new idempotency key for a retry-safe route the caller gave none
/// (`x-calimero-retry-requires`).
pub(crate) fn new_key() -> String {
    crate::new_call_key()
}

/// `v` as JSON.
pub(crate) fn value<T: Serialize + ?Sized>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Transport(e.into()))
}

/// Set the body field `name` to `v`.
pub(crate) fn put<T: Serialize + ?Sized>(
    fields: &mut Map<String, Value>,
    name: &str,
    v: &T,
) -> Result<()> {
    let _prev = fields.insert(name.to_owned(), value(v)?);
    Ok(())
}

/// Set the body field `name` to `v` when there is one (`Some(None)` sends `null`).
pub(crate) fn put_some<T: Serialize>(
    fields: &mut Map<String, Value>,
    name: &str,
    v: Option<T>,
) -> Result<()> {
    match v {
        Some(v) => put(fields, name, &v),
        None => Ok(()),
    }
}

/// Read `v` as the route's answer type.
fn decode<T: DeserializeOwned>(what: &str, v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| {
        Error::Transport(eyre::eyre!(
            "{what}: an answer this client cannot read: {e}"
        ))
    })
}

fn what(req: &Request) -> String {
    format!("{} /{}", req.method, req.path.trim_start_matches('/'))
}

/// Send a JSON route and read its answer. `passkey` answers a `412
/// CONFIRM_REQUIRED`, `operator` a `428 OPERATOR_SIGNATURE_REQUIRED`
/// ([`Transport::approved`]); a `retry` route without either is sent again
/// with the same body while the node is unavailable (D70). An approved
/// request is not: its challenge is single-use.
pub(crate) async fn json<T: DeserializeOwned>(
    t: &Transport,
    req: Request,
    passkey: Option<&mut dyn Passkey>,
    operator: Option<&OperatorSigner<'_>>,
    retry: bool,
) -> Result<T> {
    let what = what(&req);
    let v = if retry && passkey.is_none() && operator.is_none() {
        retry_unavailable(|| t.json(req.clone())).await?
    } else {
        t.approved(req, passkey, operator).await?
    };
    decode(&what, v)
}

/// Send a route the node serves in the clear (`x-calimero-sealed: false`:
/// `/health`, `/attestation`), without a credential.
pub(crate) async fn clear<T: DeserializeOwned>(t: &Transport, req: Request) -> Result<T> {
    let what = what(&req);
    let mut url = t
        .base()
        .join(req.path.trim_start_matches('/'))
        .map_err(eyre::Report::from)?;
    if !req.query.is_empty() {
        let _pairs = url.query_pairs_mut().extend_pairs(&req.query);
    }
    let resp = t
        .http()
        .request(req.method, url)
        .send()
        .await
        .map_err(eyre::Report::from)?;
    decode(&what, crate::transport::answer(resp).await?)
}

/// Send a route that answers raw bytes (a blob).
pub(crate) async fn bytes(t: &Transport, req: Request) -> Result<Vec<u8>> {
    bytes_up_to(t, req, usize::MAX).await
}

/// [`bytes`], refusing an answer longer than `max` before it is buffered: by its `Content-Length`,
/// and by a running count while reading, so a missing or false length cannot exceed it.
pub(crate) async fn bytes_up_to(t: &Transport, req: Request, max: usize) -> Result<Vec<u8>> {
    let what = what(&req);
    let resp = t.send(req).await?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        return Err(api_error(status, &v));
    }
    let too_long = || eyre::eyre!("{what}: the answer is longer than the {max} bytes allowed");
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Err(too_long().into());
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(eyre::Report::from)?;
        if chunk.len() > max - body.len() {
            return Err(too_long().into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Send a route that answers text (`/ops/metrics`); a refusal is read as
/// JSON when it is, else its text is the message.
pub(crate) async fn text(t: &Transport, req: Request) -> Result<String> {
    let resp = t.send(req).await?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(eyre::Report::from)?;
    if status < 400 {
        return Ok(text);
    }
    Err(match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.get("error").is_some() => api_error(status, &v),
        _ => Error::Api {
            status,
            code: "ERROR".into(),
            message: text,
            details: Value::Null,
        },
    })
}

/// Serde for a field that may be absent: present (even as `null`) is `Some`.
/// For `Option<Option<T>>` (absent, `null`, a value) and for an optional
/// `Value`, whose `null` is not its absence.
pub(crate) mod present {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<T: Serialize, S: Serializer>(
        v: &Option<T>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => v.serialize(s),
            None => s.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<T>, D::Error> {
        T::deserialize(d).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Map, Value};

    use super::{put, put_some};

    /// What a generated method sends for its flat fields: a field left out
    /// (`None`) is not in the body, `Some(None)` is `null`, `Some(v)` is `v`.
    #[test]
    fn a_field_left_out_is_not_sent_and_a_null_is() {
        let mut fields = Map::new();
        put(&mut fields, "origin", &"https://kv.example.com").unwrap();
        put_some(&mut fields, "rpId", None::<&str>).unwrap();
        put_some(&mut fields, "served", Some(true)).unwrap();
        put_some(&mut fields, "parent", Some(None::<&str>)).unwrap();
        assert_eq!(
            Value::Object(fields),
            json!({ "origin": "https://kv.example.com", "served": true, "parent": null })
        );
    }

    /// A field that may be absent or `null` keeps which it was, both ways.
    #[test]
    fn an_explicit_null_is_not_an_absent_field() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct Probe {
            #[serde(
                default,
                skip_serializing_if = "Option::is_none",
                with = "super::present"
            )]
            parent: Option<Option<String>>,
            #[serde(
                default,
                skip_serializing_if = "Option::is_none",
                with = "super::present"
            )]
            metadata: Option<Value>,
        }
        for (wire, parent, metadata) in [
            (json!({}), None, None),
            (json!({ "parent": null }), Some(None), None),
            (json!({ "parent": "t" }), Some(Some("t".to_owned())), None),
            (json!({ "metadata": null }), None, Some(Value::Null)),
        ] {
            let p: Probe = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(p, Probe { parent, metadata });
            assert_eq!(serde_json::to_value(&p).unwrap(), wire);
        }
    }
}
