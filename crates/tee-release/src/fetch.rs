//! Download a signed release asset and verify it before handing it back.

use eyre::{bail, Result as EyreResult};
use tracing::warn;

use crate::sigstore_verify::{verify_signed_asset, WorkflowIdentity};

/// Where every mero-tee release asset is downloaded from.
pub const MERO_TEE_RELEASE_BASE: &str =
    "https://github.com/calimero-network/mero-tee/releases/download";

const FETCH_RETRIES: usize = 3;
/// Upper bound on the exponential fetch backoff. Without it a large `attempt`
/// would saturate to `u64::MAX` milliseconds; this caps the wait at a practical
/// ceiling.
const FETCH_MAX_BACKOFF_MS: u64 = 60_000;

/// Fetch `asset` from release `tag` together with its `.sig` and
/// `.bundle.json`, verify it was signed by `identity`, and return its body.
///
/// Transient failures (5xx, 429, network) are retried with backoff; a 404 or
/// any other client error is final, and so is a failed verification, which is
/// never retried because a bad signature does not become good.
pub async fn fetch_verified_asset(
    tag: &str,
    asset: &str,
    identity: &WorkflowIdentity,
) -> EyreResult<String> {
    match fetch_signed(&http_client()?, MERO_TEE_RELEASE_BASE, tag, asset).await? {
        Signed::Published(signed) => signed.verify(tag, asset, identity).await,
        Signed::NotPublished(error) => bail!("{error}"),
    }
}

/// [`fetch_verified_asset`], except that a release which does not publish
/// `asset` at all (a 404 on the asset itself) is `Ok(None)` rather than an
/// error, so a caller can fall back to another asset.
///
/// Only the asset's own 404 means "not published". A published asset whose
/// `.sig` or `.bundle.json` is missing is still an error: it is a broken
/// release, not a reason to try a different file.
pub async fn fetch_verified_asset_if_published(
    tag: &str,
    asset: &str,
    identity: &WorkflowIdentity,
) -> EyreResult<Option<String>> {
    match fetch_signed(&http_client()?, MERO_TEE_RELEASE_BASE, tag, asset).await? {
        Signed::Published(signed) => signed.verify(tag, asset, identity).await.map(Some),
        Signed::NotPublished(_) => Ok(None),
    }
}

fn http_client() -> EyreResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("merod/1.0")
        .build()
        .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))
}

/// An asset with its detached signature and Sigstore bundle, not yet verified.
struct SignedBodies {
    body: String,
    signature: String,
    bundle: String,
}

impl SignedBodies {
    async fn verify(
        self,
        tag: &str,
        asset: &str,
        identity: &WorkflowIdentity,
    ) -> EyreResult<String> {
        verify_signed_asset(
            self.body.as_bytes(),
            &self.signature,
            &self.bundle,
            identity,
        )
        .await
        .map_err(|e| eyre::eyre!("{asset} from {tag} failed signature verification: {e}"))?;
        Ok(self.body)
    }
}

enum Signed {
    Published(SignedBodies),
    /// The asset itself answered 404; carries the fetch error.
    NotPublished(String),
}

/// Download `asset`, `asset.sig` and `asset.bundle.json` from `base`/`tag`.
async fn fetch_signed(
    client: &reqwest::Client,
    base: &str,
    tag: &str,
    asset: &str,
) -> EyreResult<Signed> {
    let signature_name = format!("{asset}.sig");
    let bundle_name = format!("{asset}.bundle.json");
    let names = [asset, signature_name.as_str(), bundle_name.as_str()];

    let mut last_error = String::new();
    'attempts: for attempt in 1..=FETCH_RETRIES {
        let mut bodies = Vec::with_capacity(names.len());
        for (index, name) in names.into_iter().enumerate() {
            let url = format!("{base}/{tag}/{name}");
            match fetch_release_asset(client, &url, name).await {
                AssetFetchResult::Success(body) => bodies.push(body),
                AssetFetchResult::NotFound(error) if index == 0 => {
                    return Ok(Signed::NotPublished(error));
                }
                AssetFetchResult::Transient(error) => {
                    last_error = error;
                    if attempt < FETCH_RETRIES {
                        warn!(attempt, retries = FETCH_RETRIES, error = %last_error, "transient release asset fetch failure, retrying");
                        tokio::time::sleep(fetch_backoff(attempt)).await;
                        continue 'attempts;
                    }
                    break 'attempts;
                }
                AssetFetchResult::NotFound(error) | AssetFetchResult::Permanent(error) => {
                    last_error = error;
                    break 'attempts;
                }
            }
        }
        let [body, signature, bundle]: [String; 3] = bodies
            .try_into()
            .map_err(|_| eyre::eyre!("internal: expected three release asset bodies"))?;
        return Ok(Signed::Published(SignedBodies {
            body,
            signature,
            bundle,
        }));
    }

    bail!("{last_error}")
}

enum AssetFetchResult {
    Success(String),
    /// 404: the release has no such asset.
    NotFound(String),
    Transient(String),
    Permanent(String),
}

async fn fetch_release_asset(
    client: &reqwest::Client,
    url: &str,
    asset_name: &str,
) -> AssetFetchResult {
    match client.get(url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(body) => AssetFetchResult::Success(body),
            Err(err) => AssetFetchResult::Transient(format!(
                "Failed to read {asset_name} response body: {err}"
            )),
        },
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let error = format!("Failed to fetch {asset_name}: {status} {url} {body}");
            if status == reqwest::StatusCode::NOT_FOUND {
                AssetFetchResult::NotFound(error)
            } else if status.is_server_error() || status.as_u16() == 429 {
                AssetFetchResult::Transient(error)
            } else {
                AssetFetchResult::Permanent(error)
            }
        }
        Err(err) => AssetFetchResult::Transient(format!("Failed to fetch {asset_name}: {err}")),
    }
}

/// Backoff before retry `attempt` (1-based): 250ms doubling, capped.
pub fn fetch_backoff(attempt: usize) -> std::time::Duration {
    let exponent = u32::try_from(attempt).unwrap_or(u32::MAX).saturating_sub(1);
    // `1 << exponent` panics once `exponent >= 64` (attempts past ~64); fall back
    // to u64::MAX so the saturating_mul below just clamps to the max backoff.
    let factor = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
    let millis = 250_u64.saturating_mul(factor).min(FETCH_MAX_BACKOFF_MS);
    std::time::Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::extract::Path;
    use axum::http::StatusCode;
    use axum::routing::get;

    use super::{fetch_backoff, fetch_signed, Signed, FETCH_MAX_BACKOFF_MS};

    #[test]
    fn fetch_backoff_grows_then_caps() {
        // The first attempts grow exponentially from the 250ms base.
        assert_eq!(fetch_backoff(1).as_millis(), 250);
        assert_eq!(fetch_backoff(2).as_millis(), 500);
        assert_eq!(fetch_backoff(4).as_millis(), 2000);

        // Attempt 8 (250 * 128 = 32s) is the last value under the 60s ceiling,
        // and attempt 9 (250 * 256 = 64s) is the first clamped to it.
        let cap = u128::from(FETCH_MAX_BACKOFF_MS);
        assert_eq!(fetch_backoff(8).as_millis(), 32_000);
        assert_eq!(fetch_backoff(9).as_millis(), cap);

        // Attempt 65 (exponent 64) is the u64 shift-width boundary where
        // `checked_shl` returns None; the guards must clamp it, not panic.
        assert_eq!(fetch_backoff(65).as_millis(), cap);
        assert_eq!(fetch_backoff(usize::MAX).as_millis(), cap);
    }

    /// Serve `assets` (name -> body) under `/<tag>/`; anything else is a 404.
    async fn release_server(assets: &[(&'static str, &'static str)]) -> (String, reqwest::Client) {
        let assets: HashMap<&'static str, &'static str> = assets.iter().copied().collect();
        let app = axum::Router::new().route(
            "/{tag}/{asset}",
            get(move |Path((_tag, asset)): Path<(String, String)>| {
                let body = assets.get(asset.as_str()).copied();
                async move { body.ok_or(StatusCode::NOT_FOUND) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have an addr");
        drop(tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("release test server should run");
        }));
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client should build");
        (format!("http://{addr}"), client)
    }

    #[tokio::test]
    async fn an_asset_the_release_does_not_publish_is_not_an_error() {
        let (base, client) = release_server(&[]).await;
        let signed = fetch_signed(&client, &base, "v1", "policy.json")
            .await
            .expect("a 404 on the asset itself is an answer, not a failure");
        assert!(matches!(signed, Signed::NotPublished(_)));
    }

    #[tokio::test]
    async fn a_published_asset_comes_back_with_its_signature_and_bundle() {
        let (base, client) = release_server(&[
            ("policy.json", "body"),
            ("policy.json.sig", "sig"),
            ("policy.json.bundle.json", "bundle"),
        ])
        .await;
        let Signed::Published(signed) = fetch_signed(&client, &base, "v1", "policy.json")
            .await
            .expect("all three assets are published")
        else {
            panic!("the asset is published");
        };
        assert_eq!(
            (
                signed.body.as_str(),
                signed.signature.as_str(),
                signed.bundle.as_str()
            ),
            ("body", "sig", "bundle")
        );
    }

    #[tokio::test]
    async fn a_published_asset_missing_its_signature_is_an_error() {
        let (base, client) = release_server(&[
            ("policy.json", "body"),
            ("policy.json.bundle.json", "bundle"),
        ])
        .await;
        let err = fetch_signed(&client, &base, "v1", "policy.json")
            .await
            .err()
            .expect("a half-published asset is a broken release, not an absent one");
        assert!(err.to_string().contains("policy.json.sig"), "{err}");
    }
}
