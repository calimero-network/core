//! The node's rollback counters in mero-kms (`docs/specs/rollback-protection.md`).
//!
//! A v2 key release names the node (`rollbackNodeId`) and answers, sealed to
//! the TD with the key, the cluster's epoch, the node's entry `(generation,
//! mark)` and a session: an id and a key only this TD and mero-kms hold. The
//! node then raises its entry (`/rollback/raise`) or forces it down
//! (`/rollback/reset`) with HMACs under the session key, and each answer is
//! HMACed over the node's fresh nonce, so a relay can neither move the counter
//! nor fake an acknowledgement.
//!
//! ```text
//! request MAC = HMAC(session key, domain ‖ peerId ‖ "\n" ‖ nodeId ‖ sessionId
//!                                 ‖ generation ‖ mark ‖ nonce)        (u64s 8 BE)
//! answer MAC  = HMAC(session key, "mero-kms/rollback-ack/v1" ‖ nonce ‖ epoch
//!                                 ‖ generation ‖ mark)
//! ```

use base64::Engine;
use eyre::{bail, Context, Result};
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;
use ring::hmac;
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

const RAISE_DOMAIN: &[u8] = b"mero-kms/rollback-raise/v1";
const RESET_DOMAIN: &[u8] = b"mero-kms/rollback-reset/v1";
const ACK_DOMAIN: &[u8] = b"mero-kms/rollback-ack/v1";

/// A node's entry: a pair, so a forced rollback (a new generation) is larger
/// than any mark of the generation before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CounterEntry {
    pub generation: u64,
    pub mark: u64,
}

/// mero-kms's answer to a raise or reset: the cluster epoch and the entry it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CounterAck {
    pub epoch: u64,
    pub entry: CounterEntry,
}

/// What a v2 release seals besides the key (the record's JSON).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReleaseRecord {
    pub(super) key_hex: String,
    pub(super) epoch: u64,
    pub(super) generation: u64,
    #[serde(default)]
    pub(super) mark: Option<u64>,
    pub(super) session_id_b64: String,
    pub(super) session_key_hex: String,
}

impl Drop for ReleaseRecord {
    fn drop(&mut self) {
        self.key_hex.zeroize();
        self.session_key_hex.zeroize();
    }
}

/// This node's counters as the key release gave them, and the session that
/// moves them. No `Debug` of the session key.
pub struct Counters {
    /// The cluster epoch: rollbacks forced so far.
    pub epoch: u64,
    /// This node's entry; `None` when mero-kms has none for it.
    pub entry: Option<CounterEntry>,
    session: Session,
}

impl std::fmt::Debug for Counters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Counters")
            .field("epoch", &self.epoch)
            .field("entry", &self.entry)
            .field("node", &self.session.node_id)
            .finish_non_exhaustive()
    }
}

struct Session {
    client: reqwest::Client,
    base_url: Url,
    peer_id: String,
    node_id: u64,
    id: [u8; 16],
    key: Zeroizing<Vec<u8>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RollbackRequest<'a> {
    peer_id: &'a str,
    node_id: u64,
    session_id_b64: String,
    generation: u64,
    mark: u64,
    nonce_b64: String,
    mac_b64: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RollbackAck {
    epoch: u64,
    generation: u64,
    mark: u64,
    mac_b64: String,
}

impl Counters {
    pub(super) fn from_record(
        record: &ReleaseRecord,
        client: reqwest::Client,
        base_url: Url,
        peer_id: &str,
        node_id: u64,
    ) -> Result<Self> {
        let engine = &base64::engine::general_purpose::STANDARD;
        let id: [u8; 16] = engine
            .decode(&record.session_id_b64)
            .context("the release's sessionIdB64 is not base64")?
            .try_into()
            .map_err(|_| eyre::eyre!("the release's session id is not 16 bytes"))?;
        let key = Zeroizing::new(
            hex::decode(record.session_key_hex.as_str())
                .context("the release's session key is not hex")?,
        );
        if key.len() != 32 {
            bail!("the release's session key is not 32 bytes");
        }
        Ok(Self {
            epoch: record.epoch,
            entry: record.mark.map(|mark| CounterEntry {
                generation: record.generation,
                mark,
            }),
            session: Session {
                client,
                base_url,
                peer_id: peer_id.to_owned(),
                node_id,
                id,
                key,
            },
        })
    }

    /// Raise this node's entry to `to`; mero-kms never lowers it.
    pub async fn raise(&self, to: CounterEntry) -> Result<CounterAck> {
        self.session.call("rollback/raise", RAISE_DOMAIN, to).await
    }

    /// Force this node's entry to `mark` in a new generation; mero-kms raises
    /// the cluster epoch (`--accept-rollback`).
    pub async fn reset(&self, mark: u64) -> Result<CounterAck> {
        let to = CounterEntry {
            generation: self.entry.map_or(self.epoch, |e| e.generation),
            mark,
        };
        self.session.call("rollback/reset", RESET_DOMAIN, to).await
    }
}

impl Session {
    async fn call(&self, path: &str, domain: &[u8], to: CounterEntry) -> Result<CounterAck> {
        let mut nonce = [0u8; 32];
        UnwrapErr(SysRng).fill_bytes(&mut nonce);
        let key = hmac::Key::new(hmac::HMAC_SHA256, &self.key);
        let message = request_message(
            domain,
            &self.peer_id,
            self.node_id,
            &self.id,
            to.generation,
            to.mark,
            &nonce,
        );
        let engine = &base64::engine::general_purpose::STANDARD;
        let request = RollbackRequest {
            peer_id: &self.peer_id,
            node_id: self.node_id,
            session_id_b64: engine.encode(self.id),
            generation: to.generation,
            mark: to.mark,
            nonce_b64: engine.encode(nonce),
            mac_b64: engine.encode(hmac::sign(&key, &message)),
        };
        let url = self
            .base_url
            .join(path)
            .context("cannot build the KMS rollback URL")?;
        let response = self
            .client
            .post(url.as_str())
            .json(&request)
            .send()
            .await
            .with_context(|| format!("KMS {path} failed"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "KMS {path} failed ({status}): {}",
                body.chars().take(512).collect::<String>()
            );
        }
        let ack: RollbackAck = response
            .json()
            .await
            .with_context(|| format!("cannot parse the KMS {path} answer"))?;
        let mac = engine
            .decode(&ack.mac_b64)
            .context("the KMS answer's MAC is not base64")?;
        hmac::verify(
            &key,
            &ack_message(&nonce, ack.epoch, ack.generation, ack.mark),
            &mac,
        )
        .map_err(|_| {
            eyre::eyre!(
                "the KMS {path} answer is not MACed under this node's session: not mero-kms's"
            )
        })?;
        Ok(CounterAck {
            epoch: ack.epoch,
            entry: CounterEntry {
                generation: ack.generation,
                mark: ack.mark,
            },
        })
    }
}

fn request_message(
    domain: &[u8],
    peer_id: &str,
    node_id: u64,
    session_id: &[u8; 16],
    generation: u64,
    mark: u64,
    nonce: &[u8; 32],
) -> Vec<u8> {
    let mut message = Vec::with_capacity(domain.len() + peer_id.len() + 73);
    message.extend_from_slice(domain);
    message.extend_from_slice(peer_id.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(&node_id.to_be_bytes());
    message.extend_from_slice(session_id);
    message.extend_from_slice(&generation.to_be_bytes());
    message.extend_from_slice(&mark.to_be_bytes());
    message.extend_from_slice(nonce);
    message
}

fn ack_message(nonce: &[u8; 32], epoch: u64, generation: u64, mark: u64) -> Vec<u8> {
    let mut message = ACK_DOMAIN.to_vec();
    message.extend_from_slice(nonce);
    message.extend_from_slice(&epoch.to_be_bytes());
    message.extend_from_slice(&generation.to_be_bytes());
    message.extend_from_slice(&mark.to_be_bytes());
    message
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// The MAC vectors, repeated verbatim in mero-kms's tests.
    #[test]
    fn the_rollback_macs_match_the_published_vectors() {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &[0x66; 32]);
        let mac = |domain| {
            hex::encode(hmac::sign(
                &key,
                &request_message(domain, "12D3KooWPeer", 3, &[0x55; 16], 1, 42, &[0x77; 32]),
            ))
        };
        assert_eq!(
            mac(RAISE_DOMAIN),
            "171f21d4c094f60435bdc07f917f22340fb69f21cf5bc6afc055276f6c9d8107"
        );
        assert_eq!(
            mac(RESET_DOMAIN),
            "bc9633cc88afc2a7234b76a84257f2f1fb2a65b8b8dd934907dfeab90bb2fc03"
        );
        assert_eq!(
            hex::encode(hmac::sign(&key, &ack_message(&[0x77; 32], 1, 1, 42))),
            "7000a4d3e234b832028ec7d8a9f17a6d93d6389e3564575b15787718ce1cfed8"
        );
    }

    /// What the tests' stand-in KMS answers a raise or reset with.
    #[cfg(feature = "mock-attestation")]
    pub(in crate::kms) fn ack_for_test(
        session_key: &[u8],
        nonce: &[u8; 32],
        epoch: u64,
        generation: u64,
        mark: u64,
    ) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, session_key);
        base64::engine::general_purpose::STANDARD.encode(hmac::sign(
            &key,
            &ack_message(nonce, epoch, generation, mark),
        ))
    }
}
