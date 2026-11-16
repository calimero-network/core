//! The roots a client trusts for TLS on top of the built-in public ones
//! (`ca_bundle`): for nodes whose certificates come from a private or staging
//! CA, such as Let's Encrypt's staging roots on a non-production cluster.
//!
//! Every HTTP client the bindings build takes them: each node's sealed channel
//! (and its attestation fetch), a leader a `421 NOT_LEADER` names, and the
//! node directory (`Client.connect`). The roots are added, never substituted:
//! the public roots stay trusted. Core's own `calimero_client::Client` (the
//! event streams, `attestation()`, `verify_app_cluster`) builds its HTTP
//! client inside core and still trusts the public roots only.

use pyo3::exceptions::PyValueError;
use pyo3::PyErr;
use reqwest::Certificate;

/// The extra roots; none is today's behaviour.
#[derive(Clone, Default)]
pub(crate) struct ExtraRoots(Vec<Certificate>);

impl ExtraRoots {
    /// The roots in the PEM file at `path`, none without one. A file that
    /// cannot be read is the `OSError` its failure stands for
    /// (`FileNotFoundError`, `PermissionError`); one with no certificate, or
    /// one that is not a usable root, `ValueError`. Both name the path.
    pub(crate) fn load(path: Option<&str>) -> Result<Self, PyErr> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let pem = std::fs::read(path).map_err(|e| {
            PyErr::from(std::io::Error::new(
                e.kind(),
                format!("ca_bundle {path}: {e}"),
            ))
        })?;
        Self::from_pem(&pem)
            .map_err(|why| PyValueError::new_err(format!("ca_bundle {path}: {why}")))
    }

    /// The roots in a PEM bundle (comments and text between the certificates
    /// are skipped), checked by building a client with them.
    pub(crate) fn from_pem(pem: &[u8]) -> Result<Self, String> {
        let certs =
            Certificate::from_pem_bundle(pem).map_err(|e| format!("not a PEM bundle: {e}"))?;
        if certs.is_empty() {
            return Err("holds no PEM certificate".to_owned());
        }
        let roots = Self(certs);
        let _checked = roots
            .builder()
            .build()
            .map_err(|e| format!("a certificate in it is not a usable root: {e}"))?;
        Ok(roots)
    }

    /// A client builder trusting the built-in roots and these.
    pub(crate) fn builder(&self) -> reqwest::ClientBuilder {
        self.0.iter().cloned().fold(
            reqwest::Client::builder(),
            reqwest::ClientBuilder::add_root_certificate,
        )
    }

    /// How many roots were added.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_no_extra_root() {
        assert_eq!(ExtraRoots::load(None).map(|r| r.len()).ok(), Some(0));
    }

    #[test]
    fn text_with_no_certificate_is_refused() {
        let err = ExtraRoots::from_pem(b"not a certificate\n").err().unwrap();
        assert!(err.contains("no PEM certificate"), "{err}");
    }

    #[test]
    fn a_broken_certificate_is_refused() {
        let pem = b"-----BEGIN CERTIFICATE-----\nAAAAAAAAAAAA\n-----END CERTIFICATE-----\n";
        assert!(ExtraRoots::from_pem(pem).is_err());
    }
}
