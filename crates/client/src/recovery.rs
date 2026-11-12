//! The recovery disk's files as their holder reads them: a node writes them
//! (`calimero_node::recovery_disk`, `merod --recovery-dir`), sealed to the
//! X25519 public key of whoever upgrades the app, and only that key's private
//! half opens them. This is the half a holder's device needs: making a key
//! pair, checking a file's header (the node's signature, its attestation) and
//! opening the file's data key (HPKE, RFC 9180: base mode,
//! DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM).
//!
//! mero-js and calimero-client-py hold the same check; change all or none.

use eyre::Result as EyreResult;
use hpke::aead::AesGcm256;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, Serializable};
use ring::digest::{Context, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

/// Who holds the key that opens an app's recovery-disk backups: the party
/// that upgrades it (the node's `SystemState::backup_holder`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum BackupHolder {
    /// The operator, where governance gives it app upgrades: every app and
    /// the whole system group.
    Operator,
    /// An organization: its apps and its slice of the system state, the key
    /// set by its org-wide `MANAGE_APPLICATION` holders.
    Org(String),
    /// An account: its personal apps.
    Account(String),
}

impl BackupHolder {
    /// Its key in the node's `backup_keys`, and the subject its consent
    /// names: `operator`, `org:<id>`, `account:<id>`.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Operator => "operator".to_owned(),
            Self::Org(org) => format!("org:{org}"),
            Self::Account(account) => format!("account:{account}"),
        }
    }
}

/// The first line of every recovery file.
pub const MAGIC: &[u8] = b"calimero.recovery-backup.v1\n";
/// The format this release writes and the newest it reads.
pub const FORMAT: u32 = 1;
/// What a recovery file's name ends with.
pub const SUFFIX: &str = ".rbak";
/// What the node's signature covers before the header bytes.
pub const SIGNING_LABEL: &[u8] = b"calimero.recovery-backup.v1\0";
/// HPKE's `info` for the data key.
pub const HPKE_INFO: &[u8] = b"calimero.recovery-backup.v1";
/// HPKE's associated data for the data key.
pub const HPKE_AAD: &[u8] = b"calimero.recovery-backup.key.v1";
/// The largest header read: an attestation (quote and collateral) included.
pub const MAX_HEADER_BYTES: usize = 1024 * 1024;
type Kem = X25519HkdfSha256;

/// What a recovery file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// One app's state.
    App,
    /// The holder's slice of the system state (`system::HolderSlice`).
    Holder,
    /// The whole system state (the operator's).
    System,
    /// One bundle or release `.mpk`.
    Code,
}

impl Kind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Holder => "holder",
            Self::System => "system",
            Self::Code => "code",
        }
    }
}

/// A recovery file's header: what it holds, for whom, and who wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    pub format: u32,
    pub kind: Kind,
    pub holder: BackupHolder,
    /// The cluster's name (`--cluster`).
    pub cluster: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// For `code`, the bundle's hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The group the state came from (0 for `holder` and `code`).
    pub group: u64,
    /// The log index the state stands at (0 for `code`).
    pub index: u64,
    /// Unix seconds.
    pub written_at: u64,
    /// The `merod` and mero-tee release that wrote it, and the node.
    pub merod: String,
    pub release: String,
    pub node: u64,
    /// The holder's X25519 public key the data key is sealed to (hex).
    pub recipient: String,
    /// HPKE's encapsulated key (hex).
    pub enc: String,
    /// The body's data key, HPKE-sealed (hex).
    pub wrapped_key: String,
    /// Plaintext bytes and SHA-256 (hex).
    pub size: u64,
    pub sha256: String,
    /// Ciphertext bytes and SHA-256 (hex): what follows the signature.
    pub body_bytes: u64,
    pub body_sha256: String,
    /// The node's receipt key (Ed25519, hex), which signs the header.
    pub signing_key: String,
    /// That key's attestation (`GET /attestation?purpose=receipts`).
    pub attestation: Value,
}

/// The nonce a node's recovery attestation binds: fixed per cluster and
/// node, so a reader recomputes it; the quote proves the key is held by
/// attested hardware, and the file's own signature what it wrote.
#[must_use]
pub fn attestation_nonce(cluster: &str, node: u64) -> [u8; 32] {
    let mut h = Context::new(&SHA256);
    h.update(b"calimero.recovery-backup.attest.v1\0");
    h.update(
        &u32::try_from(cluster.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    h.update(cluster.as_bytes());
    h.update(&node.to_be_bytes());
    let mut out = [0; 32];
    out.copy_from_slice(h.finish().as_ref());
    out
}

/// Ring's random source, for HPKE's encapsulation.
struct RingRng(SystemRandom);

impl hpke::rand_core::RngCore for RingRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        // SAFETY: the OS random source failing is not something to go on from.
        self.0.fill(dest).expect("the system random source failed");
    }
}

impl hpke::rand_core::CryptoRng for RingRng {}

/// Seal `data_key` to `recipient`: `(enc, wrapped)`; what a node does.
pub fn seal_key(recipient: &[u8; 32], data_key: &[u8; 32]) -> EyreResult<(Vec<u8>, Vec<u8>)> {
    let pk = <Kem as hpke::Kem>::PublicKey::from_bytes(recipient)
        .map_err(|e| eyre::eyre!("not an X25519 public key: {e:?}"))?;
    let (enc, wrapped) = hpke::single_shot_seal::<AesGcm256, HkdfSha256, Kem, _>(
        &OpModeS::Base,
        &pk,
        HPKE_INFO,
        data_key,
        HPKE_AAD,
        &mut RingRng(SystemRandom::new()),
    )
    .map_err(|e| eyre::eyre!("HPKE seal: {e:?}"))?;
    Ok((enc.to_bytes().to_vec(), wrapped))
}

/// Open a file's data key with the holder's X25519 private key: what the
/// holder's own device does, never a node.
pub fn open_key(header: &Header, secret: &[u8; 32]) -> EyreResult<Zeroizing<[u8; 32]>> {
    let sk = <Kem as hpke::Kem>::PrivateKey::from_bytes(secret)
        .map_err(|e| eyre::eyre!("not an X25519 private key: {e:?}"))?;
    let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(&hex::decode(&header.enc)?)
        .map_err(|e| eyre::eyre!("a malformed encapsulated key: {e:?}"))?;
    let plain = Zeroizing::new(
        hpke::single_shot_open::<AesGcm256, HkdfSha256, Kem>(
            &OpModeR::Base,
            &sk,
            &enc,
            HPKE_INFO,
            &hex::decode(&header.wrapped_key)?,
            HPKE_AAD,
        )
        .map_err(|_| eyre::eyre!("this key does not open the file"))?,
    );
    let key: [u8; 32] = plain
        .as_slice()
        .try_into()
        .map_err(|_| eyre::eyre!("the data key is not 32 bytes"))?;
    Ok(Zeroizing::new(key))
}

/// A new X25519 key pair for a holder: `(secret, public)`. Made where the
/// holder keeps it (meroctl, the console), never on a node.
#[must_use]
pub fn new_keypair() -> (Zeroizing<[u8; 32]>, [u8; 32]) {
    let (sk, pk) = Kem::gen_keypair(&mut RingRng(SystemRandom::new()));
    let mut secret = Zeroizing::new([0; 32]);
    secret.copy_from_slice(&sk.to_bytes());
    let mut public = [0; 32];
    public.copy_from_slice(&pk.to_bytes());
    (secret, public)
}

/// The public key of a holder's secret.
pub fn public_of(secret: &[u8; 32]) -> EyreResult<[u8; 32]> {
    let sk = <Kem as hpke::Kem>::PrivateKey::from_bytes(secret)
        .map_err(|e| eyre::eyre!("not an X25519 private key: {e:?}"))?;
    let mut public = [0; 32];
    public.copy_from_slice(&Kem::sk_to_pk(&sk).to_bytes());
    Ok(public)
}

/// What the node's signature covers: [`SIGNING_LABEL`], then the header bytes.
#[must_use]
pub fn signing_message(header_bytes: &[u8]) -> Vec<u8> {
    let mut m = SIGNING_LABEL.to_vec();
    m.extend_from_slice(header_bytes);
    m
}

/// A header read from its bytes and checked against the signature its own
/// key made. Whether that key belongs to attested hardware is
/// [`verify_attestation`]'s to say.
pub fn verify_header(header_bytes: &[u8], signature: &[u8; 64]) -> EyreResult<Header> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    // The format before the rest: a newer one may hold fields this one refuses.
    let format = serde_json::from_slice::<Value>(header_bytes)?
        .get("format")
        .and_then(Value::as_u64)
        .ok_or_else(|| eyre::eyre!("a recovery file's header names no format"))?;
    if format > u64::from(FORMAT) {
        eyre::bail!("recovery file format {format} is newer than this release reads ({FORMAT})");
    }
    let header: Header = serde_json::from_slice(header_bytes)?;
    let key: [u8; 32] = hex::decode(&header.signing_key)?
        .try_into()
        .map_err(|_| eyre::eyre!("the signing key is not 32 bytes"))?;
    VerifyingKey::from_bytes(&key)?
        .verify(
            &signing_message(header_bytes),
            &Signature::from_bytes(signature),
        )
        .map_err(|_| eyre::eyre!("the node's signature over the header does not hold"))?;
    Ok(header)
}

/// Check that the key that signed `header` is a node's receipt key, attested
/// under `policy` (`purpose=receipts`, nonce [`attestation_nonce`] of the
/// header's cluster and node): attested hardware wrote the file. Answers the
/// governance that attestation binds.
pub async fn verify_attestation(
    header: &Header,
    policy: &crate::attest::Policy,
) -> EyreResult<crate::attest::Governance> {
    let doc: crate::attest::AttestationDoc = serde_json::from_value(header.attestation.clone())
        .map_err(|e| eyre::eyre!("the file carries no attestation: {e}"))?;
    if doc.cluster != header.cluster || doc.node != header.node {
        eyre::bail!("the attestation is another node's");
    }
    let nonce = attestation_nonce(&header.cluster, header.node);
    let (key, governance) = crate::attest::verify_receipt_key(&doc, &nonce, policy).await?;
    if hex::encode(key) != header.signing_key {
        eyre::bail!("the attested key is not the one that signed the file");
    }
    Ok(governance)
}
