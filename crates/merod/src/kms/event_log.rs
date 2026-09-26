//! The KMS's compose hash, read from its RTMR3 event log.
//!
//! A node's storage key is derived by dstack from the KMS's **app** key. Whoever
//! can run code under that app can derive it, so the KMS the node trusts must be
//! one running a compose file that mero-tee released — not merely one whose
//! registers happen to be allowlisted (mero-tee#338).
//!
//! dstack measures the compose file into RTMR3 as a runtime event named
//! `compose-hash`. The quote carries only the final register value, so the
//! event log `/attest` returns is what says which compose file booted, and only
//! once replaying it reproduces the quote's RTMR3. Every RTMR3 event's digest is
//! recomputed from its contents here, never taken from the log, so a log that
//! pairs a genuine digest chain with a substituted payload does not replay.
//!
//! Replay, as dstack extends RTMR3 (and `verify_dstack_compose_hash.py` in
//! mero-tee checks it):
//!
//! ```text
//! digest = SHA384(event_type as u32 LE || ":" || event || ":" || payload)
//! rtmr3  = SHA384(rtmr3 || digest), starting from 48 zero bytes
//! ```

use calimero_config::normalize_attestation_measurement;
use eyre::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha384};

/// The register dstack's runtime events extend.
const RUNTIME_EVENT_IMR: u32 = 3;
/// The runtime event that measures the app's compose file.
const COMPOSE_HASH_EVENT: &str = "compose-hash";
/// The runtime event that measures the dstack app id.
const APP_ID_EVENT: &str = "app-id";
/// Bound on the entries accepted, so a hostile KMS cannot make the node hash
/// an arbitrarily long log. A real one carries a few dozen.
const MAX_EVENT_LOG_ENTRIES: usize = 4096;
/// A compose hash is a SHA-256.
const COMPOSE_HASH_BYTES: usize = 32;

/// One entry of the event log dstack returns with a quote.
#[derive(Debug, Deserialize)]
struct EventLogEntry {
    imr: u32,
    #[serde(alias = "eventType")]
    event_type: u32,
    #[serde(default)]
    digest: String,
    #[serde(default)]
    event: String,
    #[serde(default, alias = "eventPayload")]
    event_payload: String,
}

/// What a replayed RTMR3 event log says the KMS booted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KmsAppIdentity {
    /// SHA-256 of the compose file, lowercase hex.
    pub compose_hash: String,
    /// The dstack app id, lowercase hex, when the log measures one.
    pub app_id: Option<String>,
}

/// Replay `event_log`'s RTMR3 events against the quote's `quote_rtmr3` and
/// return the compose hash and app id they measure.
///
/// Fails when the log is missing or malformed, when it does not replay to the
/// quote's register, or when it measures no compose hash, or more than one.
pub(super) fn verified_app_identity(
    event_log: Option<&serde_json::Value>,
    quote_rtmr3: &str,
) -> Result<KmsAppIdentity> {
    let Some(event_log) = event_log else {
        bail!(
            "KMS attest response carries no eventLog, so its compose hash cannot be checked. \
             Upgrade mero-kms."
        );
    };
    // dstack hands the log over as a JSON string; mero-kms parses it first.
    let entries: Vec<EventLogEntry> = match event_log {
        serde_json::Value::String(raw) => serde_json::from_str(raw),
        other => serde_json::from_value(other.clone()),
    }
    .context("KMS attest eventLog is not a dstack event log")?;
    if entries.len() > MAX_EVENT_LOG_ENTRIES {
        bail!(
            "KMS attest eventLog has {} entries (at most {MAX_EVENT_LOG_ENTRIES} accepted)",
            entries.len()
        );
    }

    let mut rtmr3 = [0u8; 48];
    let mut compose_hash = None;
    let mut app_id = None;
    for entry in entries.iter().filter(|e| e.imr == RUNTIME_EVENT_IMR) {
        let payload = hex::decode(entry.event_payload.trim())
            .with_context(|| format!("KMS RTMR3 event {:?} has a non-hex payload", entry.event))?;
        let digest = runtime_event_digest(entry.event_type, &entry.event, &payload);
        let claimed = entry.digest.trim();
        if !claimed.is_empty() && !claimed.eq_ignore_ascii_case(&hex::encode(digest)) {
            bail!(
                "KMS RTMR3 event {:?} digest does not match its contents",
                entry.event
            );
        }
        rtmr3 = Sha384::new()
            .chain_update(rtmr3)
            .chain_update(digest)
            .finalize()
            .into();

        match entry.event.as_str() {
            COMPOSE_HASH_EVENT => {
                if compose_hash.is_some() {
                    bail!("KMS RTMR3 event log measures more than one compose hash");
                }
                if payload.len() != COMPOSE_HASH_BYTES {
                    bail!(
                        "KMS compose hash is {} bytes, expected {COMPOSE_HASH_BYTES}",
                        payload.len()
                    );
                }
                compose_hash = Some(hex::encode(&payload));
            }
            APP_ID_EVENT => app_id = Some(hex::encode(&payload)),
            _ => {}
        }
    }

    let replayed = hex::encode(rtmr3);
    let quoted = normalize_attestation_measurement(quote_rtmr3);
    if replayed != quoted {
        bail!(
            "KMS event log does not replay to the quote's RTMR3 (replayed {replayed}, quote {quoted})"
        );
    }
    let Some(compose_hash) = compose_hash else {
        bail!("KMS RTMR3 event log measures no compose hash");
    };
    Ok(KmsAppIdentity {
        compose_hash,
        app_id,
    })
}

/// Refuse a KMS whose compose hash is not one `allowed` lists.
///
/// `allowed` holds normalized (lowercase, unprefixed) hex compose hashes.
pub(super) fn enforce_compose_hash_allowlist(
    identity: &KmsAppIdentity,
    allowed: &[String],
) -> Result<()> {
    if allowed.contains(&identity.compose_hash) {
        return Ok(());
    }
    bail!(
        "KMS compose hash {} is not a released one [{}]; refusing a KMS app running an \
         unreleased compose file",
        identity.compose_hash,
        allowed.join(", ")
    )
}

fn runtime_event_digest(event_type: u32, event: &str, payload: &[u8]) -> [u8; 48] {
    Sha384::new()
        .chain_update(event_type.to_le_bytes())
        .chain_update(b":")
        .chain_update(event.as_bytes())
        .chain_update(b":")
        .chain_update(payload)
        .finalize()
        .into()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// dstack's event type for runtime events.
    const DSTACK_RUNTIME_EVENT_TYPE: u32 = 0x0800_0001;

    /// An RTMR3 event log as dstack reports it, measuring `compose_hash`, and
    /// the RTMR3 it replays to.
    pub(in crate::kms) fn event_log_for(compose_hash: &str) -> (serde_json::Value, String) {
        let events = [
            ("system-preparing", String::new()),
            ("app-id", "ab".repeat(20)),
            (COMPOSE_HASH_EVENT, compose_hash.to_owned()),
            ("instance-id", "cd".repeat(20)),
            ("boot-mr-done", String::new()),
            (
                "calimero.kms.profile",
                hex::encode("calimero.kms.profile=locked-read-only"),
            ),
            ("system-ready", String::new()),
        ];
        let mut rtmr3 = [0u8; 48];
        let mut entries = vec![serde_json::json!({
            // An RTMR0 entry: not replayed into RTMR3, and its TCG digest is not
            // a runtime-event digest.
            "imr": 0,
            "event_type": 1,
            "digest": "ff".repeat(48),
            "event": "",
            "event_payload": "",
        })];
        for (event, payload) in events {
            let digest = runtime_event_digest(
                DSTACK_RUNTIME_EVENT_TYPE,
                event,
                &hex::decode(&payload).unwrap(),
            );
            rtmr3 = Sha384::new()
                .chain_update(rtmr3)
                .chain_update(digest)
                .finalize()
                .into();
            entries.push(serde_json::json!({
                "imr": 3,
                "event_type": DSTACK_RUNTIME_EVENT_TYPE,
                // dstack leaves runtime-event digests empty or fills them in;
                // the replay must accept both.
                "digest": if event == "app-id" { hex::encode(digest) } else { String::new() },
                "event": event,
                "event_payload": payload,
            }));
        }
        (serde_json::Value::Array(entries), hex::encode(rtmr3))
    }

    #[test]
    fn a_log_that_replays_to_the_quote_yields_its_compose_hash() {
        let compose = "aa".repeat(32);
        let (log, rtmr3) = event_log_for(&compose);

        let identity = verified_app_identity(Some(&log), &rtmr3).unwrap();
        assert_eq!(identity.compose_hash, compose);
        assert_eq!(identity.app_id.as_deref(), Some("ab".repeat(20).as_str()));

        // The raw string dstack returns replays the same.
        let raw = serde_json::Value::String(log.to_string());
        assert_eq!(
            verified_app_identity(Some(&raw), &rtmr3.to_uppercase()).unwrap(),
            identity
        );
    }

    #[test]
    fn a_substituted_compose_hash_does_not_replay() {
        let (mut log, rtmr3) = event_log_for(&"aa".repeat(32));
        // Swap in another compose hash but keep the register the genuine log
        // replays to: exactly what a KMS running another compose file would
        // need to present.
        log[3]["event_payload"] = serde_json::json!("bb".repeat(32));

        let err = verified_app_identity(Some(&log), &rtmr3).unwrap_err();
        assert!(err.to_string().contains("does not replay"), "{err}");
    }

    #[test]
    fn a_digest_that_disagrees_with_its_event_is_refused() {
        let (mut log, rtmr3) = event_log_for(&"aa".repeat(32));
        log[2]["event_payload"] = serde_json::json!("ef".repeat(20));

        let err = verified_app_identity(Some(&log), &rtmr3).unwrap_err();
        assert!(err.to_string().contains("digest"), "{err}");
    }

    #[test]
    fn a_missing_or_doubled_compose_hash_is_refused() {
        let err = verified_app_identity(None, &"00".repeat(48)).unwrap_err();
        assert!(err.to_string().contains("no eventLog"), "{err}");

        let (mut log, _) = event_log_for(&"aa".repeat(32));
        let entries = log.as_array_mut().unwrap();
        let compose = entries.remove(3);
        let rtmr3 = replay_only(entries);
        let err = verified_app_identity(Some(&log), &rtmr3).unwrap_err();
        assert!(err.to_string().contains("no compose hash"), "{err}");

        let entries = log.as_array_mut().unwrap();
        entries.push(compose.clone());
        entries.push(compose);
        let rtmr3 = replay_only(entries);
        let err = verified_app_identity(Some(&log), &rtmr3).unwrap_err();
        assert!(err.to_string().contains("more than one"), "{err}");
    }

    #[test]
    fn only_an_allowlisted_compose_hash_passes() {
        let identity = KmsAppIdentity {
            compose_hash: "aa".repeat(32),
            app_id: None,
        };
        enforce_compose_hash_allowlist(&identity, &["bb".repeat(32), "aa".repeat(32)]).unwrap();
        let err = enforce_compose_hash_allowlist(&identity, &["bb".repeat(32)]).unwrap_err();
        assert!(err.to_string().contains("not a released one"), "{err}");
    }

    /// The RTMR3 `entries` replay to, for logs edited after construction.
    fn replay_only(entries: &[serde_json::Value]) -> String {
        let mut rtmr3 = [0u8; 48];
        for entry in entries.iter().filter(|e| e["imr"] == 3) {
            let digest = runtime_event_digest(
                u32::try_from(entry["event_type"].as_u64().unwrap()).unwrap(),
                entry["event"].as_str().unwrap(),
                &hex::decode(entry["event_payload"].as_str().unwrap()).unwrap(),
            );
            rtmr3 = Sha384::new()
                .chain_update(rtmr3)
                .chain_update(digest)
                .finalize()
                .into();
        }
        hex::encode(rtmr3)
    }
}
