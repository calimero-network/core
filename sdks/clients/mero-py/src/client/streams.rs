//! What is not a plain request: the event streams (server-sent events,
//! read until `limit` or `timeout`) and the verification of an app's
//! replicas, both over core's client.

use super::prelude::*;

/// How long a stream is read for, from `timeout` seconds: a negative one is
/// none, one too large for a `Duration` (`inf`) is no limit but `limit`, and
/// NaN is a `ValueError` (it is not a number of seconds to wait).
fn wait_for(timeout: f64) -> PyResult<Duration> {
    if timeout.is_nan() {
        return Err(PyValueError::new_err("timeout must be a number of seconds"));
    }
    Ok(Duration::try_from_secs_f64(timeout.max(0.0)).unwrap_or(Duration::MAX))
}

#[pymethods]
impl PyCloudClient {
    /// Committed events from index `start`: up to `limit` of them, or as many
    /// as arrive within `timeout` seconds. Each is `{"id", "kind", "data"}`.
    #[pyo3(signature = (app, start = 0, limit = 100, timeout = 5.0))]
    fn events(
        &self,
        py: Python<'_>,
        app: &str,
        start: u64,
        limit: usize,
        timeout: f64,
    ) -> PyResult<Py<PyAny>> {
        let app = app.to_owned();
        let wait = wait_for(timeout)?;
        let out = self.run(py, async |c| {
            let stream = c.events(&app, start).await?;
            let mut stream = Box::pin(stream.take(limit));
            let mut out = Vec::new();
            let _elapsed = tokio::time::timeout(wait, async {
                while let Some((id, kind, data)) = stream.next().await {
                    out.push(serde_json::json!({ "id": id, "kind": kind, "data": data }));
                }
            })
            .await;
            Ok(out)
        })?;
        to_py(py, &out)
    }

    /// Presence as a stream: the snapshot first (`{"kind": "snapshot",
    /// "data": [...]}`), then one `{"kind": "presence", "data": {...}}` per
    /// change; up to `limit` of them, or as many as arrive within `timeout`
    /// seconds.
    #[pyo3(signature = (app, limit = 100, timeout = 5.0))]
    fn presence_stream(
        &self,
        py: Python<'_>,
        app: &str,
        limit: usize,
        timeout: f64,
    ) -> PyResult<Py<PyAny>> {
        let app = app.to_owned();
        let wait = wait_for(timeout)?;
        let out = self.run(py, async |c| {
            let stream = c.presence_stream(&app).await?;
            let mut stream = Box::pin(stream.take(limit));
            let mut out = Vec::new();
            let _elapsed = tokio::time::timeout(wait, async {
                while let Some((_id, kind, data)) = stream.next().await {
                    out.push(serde_json::json!({ "kind": kind, "data": data }));
                }
            })
            .await;
            Ok(out)
        })?;
        to_py(py, &out)
    }

    /// Verify every replica of `app`: each one's quote over a fresh nonce,
    /// held to this client's policy, binding its receipt key and the
    /// cluster's governance. Returns `{"keys": {node_id: receipt_key_hex},
    /// "governance": {"releases", "appUpgrades", "outbound"}}`; a call's or
    /// view's `receipt` is to be trusted only from one of those keys.
    fn verify_app_cluster(&self, py: Python<'_>, app: &str) -> PyResult<Py<PyAny>> {
        let app = app.to_owned();
        let cluster = self.run(py, async |c| c.verify_app_cluster(&app).await)?;
        let keys = pyo3::types::PyDict::new(py);
        for (node, key) in &cluster.keys {
            keys.set_item(node, hex::encode(key))?;
        }
        let out = pyo3::types::PyDict::new(py);
        out.set_item("keys", keys)?;
        out.set_item("governance", to_py(py, &cluster.governance)?)?;
        Ok(out.into_any().unbind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_timeout_is_a_wait_or_a_value_error() {
        assert_eq!(wait_for(-1.0).unwrap(), Duration::ZERO);
        assert_eq!(wait_for(1.5).unwrap(), Duration::from_millis(1500));
        assert_eq!(wait_for(f64::INFINITY).unwrap(), Duration::MAX);
        assert_eq!(wait_for(1e300).unwrap(), Duration::MAX);
        assert!(wait_for(f64::NAN).is_err());
    }
}
