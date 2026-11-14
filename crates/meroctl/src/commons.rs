//! What the commands that call Calimero's services share: the app registry and
//! the Cloud's manager (mdma) both serve their API at `/api/...`, and before
//! that only at `/api/v2/...`.

use reqwest::{RequestBuilder, Response, StatusCode};

/// Send the request `request` builds for `{base}/api/{path}`; when the service
/// answers `404`, as a registry or manager from before its unversioned routes
/// does, send it once more to `{base}/api/v2/{path}` and answer that.
pub async fn send_api(
    base: &str,
    path: &str,
    request: impl Fn(String) -> RequestBuilder,
) -> reqwest::Result<Response> {
    let response = request(format!("{base}/api/{path}")).send().await?;
    if response.status() != StatusCode::NOT_FOUND {
        return Ok(response);
    }
    request(format!("{base}/api/v2/{path}")).send().await
}
