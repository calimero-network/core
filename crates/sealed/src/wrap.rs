//! Wrapping a 32-byte secret: the AES-GCM helpers every wrap shares, and a file's data key
//! wrapped to an X25519 key its requester holds.

use curve25519_dalek::montgomery::MontgomeryPoint;
use eyre::{bail, eyre, Result as EyreResult};
use ring::aead::{Aad, LessSafeKey, Nonce, AES_256_GCM, NONCE_LEN};
use zeroize::Zeroizing;

use crate::file::cipher;
use crate::server::derive_shared;

const EXPORT_WRAP_SALT: &[u8] = b"calimero.export-wrap.v1"; // HKDF salt of an export wrap

/// What an export wrap is bound to: the app and the file.
#[derive(Clone, Copy)]
pub struct ExportWrap<'a> {
    pub app_id: &'a str,
    pub file_id: &'a str,
}

impl ExportWrap<'_> {
    /// The HKDF info: both public keys and the length-prefixed ids.
    fn info(&self, ephemeral_public: &[u8; 32], recipient: &[u8; 32]) -> EyreResult<Vec<u8>> {
        let mut info = [ephemeral_public.as_slice(), recipient].concat();
        push_prefixed(&mut info, self.app_id)?;
        push_prefixed(&mut info, self.file_id)?;
        Ok(info)
    }
}

pub fn push_prefixed(info: &mut Vec<u8>, field: &str) -> EyreResult<()> {
    let len = u32::try_from(field.len()).map_err(|_| eyre!("an id is too long"))?;
    info.extend_from_slice(&len.to_be_bytes());
    info.extend_from_slice(field.as_bytes());
    Ok(())
}

/// `nonce || AES-256-GCM(key, secret)`.
pub fn seal(
    key: &LessSafeKey,
    nonce: [u8; NONCE_LEN],
    aad: &[u8],
    secret: &[u8; 32],
) -> EyreResult<Vec<u8>> {
    let mut sealed = Zeroizing::new(Vec::with_capacity(secret.len() + AES_256_GCM.tag_len()));
    sealed.extend_from_slice(secret);
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad),
        &mut *sealed,
    )
    .map_err(|_| eyre!("cannot wrap the key"))?;
    Ok([nonce.as_slice(), &sealed].concat())
}

/// Open what [`seal`] wrote.
pub fn open(key: &LessSafeKey, aad: &[u8], bytes: &[u8]) -> EyreResult<Zeroizing<[u8; 32]>> {
    if bytes.len() < NONCE_LEN {
        bail!("the wrapped key is truncated");
    }
    let (nonce, sealed) = bytes.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| eyre!("bad nonce"))?;
    let mut sealed = Zeroizing::new(sealed.to_vec());
    let plain = key
        .open_in_place(nonce, Aad::from(aad), &mut sealed)
        .map_err(|_| eyre!("the wrapped key does not open"))?;
    let key: [u8; 32] = (&*plain)
        .try_into()
        .map_err(|_| eyre!("the wrapped key has the wrong length"))?;
    Ok(Zeroizing::new(key))
}

/// A fresh X25519 key pair for an export: the secret stays with the requester.
#[must_use]
pub fn export_keypair() -> (Zeroizing<[u8; 32]>, [u8; 32]) {
    let secret = Zeroizing::new(calimero_identity::random());
    let public = MontgomeryPoint::mul_base_clamped(*secret).0;
    (secret, public)
}

/// A fresh ephemeral X25519 key and `nonce || AES-256-GCM(HKDF(X25519(ephemeral, recipient)), secret)`,
/// bound to both public keys and to `wrap`; the first is the public half to send with the second.
pub fn wrap_export(
    recipient: &[u8; 32],
    wrap: &ExportWrap,
    secret: &[u8; 32],
) -> EyreResult<([u8; 32], Vec<u8>)> {
    let (ephemeral, ephemeral_public) = export_keypair();
    let shared = Zeroizing::new(MontgomeryPoint(*recipient).mul_clamped(*ephemeral).0);
    let key = derive_shared(
        &shared,
        EXPORT_WRAP_SALT,
        &wrap.info(&ephemeral_public, recipient)?,
    )?;
    let sealed = seal(&cipher(&key), calimero_identity::random(), &[], secret)?;
    Ok((ephemeral_public, sealed))
}

/// Open what [`wrap_export`] wrote to the key pair that `recipient_secret` belongs to.
pub fn unwrap_export(
    recipient_secret: &[u8; 32],
    ephemeral: &[u8; 32],
    wrap: &ExportWrap,
    wrapped: &[u8],
) -> EyreResult<Zeroizing<[u8; 32]>> {
    let public = MontgomeryPoint::mul_base_clamped(*recipient_secret).0;
    let shared = Zeroizing::new(MontgomeryPoint(*ephemeral).mul_clamped(*recipient_secret).0);
    let key = derive_shared(&shared, EXPORT_WRAP_SALT, &wrap.info(ephemeral, &public)?)?;
    open(&cipher(&key), &[], wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::LowOrder;

    const DATA_KEY: [u8; 32] = [42; 32];
    const WRAP: ExportWrap = ExportWrap {
        app_id: "app",
        file_id: "file",
    };

    fn opens(secret: &[u8; 32], ephemeral: &[u8; 32], wrap: &ExportWrap, wrapped: &[u8]) -> bool {
        unwrap_export(secret, ephemeral, wrap, wrapped).is_ok()
    }

    #[test]
    fn an_export_wrap_opens_with_the_recipients_key() {
        let (secret, public) = export_keypair();
        let (ephemeral, wrapped) = wrap_export(&public, &WRAP, &DATA_KEY).unwrap();
        assert_eq!(
            *unwrap_export(&secret, &ephemeral, &WRAP, &wrapped).unwrap(),
            DATA_KEY
        );
    }

    #[test]
    fn an_export_wrap_does_not_open_for_another_key_app_or_file() {
        let (secret, public) = export_keypair();
        let (ephemeral, wrapped) = wrap_export(&public, &WRAP, &DATA_KEY).unwrap();
        assert!(opens(&secret, &ephemeral, &WRAP, &wrapped));
        assert!(!opens(&export_keypair().0, &ephemeral, &WRAP, &wrapped));
        let other_app = ExportWrap {
            app_id: "other",
            ..WRAP
        };
        assert!(!opens(&secret, &ephemeral, &other_app, &wrapped));
        let other_file = ExportWrap {
            file_id: "other",
            ..WRAP
        };
        assert!(!opens(&secret, &ephemeral, &other_file, &wrapped));
    }

    #[test]
    fn a_tampered_or_short_export_wrap_does_not_open() {
        let (secret, public) = export_keypair();
        let (ephemeral, mut wrapped) = wrap_export(&public, &WRAP, &DATA_KEY).unwrap();
        assert!(!opens(&secret, &ephemeral, &WRAP, &wrapped[..5]));
        assert!(!opens(
            &secret,
            &ephemeral,
            &WRAP,
            &wrapped[..NONCE_LEN + 5]
        ));
        *wrapped.last_mut().unwrap() ^= 1;
        assert!(!opens(&secret, &ephemeral, &WRAP, &wrapped));
        let (ephemeral, wrapped) = wrap_export(&public, &WRAP, &DATA_KEY).unwrap();
        let mut moved = ephemeral;
        moved[31] ^= 0x80; // X25519 ignores bit 255, so only the HKDF info can notice it
        assert!(!opens(&secret, &moved, &WRAP, &wrapped));
    }

    #[test]
    fn wrapping_twice_gives_different_bytes() {
        let (_, public) = export_keypair();
        assert_ne!(
            wrap_export(&public, &WRAP, &DATA_KEY).unwrap(),
            wrap_export(&public, &WRAP, &DATA_KEY).unwrap()
        );
    }

    #[test]
    fn ids_that_differ_only_in_where_they_split_get_different_keys() {
        let (secret, public) = export_keypair();
        let split = ExportWrap {
            app_id: "a\0b",
            file_id: "c",
        };
        let moved = ExportWrap {
            app_id: "a",
            file_id: "b\0c",
        };
        let (ephemeral, wrapped) = wrap_export(&public, &split, &DATA_KEY).unwrap();
        assert!(!opens(&secret, &ephemeral, &moved, &wrapped));
        assert!(opens(&secret, &ephemeral, &split, &wrapped));
    }

    #[test]
    fn a_low_order_recipient_is_refused() {
        let mut one = [0; 32];
        one[0] = 1;
        let order_8: [u8; 32] =
            hex::decode("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800")
                .unwrap()
                .try_into()
                .unwrap();
        for point in [[0; 32], one, order_8] {
            let err = wrap_export(&point, &WRAP, &DATA_KEY).unwrap_err();
            assert!(err.downcast_ref::<LowOrder>().is_some(), "{err}");
        }
    }
}
