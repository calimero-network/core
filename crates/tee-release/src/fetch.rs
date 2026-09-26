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
    let asset_url = format!("{MERO_TEE_RELEASE_BASE}/{tag}/{asset}");
    let signature_name = format!("{asset}.sig");
    let bundle_name = format!("{asset}.bundle.json");
    let signature_url = format!("{MERO_TEE_RELEASE_BASE}/{tag}/{signature_name}");
    let bundle_url = format!("{MERO_TEE_RELEASE_BASE}/{tag}/{bundle_name}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("merod/1.0")
        .build()
        .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))?;

    let mut last_error = String::new();
    'attempts: for attempt in 1..=FETCH_RETRIES {
        let mut bodies = Vec::with_capacity(3);
        for (url, name) in [
            (&asset_url, asset),
            (&signature_url, signature_name.as_str()),
            (&bundle_url, bundle_name.as_str()),
        ] {
            match fetch_release_asset(&client, url, name).await {
                AssetFetchResult::Success(body) => bodies.push(body),
                AssetFetchResult::Transient(error) => {
                    last_error = error;
                    if attempt < FETCH_RETRIES {
                        warn!(attempt, retries = FETCH_RETRIES, error = %last_error, "transient release asset fetch failure, retrying");
                        tokio::time::sleep(fetch_backoff(attempt)).await;
                        continue 'attempts;
                    }
                    break 'attempts;
                }
                AssetFetchResult::Permanent(error) => {
                    last_error = error;
                    break 'attempts;
                }
            }
        }
        let [body, signature, bundle]: [String; 3] = bodies
            .try_into()
            .map_err(|_| eyre::eyre!("internal: expected three release asset bodies"))?;
        verify_signed_asset(body.as_bytes(), &signature, &bundle, identity)
            .await
            .map_err(|e| eyre::eyre!("{asset} from {tag} failed signature verification: {e}"))?;
        return Ok(body);
    }

    bail!("{last_error}")
}

enum AssetFetchResult {
    Success(String),
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
            if status.is_server_error() || status.as_u16() == 429 {
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
    use super::{fetch_backoff, FETCH_MAX_BACKOFF_MS};

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
}
