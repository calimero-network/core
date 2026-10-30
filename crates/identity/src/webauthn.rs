//! WebAuthn registration and assertion verification (ES256 and EdDSA).
//!
//! Only what a relying party needs: parse the client data and authenticator
//! data, check challenge, origin, RP id and flags, and verify the signature.
//! Attestation statements are not checked (format `none` is the norm for
//! passkeys); trust in the authenticator's make is not part of this model.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ciborium::value::Value;
use ring::digest::{digest, SHA256};
use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ED25519};
use serde::{Deserialize, Serialize};

use crate::Error;

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;

/// A credential's public key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "alg", content = "key")]
pub enum PublicKey {
    /// P-256, uncompressed SEC1 point (65 bytes), base64url.
    Es256(String),
    /// Ed25519 public key (32 bytes), base64url.
    EdDsa(String),
}

impl PublicKey {
    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), Error> {
        let (alg, key): (&dyn ring::signature::VerificationAlgorithm, &str) = match self {
            Self::Es256(k) => (&ECDSA_P256_SHA256_ASN1, k),
            Self::EdDsa(k) => (&ED25519, k),
        };
        let key = b64(key)?;
        UnparsedPublicKey::new(alg, key)
            .verify(message, signature)
            .map_err(|_| Error::BadSignature)
    }
}

/// A registered passkey.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// The credential id, base64url.
    pub id: String,
    pub public_key: PublicKey,
    pub sign_count: u32,
}

/// What the relying party expects of every ceremony.
#[derive(Clone, Debug)]
pub struct RelyingParty {
    /// The RP id (a domain), e.g. `id.calimero.network` or `localhost`.
    pub id: String,
    /// Origins allowed to run ceremonies for this RP.
    pub origins: Vec<String>,
    /// Require user verification (biometric or PIN), not only presence.
    pub require_uv: bool,
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
}

pub(crate) fn b64(s: &str) -> Result<Vec<u8>, Error> {
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|_| Error::Malformed("base64url"))
}

pub(crate) fn enc(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

struct AuthData {
    rp_id_hash: [u8; 32],
    flags: u8,
    sign_count: u32,
    credential: Option<(Vec<u8>, Value)>,
}

fn parse_auth_data(data: &[u8]) -> Result<AuthData, Error> {
    if data.len() < 37 {
        return Err(Error::Malformed("authenticator data"));
    }
    let mut rp_id_hash = [0; 32];
    rp_id_hash.copy_from_slice(&data[..32]);
    let flags = data[32];
    let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
    let mut credential = None;
    if flags & FLAG_AT != 0 {
        let rest = &data[37..];
        if rest.len() < 18 {
            return Err(Error::Malformed("attested credential data"));
        }
        let id_len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
        let id = rest
            .get(18..18 + id_len)
            .ok_or(Error::Malformed("credential id"))?
            .to_vec();
        let key_bytes = &rest[18 + id_len..];
        let key: Value =
            ciborium::de::from_reader(key_bytes).map_err(|_| Error::Malformed("COSE key"))?;
        credential = Some((id, key));
    }
    Ok(AuthData {
        rp_id_hash,
        flags,
        sign_count,
        credential,
    })
}

fn map_get(map: &[(Value, Value)], key: i64) -> Option<&Value> {
    map.iter().find_map(|(k, v)| match k {
        Value::Integer(i) if i128::from(*i) == i128::from(key) => Some(v),
        _ => None,
    })
}

fn bytes_of(v: Option<&Value>) -> Result<Vec<u8>, Error> {
    match v {
        Some(Value::Bytes(b)) => Ok(b.clone()),
        _ => Err(Error::Malformed("COSE key field")),
    }
}

fn int_of(v: Option<&Value>) -> Option<i128> {
    match v {
        Some(Value::Integer(i)) => Some(i128::from(*i)),
        _ => None,
    }
}

fn cose_public_key(key: &Value) -> Result<PublicKey, Error> {
    let Value::Map(map) = key else {
        return Err(Error::Malformed("COSE key"));
    };
    match (int_of(map_get(map, 1)), int_of(map_get(map, 3))) {
        // kty EC2, alg ES256
        (Some(2), Some(-7)) => {
            if int_of(map_get(map, -1)) != Some(1) {
                return Err(Error::Unsupported("curve"));
            }
            let x = bytes_of(map_get(map, -2))?;
            let y = bytes_of(map_get(map, -3))?;
            if x.len() != 32 || y.len() != 32 {
                return Err(Error::Malformed("P-256 point"));
            }
            let mut point = vec![4];
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            Ok(PublicKey::Es256(enc(&point)))
        }
        // kty OKP, alg EdDSA, crv Ed25519
        (Some(1), Some(-8)) => {
            if int_of(map_get(map, -1)) != Some(6) {
                return Err(Error::Unsupported("curve"));
            }
            let x = bytes_of(map_get(map, -2))?;
            if x.len() != 32 {
                return Err(Error::Malformed("Ed25519 key"));
            }
            Ok(PublicKey::EdDsa(enc(&x)))
        }
        _ => Err(Error::Unsupported("algorithm")),
    }
}

/// The origin a ceremony's client data claims, unchecked: what a relying party
/// with several kinds of origin reads to decide which to verify it against.
/// Verifying the ceremony still requires the origin to be one it allows.
#[must_use]
pub fn claimed_origin(client_data_json: &str) -> Option<String> {
    let raw = b64(client_data_json).ok()?;
    let cd: ClientData = serde_json::from_slice(&raw).ok()?;
    Some(cd.origin)
}

fn check_client_data(
    rp: &RelyingParty,
    client_data_json: &[u8],
    kind: &str,
    challenge: &[u8],
) -> Result<(), Error> {
    let cd: ClientData =
        serde_json::from_slice(client_data_json).map_err(|_| Error::Malformed("client data"))?;
    if cd.kind != kind {
        return Err(Error::Mismatch("ceremony type"));
    }
    if b64(&cd.challenge)? != challenge {
        return Err(Error::Mismatch("challenge"));
    }
    if !rp.origins.iter().any(|o| o == &cd.origin) {
        return Err(Error::Mismatch("origin"));
    }
    Ok(())
}

fn check_flags(rp: &RelyingParty, auth: &AuthData) -> Result<(), Error> {
    let expected = digest(&SHA256, rp.id.as_bytes());
    if auth.rp_id_hash != expected.as_ref() {
        return Err(Error::Mismatch("rp id"));
    }
    if auth.flags & FLAG_UP == 0 {
        return Err(Error::Mismatch("user presence"));
    }
    if rp.require_uv && auth.flags & FLAG_UV == 0 {
        return Err(Error::Mismatch("user verification"));
    }
    Ok(())
}

/// The browser's answer to `navigator.credentials.create`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    /// base64url of `response.attestationObject`.
    pub attestation_object: String,
    /// base64url of `response.clientDataJSON`.
    pub client_data_json: String,
}

/// The browser's answer to `navigator.credentials.get`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assertion {
    /// base64url of `rawId`.
    pub credential_id: String,
    pub authenticator_data: String,
    pub client_data_json: String,
    pub signature: String,
}

/// Verify a registration against the challenge the RP issued.
pub fn verify_registration(
    rp: &RelyingParty,
    challenge: &[u8],
    reg: &Registration,
) -> Result<Credential, Error> {
    let client_data = b64(&reg.client_data_json)?;
    check_client_data(rp, &client_data, "webauthn.create", challenge)?;
    let att: Value = ciborium::de::from_reader(b64(&reg.attestation_object)?.as_slice())
        .map_err(|_| Error::Malformed("attestation object"))?;
    let Value::Map(att) = att else {
        return Err(Error::Malformed("attestation object"));
    };
    let auth_data = att
        .iter()
        .find_map(|(k, v)| match (k, v) {
            (Value::Text(t), Value::Bytes(b)) if t == "authData" => Some(b.clone()),
            _ => None,
        })
        .ok_or(Error::Malformed("authData"))?;
    let auth = parse_auth_data(&auth_data)?;
    check_flags(rp, &auth)?;
    let (id, key) = auth
        .credential
        .as_ref()
        .ok_or(Error::Malformed("no attested credential"))?;
    Ok(Credential {
        id: enc(id),
        public_key: cose_public_key(key)?,
        sign_count: auth.sign_count,
    })
}

/// Verify an assertion by `credential` over `challenge`. Returns the new sign
/// count, which the caller stores.
pub fn verify_assertion(
    rp: &RelyingParty,
    challenge: &[u8],
    credential: &Credential,
    assertion: &Assertion,
) -> Result<u32, Error> {
    if assertion.credential_id != credential.id {
        return Err(Error::Mismatch("credential"));
    }
    let client_data = b64(&assertion.client_data_json)?;
    check_client_data(rp, &client_data, "webauthn.get", challenge)?;
    let auth_bytes = b64(&assertion.authenticator_data)?;
    let auth = parse_auth_data(&auth_bytes)?;
    check_flags(rp, &auth)?;
    // A counter that does not advance signals a cloned authenticator, unless
    // both are zero (synced passkeys do not count).
    if (auth.sign_count != 0 || credential.sign_count != 0)
        && auth.sign_count <= credential.sign_count
    {
        return Err(Error::Mismatch("signature counter"));
    }
    let mut message = auth_bytes;
    message.extend_from_slice(digest(&SHA256, &client_data).as_ref());
    credential
        .public_key
        .verify(&message, &b64(&assertion.signature)?)?;
    Ok(auth.sign_count)
}

/// What a replica checks of a logged consent, from the system state alone:
/// `assertion` is `credential`'s signature over `challenge`, made with the
/// user present and verified. Every node reaches the same answer, so it can
/// run where the command applies.
///
/// The origin, the RP id and the signature counter are left to the node that
/// took the assertion ([`verify_assertion`]): origins and the RP id are each
/// node's configuration, not replicated state; the credential is bound to its
/// RP id by the authenticator; and a replayed consent is refused by the
/// command, whose challenge names the state it changes.
pub fn verify_consent(
    challenge: &[u8],
    credential: &Credential,
    assertion: &Assertion,
) -> Result<(), Error> {
    if assertion.credential_id != credential.id {
        return Err(Error::Mismatch("credential"));
    }
    let client_data = b64(&assertion.client_data_json)?;
    let cd: ClientData =
        serde_json::from_slice(&client_data).map_err(|_| Error::Malformed("client data"))?;
    if cd.kind != "webauthn.get" {
        return Err(Error::Mismatch("ceremony type"));
    }
    if b64(&cd.challenge)? != challenge {
        return Err(Error::Mismatch("challenge"));
    }
    let auth_bytes = b64(&assertion.authenticator_data)?;
    let auth = parse_auth_data(&auth_bytes)?;
    if auth.flags & FLAG_UP == 0 {
        return Err(Error::Mismatch("user presence"));
    }
    if auth.flags & FLAG_UV == 0 {
        return Err(Error::Mismatch("user verification"));
    }
    let mut message = auth_bytes;
    message.extend_from_slice(digest(&SHA256, &client_data).as_ref());
    credential
        .public_key
        .verify(&message, &b64(&assertion.signature)?)
}
