//! A file's data key, wrapped under the app's file key (`backup::file_key`), and
//! re-wrapped to the X25519 transport key of the job node that reads it.

use calimero_sealed::{derive_shared, SealedTransport};
use curve25519_dalek::montgomery::MontgomeryPoint;
use eyre::{bail, eyre, Result as EyreResult};
use ring::aead::{Aad, LessSafeKey, Nonce, AES_256_GCM, NONCE_LEN};
use zeroize::Zeroizing;

use super::seal::cipher;
use crate::backup::tag;

const FILE_WRAP_LABEL: &str = "calimero.file-wrap.v1"; // HMAC label of a wrap subkey
const JOB_WRAP_SALT: &[u8] = b"calimero.job-wrap.v1"; // HKDF salt of a job wrap
const PUBLIC_LEN: usize = 32; // an X25519 public key
const SALT_LEN: usize = 16; // the per-wrap salt of a file-key wrap
const DERIVED_EPHEMERAL_LABEL: &[u8] = b"calimero.job-wrap.ephemeral.v1|"; // HMAC label of a derived wrap's ephemeral key
const DERIVED_NONCE_LABEL: &[u8] = b"calimero.job-wrap.nonce.v1|"; // HMAC label of a derived wrap's nonce

/// What a job wrap carries; part of its key, so one kind never opens as another.
#[derive(Clone, Copy)]
pub enum WrapKind {
    FileKey = 0,
    NoiseSeed = 1,
}

/// What a job wrap is bound to: the run, the attempt, and which secret of it.
#[derive(Clone, Copy)]
pub struct JobWrap<'a> {
    pub app_id: &'a str,
    pub run_id: &'a str,
    pub attempt: u32,
    pub kind: WrapKind,
    pub index: u32,
}

impl JobWrap<'_> {
    /// The HKDF info: both public keys, the kind, the length-prefixed ids, attempt and index.
    fn info(&self, ephemeral_public: &[u8; 32], job_public: &[u8; 32]) -> EyreResult<Vec<u8>> {
        let mut info = [ephemeral_public.as_slice(), job_public].concat();
        info.push(self.kind as u8);
        push_prefixed(&mut info, self.app_id)?;
        push_prefixed(&mut info, self.run_id)?;
        info.extend_from_slice(&self.attempt.to_be_bytes());
        info.extend_from_slice(&self.index.to_be_bytes());
        Ok(info)
    }
}

fn push_prefixed(info: &mut Vec<u8>, field: &str) -> EyreResult<()> {
    let len = u32::try_from(field.len()).map_err(|_| eyre!("an id is too long"))?;
    info.extend_from_slice(&len.to_be_bytes());
    info.extend_from_slice(field.as_bytes());
    Ok(())
}

/// `nonce || AES-256-GCM(key, secret)`.
fn seal(
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
fn open(key: &LessSafeKey, aad: &[u8], bytes: &[u8]) -> EyreResult<Zeroizing<[u8; 32]>> {
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

/// A fresh subkey per wrap, so the random nonce has no per-key bound.
fn file_wrap_key(file_key: &[u8; 32], salt: &[u8]) -> LessSafeKey {
    let label = [FILE_WRAP_LABEL.as_bytes(), salt].concat();
    cipher(&Zeroizing::new(tag(file_key, label)))
}

/// `salt || nonce || AES-256-GCM(HMAC(file_key, label || salt), data_key)`, bound to `app_id`.
pub fn wrap_key(file_key: &[u8; 32], app_id: &str, data_key: &[u8; 32]) -> EyreResult<Vec<u8>> {
    wrap_key_with(
        file_key,
        app_id,
        data_key,
        calimero_identity::random(),
        calimero_identity::random(),
    )
}

fn wrap_key_with(
    file_key: &[u8; 32],
    app_id: &str,
    data_key: &[u8; 32],
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
) -> EyreResult<Vec<u8>> {
    let sealed = seal(
        &file_wrap_key(file_key, &salt),
        nonce,
        app_id.as_bytes(),
        data_key,
    )?;
    Ok([salt.as_slice(), &sealed].concat())
}

/// Open what [`wrap_key`] wrote, for the same app and key.
pub fn unwrap_key(
    file_key: &[u8; 32],
    app_id: &str,
    wrapped: &[u8],
) -> EyreResult<Zeroizing<[u8; 32]>> {
    let Some((salt, rest)) = wrapped.split_first_chunk::<SALT_LEN>() else {
        bail!("the wrapped key is truncated");
    };
    open(&file_wrap_key(file_key, salt), app_id.as_bytes(), rest)
}

/// `ephemeral public || nonce || AES-256-GCM(HKDF(X25519(ephemeral, job_pub)), secret)`,
/// bound to both public keys and to `wrap`.
pub fn wrap_for_job(
    job_public: &[u8; 32],
    wrap: &JobWrap,
    secret: &[u8; 32],
) -> EyreResult<Vec<u8>> {
    wrap_for_job_with(
        job_public,
        wrap,
        secret,
        Zeroizing::new(calimero_identity::random()),
        calimero_identity::random(),
    )
}

/// [`wrap_for_job`] with the ephemeral key and nonce derived from `file_key`: the same run,
/// attempt, secret and job node always wrap to the same bytes, so a new leader rebuilds the
/// assignment its predecessor signed. What is wrapped is fixed by `wrap`, so a nonce is never
/// used for two secrets.
pub fn wrap_for_job_derived(
    file_key: &[u8; 32],
    job_public: &[u8; 32],
    wrap: &JobWrap,
    secret: &[u8; 32],
) -> EyreResult<Vec<u8>> {
    let context = wrap.info(&[0; PUBLIC_LEN], job_public)?;
    let derive = |label: &[u8]| tag(file_key, [label, &context].concat());
    let nonce = derive(DERIVED_NONCE_LABEL)[..NONCE_LEN]
        .try_into()
        .map_err(|_| eyre!("a nonce is {NONCE_LEN} bytes"))?;
    wrap_for_job_with(
        job_public,
        wrap,
        secret,
        Zeroizing::new(derive(DERIVED_EPHEMERAL_LABEL)),
        nonce,
    )
}

fn wrap_for_job_with(
    job_public: &[u8; 32],
    wrap: &JobWrap,
    secret: &[u8; 32],
    ephemeral: Zeroizing<[u8; 32]>,
    nonce: [u8; NONCE_LEN],
) -> EyreResult<Vec<u8>> {
    let ephemeral_public = MontgomeryPoint::mul_base_clamped(*ephemeral).0;
    let shared = Zeroizing::new(MontgomeryPoint(*job_public).mul_clamped(*ephemeral).0);
    let info = wrap.info(&ephemeral_public, job_public)?;
    let key = derive_shared(&shared, JOB_WRAP_SALT, &info)?;
    let sealed = seal(&cipher(&key), nonce, &[], secret)?;
    Ok([ephemeral_public.as_slice(), &sealed].concat())
}

/// Open what [`wrap_for_job`] wrote to `job`'s transport key.
pub fn unwrap_for_job(
    job: &SealedTransport,
    wrap: &JobWrap,
    wrapped: &[u8],
) -> EyreResult<Zeroizing<[u8; 32]>> {
    let Some((ephemeral, rest)) = wrapped.split_first_chunk::<PUBLIC_LEN>() else {
        bail!("the wrapped key is truncated");
    };
    let info = wrap.info(ephemeral, &job.public_key())?;
    let key = job.derive(ephemeral, JOB_WRAP_SALT, &info)?;
    open(&cipher(&key), &[], rest)
}

#[cfg(test)]
mod tests {
    use calimero_sealed::SealedOptions;

    use super::*;

    const FILE_KEY: [u8; 32] = [1; 32];
    const DATA_KEY: [u8; 32] = [42; 32];
    const KEY_WRAP: &str = "02020202020202020202020202020202030303030303030303030303504c01ed29c7602e41d243fe4eb131c2c4c494803499b899e2dabf22677ee932070f72b431d0286044594dacff064fca";
    const JOB_WRAP: &str = "ac01b2209e86354fb853237b5de0f4fab13c7fcbf433a61c019369617fecf10b030303030303030303030303e277fe45d072c5b49cb9122e5489d9273b2572e35f1bc9a656bf514099efc3c687f41a53e5d7e47256c3662a555ef78f";

    #[test]
    fn a_data_key_unwraps_for_its_app() {
        let wrapped = wrap_key(&FILE_KEY, "app", &DATA_KEY).unwrap();
        assert_eq!(*unwrap_key(&FILE_KEY, "app", &wrapped).unwrap(), DATA_KEY);
    }

    #[test]
    fn a_data_key_does_not_unwrap_for_another_app_or_key() {
        let wrapped = wrap_key(&FILE_KEY, "app", &DATA_KEY).unwrap();
        assert!(unwrap_key(&FILE_KEY, "other", &wrapped).is_err());
        assert!(unwrap_key(&[2; 32], "app", &wrapped).is_err());
    }

    #[test]
    fn a_tampered_or_short_wrap_does_not_unwrap() {
        let mut wrapped = wrap_key(&FILE_KEY, "app", &DATA_KEY).unwrap();
        assert!(unwrap_key(&FILE_KEY, "app", &wrapped[..5]).is_err());
        assert!(unwrap_key(&FILE_KEY, "app", &wrapped[..SALT_LEN + 5]).is_err());
        wrapped[SALT_LEN + NONCE_LEN] ^= 1;
        assert!(unwrap_key(&FILE_KEY, "app", &wrapped).is_err());
    }

    #[test]
    fn a_wrap_with_another_salt_does_not_unwrap() {
        let mut wrapped = wrap_key(&FILE_KEY, "app", &DATA_KEY).unwrap();
        wrapped[0] ^= 1;
        assert!(unwrap_key(&FILE_KEY, "app", &wrapped).is_err());
    }

    #[test]
    fn wrapping_twice_gives_different_bytes() {
        assert_ne!(
            wrap_key(&FILE_KEY, "app", &DATA_KEY).unwrap(),
            wrap_key(&FILE_KEY, "app", &DATA_KEY).unwrap()
        );
    }

    #[test]
    fn a_key_wrap_matches_its_known_answer() {
        let wrapped =
            wrap_key_with(&FILE_KEY, "app", &DATA_KEY, [2; SALT_LEN], [3; NONCE_LEN]).unwrap();
        assert_eq!(hex::encode(&wrapped), KEY_WRAP);
        let known = hex::decode(KEY_WRAP).unwrap();
        assert_eq!(*unwrap_key(&FILE_KEY, "app", &known).unwrap(), DATA_KEY);
    }

    fn job() -> SealedTransport {
        SealedTransport::generate(&SealedOptions::default())
    }

    const WRAP: JobWrap = JobWrap {
        app_id: "app",
        run_id: "run",
        attempt: 2,
        kind: WrapKind::FileKey,
        index: 3,
    };

    #[test]
    fn a_derived_wrap_is_the_same_every_time_and_opens_at_the_job_node() {
        let job = job();
        let wrapped = wrap_for_job_derived(&FILE_KEY, &job.public_key(), &WRAP, &DATA_KEY).unwrap();
        assert_eq!(
            wrapped,
            wrap_for_job_derived(&FILE_KEY, &job.public_key(), &WRAP, &DATA_KEY).unwrap()
        );
        assert_eq!(*unwrap_for_job(&job, &WRAP, &wrapped).unwrap(), DATA_KEY);
        assert!(unwrap_for_job(&self::job(), &WRAP, &wrapped).is_err());
    }

    #[test]
    fn a_derived_wrap_differs_by_key_node_attempt_index_and_kind() {
        let (job, other) = (job(), job());
        let wrap = |wrap: &JobWrap| {
            wrap_for_job_derived(&FILE_KEY, &job.public_key(), wrap, &DATA_KEY).unwrap()
        };
        let base = wrap(&WRAP);
        assert_ne!(
            base,
            wrap_for_job_derived(&[2; 32], &job.public_key(), &WRAP, &DATA_KEY).unwrap()
        );
        assert_ne!(
            base,
            wrap_for_job_derived(&FILE_KEY, &other.public_key(), &WRAP, &DATA_KEY).unwrap()
        );
        assert_ne!(base, wrap(&JobWrap { attempt: 3, ..WRAP }));
        assert_ne!(base, wrap(&JobWrap { index: 4, ..WRAP }));
        assert_ne!(
            base,
            wrap(&JobWrap {
                kind: WrapKind::NoiseSeed,
                ..WRAP
            })
        );
    }

    #[test]
    fn a_job_node_unwraps_a_key_wrapped_to_it() {
        let job = job();
        let wrapped = wrap_for_job(&job.public_key(), &WRAP, &DATA_KEY).unwrap();
        assert_eq!(*unwrap_for_job(&job, &WRAP, &wrapped).unwrap(), DATA_KEY);
    }

    #[test]
    fn a_job_wrap_is_bound_to_its_run_attempt_index_kind_and_app() {
        let job = job();
        let wrapped = wrap_for_job(&job.public_key(), &WRAP, &DATA_KEY).unwrap();
        let opens = |wrap: JobWrap| unwrap_for_job(&job, &wrap, &wrapped).is_ok();
        assert!(opens(WRAP));
        assert!(!opens(JobWrap {
            run_id: "other",
            ..WRAP
        }));
        assert!(!opens(JobWrap { attempt: 1, ..WRAP }));
        assert!(!opens(JobWrap { index: 4, ..WRAP }));
        assert!(!opens(JobWrap {
            app_id: "other",
            ..WRAP
        }));
        assert!(!opens(JobWrap {
            kind: WrapKind::NoiseSeed,
            ..WRAP
        }));
    }

    #[test]
    fn ids_that_differ_only_in_where_they_split_get_different_keys() {
        let job = job();
        let split = JobWrap {
            app_id: "a\0b",
            run_id: "c",
            ..WRAP
        };
        let moved = JobWrap {
            app_id: "a",
            run_id: "b\0c",
            ..WRAP
        };
        let wrapped = wrap_for_job(&job.public_key(), &split, &DATA_KEY).unwrap();
        assert!(unwrap_for_job(&job, &moved, &wrapped).is_err());
        assert!(unwrap_for_job(&job, &split, &wrapped).is_ok());
    }

    #[test]
    fn a_job_wrap_does_not_unwrap_on_another_node() {
        let wrapped = wrap_for_job(&job().public_key(), &WRAP, &DATA_KEY).unwrap();
        assert!(unwrap_for_job(&job(), &WRAP, &wrapped).is_err());
    }

    #[test]
    fn a_tampered_or_short_job_wrap_does_not_unwrap() {
        let job = job();
        let mut wrapped = wrap_for_job(&job.public_key(), &WRAP, &DATA_KEY).unwrap();
        assert!(unwrap_for_job(&job, &WRAP, &wrapped[..20]).is_err());
        assert!(unwrap_for_job(&job, &WRAP, &wrapped[..PUBLIC_LEN + 5]).is_err());
        *wrapped.last_mut().unwrap() ^= 1;
        assert!(unwrap_for_job(&job, &WRAP, &wrapped).is_err());
    }

    #[test]
    fn a_job_wrap_with_a_malleated_ephemeral_key_does_not_unwrap() {
        let job = job();
        let mut wrapped = wrap_for_job(&job.public_key(), &WRAP, &DATA_KEY).unwrap();
        // X25519 ignores bit 255, so only the HKDF info can notice it.
        wrapped[PUBLIC_LEN - 1] ^= 0x80;
        assert!(unwrap_for_job(&job, &WRAP, &wrapped).is_err());
    }

    #[test]
    fn a_job_wrap_matches_its_known_answer() {
        let job_public = MontgomeryPoint::mul_base_clamped([5; 32]).0;
        let wrapped = wrap_for_job_with(
            &job_public,
            &WRAP,
            &DATA_KEY,
            Zeroizing::new([4; 32]),
            [3; NONCE_LEN],
        )
        .unwrap();
        assert_eq!(hex::encode(wrapped), JOB_WRAP);
    }

    #[test]
    fn a_low_order_key_is_refused_both_ways() {
        let mut one = [0; 32];
        one[0] = 1;
        let order_8: [u8; 32] =
            hex::decode("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800")
                .unwrap()
                .try_into()
                .unwrap();
        for point in [[0; 32], one, order_8] {
            assert!(wrap_for_job(&point, &WRAP, &DATA_KEY).is_err());
            let mut wrapped = [0u8; PUBLIC_LEN + NONCE_LEN + 48];
            wrapped[..PUBLIC_LEN].copy_from_slice(&point);
            assert!(unwrap_for_job(&job(), &WRAP, &wrapped).is_err());
        }
    }
}
