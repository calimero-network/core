//! The roots a node trusts for HTTPS to its peers, job nodes and attestation
//! fetches: the built-in web roots (`webpki-roots`), plus the extra roots of
//! `--tls-extra-roots` when given.
//!
//! The extra roots are for a test environment whose node certificates come
//! from a CA no browser or `webpki-roots` trusts, such as the Let's Encrypt
//! staging CA. Without them two such nodes refuse each other's certificate
//! (`UnknownIssuer`), so no node can join a running cluster. They add to the
//! web roots and never replace them; with none, every client is built as it
//! was before the option existed.

use std::path::Path;

use eyre::{bail, WrapErr};

/// The certificates of a PEM file of extra roots, in file order.
///
/// # Errors
///
/// The file cannot be read, a PEM block is not a certificate, or it holds no
/// certificate at all: a node asked to trust extra roots that it would
/// silently drop would fail later, and far from the cause.
pub fn load_extra_roots(path: &Path) -> eyre::Result<Vec<reqwest::Certificate>> {
    let pem = std::fs::read(path)
        .wrap_err_with(|| format!("--tls-extra-roots: cannot read {}", path.display()))?;
    let roots = reqwest::Certificate::from_pem_bundle(&pem).wrap_err_with(|| {
        format!(
            "--tls-extra-roots: {} is not a PEM bundle of certificates",
            path.display()
        )
    })?;
    if roots.is_empty() {
        bail!("--tls-extra-roots: {} holds no certificate", path.display());
    }
    Ok(roots)
}

/// A client builder that trusts `extra` on top of the web roots: every client
/// a node uses to reach its peers starts here.
pub fn peer_client(extra: &[reqwest::Certificate]) -> reqwest::ClientBuilder {
    extra
        .iter()
        .cloned()
        .fold(reqwest::Client::builder(), |builder, root| {
            builder.add_root_certificate(root)
        })
}
