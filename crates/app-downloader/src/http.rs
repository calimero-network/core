//! Fetching an application artifact over HTTP: the client, the redirect guard
//! it applies, and the capped body read every caller shares.

use eyre::bail;

/// Maximum redirects to follow when downloading an application.
const MAX_REDIRECTS: usize = 10;

/// Total request timeout (connect + transfer). Generous so large `.mpk` bundles
/// over slow links still succeed, but bounded so a slow host can't hold the
/// install open indefinitely (slowloris).
const TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Connect timeout for an artifact fetch.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Maximum size of a fetched application artifact. Bounds memory so a hostile
/// host can't OOM the node with an unbounded (or Content-Length-lying) body.
/// Generous headroom for real bundles.
pub const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// The one artifact client. Its target is always the operator's own configured
/// registry, which is routinely private or air-gapped and redirects to storage,
/// so only the scheme and the hop cap are enforced on a redirect.
pub fn registry_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if !matches!(attempt.url().scheme(), "http" | "https") {
                attempt.error("redirect to a non-http(s) scheme is blocked")
            } else if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
}

/// Read an HTTP response body into memory, enforcing a hard byte cap so a
/// missing or lying `Content-Length` can't grow the buffer without bound.
pub async fn read_body_capped(mut response: reqwest::Response, max: u64) -> eyre::Result<Vec<u8>> {
    let mut buf = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        // `> max` means a body of exactly `max` bytes is accepted (`max` is an
        // inclusive limit) while anything larger is rejected before it is
        // buffered — the running total never exceeds `max`.
        if buf.len() as u64 + chunk.len() as u64 > max {
            bail!("application artifact exceeded size limit of {max} bytes");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}
