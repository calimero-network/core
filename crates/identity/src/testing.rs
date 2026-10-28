//! A software passkey authenticator (ES256), for tests.

use ciborium::value::Value;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};

use crate::webauthn::{Assertion, Registration};
use crate::{b64url, random, sha256};

/// One passkey held in memory.
pub struct SoftAuthenticator {
    pkcs8: Vec<u8>,
    credential_id: Vec<u8>,
    rp_id: String,
    origin: String,
    counter: u32,
}

impl SoftAuthenticator {
    /// A new passkey for `rp_id`, used from `origin`.
    #[must_use]
    pub fn new(rp_id: &str, origin: &str) -> Self {
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .expect("key generation")
                .as_ref()
                .to_vec();
        Self {
            pkcs8,
            credential_id: random::<16>().to_vec(),
            rp_id: rp_id.to_owned(),
            origin: origin.to_owned(),
            counter: 0,
        }
    }

    /// The same key used from another origin or RP (to test refusals).
    #[must_use]
    pub fn with_key(other: &Self, rp_id: &str, origin: &str) -> Self {
        Self {
            pkcs8: other.pkcs8.clone(),
            credential_id: other.credential_id.clone(),
            rp_id: rp_id.to_owned(),
            origin: origin.to_owned(),
            counter: other.counter + 100,
        }
    }

    /// Save this passkey (private key included) as JSON. A file passkey is for
    /// development clusters only: whoever reads the file holds the account.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "pkcs8": b64url(&self.pkcs8),
            "credentialId": b64url(&self.credential_id),
            "rpId": self.rp_id,
            "origin": self.origin,
            "counter": self.counter,
        })
        .to_string()
    }

    /// Load a passkey saved with [`Self::to_json`].
    pub fn from_json(json: &str) -> Result<Self, crate::Error> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|_| crate::Error::Malformed("passkey file"))?;
        let field = |k: &str| {
            v[k].as_str()
                .ok_or(crate::Error::Malformed("passkey file field"))
        };
        let pkcs8 = crate::from_b64url(field("pkcs8")?)?;
        // Refuse a file whose key does not load now, not at the first signature.
        let _check = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|_| crate::Error::Malformed("passkey key"))?;
        Ok(Self {
            pkcs8,
            credential_id: crate::from_b64url(field("credentialId")?)?,
            rp_id: field("rpId")?.to_owned(),
            origin: field("origin")?.to_owned(),
            counter: v["counter"]
                .as_u64()
                .and_then(|c| u32::try_from(c).ok())
                .unwrap_or(0),
        })
    }

    fn pair(&self) -> EcdsaKeyPair {
        EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &self.pkcs8,
            &SystemRandom::new(),
        )
        .expect("valid key")
    }

    #[must_use]
    pub fn credential_id(&self) -> String {
        b64url(&self.credential_id)
    }

    fn client_data(&self, kind: &str, challenge: &[u8]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "type": kind,
            "challenge": b64url(challenge),
            "origin": self.origin,
            "crossOrigin": false,
        }))
        .expect("json")
    }

    fn auth_data(&self, flags: u8, attested: Option<Vec<u8>>) -> Vec<u8> {
        let mut out = sha256(self.rp_id.as_bytes()).to_vec();
        out.push(flags);
        out.extend_from_slice(&self.counter.to_be_bytes());
        if let Some(a) = attested {
            out.extend_from_slice(&a);
        }
        out
    }

    /// Answer `navigator.credentials.create` with attestation format `none`.
    pub fn register(&mut self, challenge: &[u8]) -> Registration {
        let public = self.pair().public_key().as_ref().to_vec(); // 0x04 || x || y
        let cose = Value::Map(vec![
            (Value::Integer(1.into()), Value::Integer(2.into())),
            (Value::Integer(3.into()), Value::Integer((-7).into())),
            (Value::Integer((-1).into()), Value::Integer(1.into())),
            (
                Value::Integer((-2).into()),
                Value::Bytes(public[1..33].to_vec()),
            ),
            (
                Value::Integer((-3).into()),
                Value::Bytes(public[33..65].to_vec()),
            ),
        ]);
        let mut cose_bytes = Vec::new();
        ciborium::ser::into_writer(&cose, &mut cose_bytes).expect("cbor");
        let mut attested = vec![0_u8; 16]; // AAGUID
        attested.extend_from_slice(
            &u16::try_from(self.credential_id.len())
                .expect("short id")
                .to_be_bytes(),
        );
        attested.extend_from_slice(&self.credential_id);
        attested.extend_from_slice(&cose_bytes);
        let auth_data = self.auth_data(0x01 | 0x04 | 0x40, Some(attested));
        let att = Value::Map(vec![
            (Value::Text("fmt".into()), Value::Text("none".into())),
            (Value::Text("attStmt".into()), Value::Map(vec![])),
            (Value::Text("authData".into()), Value::Bytes(auth_data)),
        ]);
        let mut att_bytes = Vec::new();
        ciborium::ser::into_writer(&att, &mut att_bytes).expect("cbor");
        Registration {
            attestation_object: b64url(&att_bytes),
            client_data_json: b64url(&self.client_data("webauthn.create", challenge)),
        }
    }

    /// Answer `navigator.credentials.get`.
    pub fn assert(&mut self, challenge: &[u8]) -> Assertion {
        self.counter += 1;
        let client_data = self.client_data("webauthn.get", challenge);
        let auth_data = self.auth_data(0x01 | 0x04, None);
        let mut message = auth_data.clone();
        message.extend_from_slice(&sha256(&client_data));
        let sig = self
            .pair()
            .sign(&SystemRandom::new(), &message)
            .expect("sign");
        Assertion {
            credential_id: self.credential_id(),
            authenticator_data: b64url(&auth_data),
            client_data_json: b64url(&client_data),
            signature: b64url(sig.as_ref()),
        }
    }
}
