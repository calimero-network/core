//! `Visitor`: an anonymous visitor through a public link, over core's
//! `calimero_client::visitor`.

use calimero_client::visitor::Visitor;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::client::unix_now;

/// A visitor of one public link: the link's token and the visitor's own
/// Ed25519 key. `Client.use_visitor` makes one and signs every request with
/// it; the node names the visitor to the app as `Caller::Visitor { link, key }`.
#[pyclass(name = "Visitor", module = "calimero_client_py._native", frozen)]
pub struct PyVisitor {
    pub(crate) inner: Visitor,
}

impl PyVisitor {
    /// The visitor of the link whose token is `link`, with the key whose seed
    /// is `seed` (32 bytes).
    pub(crate) fn new(link: &str, seed: &[u8]) -> PyResult<Self> {
        let seed = <[u8; 32]>::try_from(seed)
            .map_err(|_| PyValueError::new_err("a visitor seed is 32 bytes"))?;
        if link.is_empty() {
            return Err(PyValueError::new_err("a public link token is not empty"));
        }
        Ok(Self {
            inner: Visitor::new(link, seed),
        })
    }
}

#[pymethods]
impl PyVisitor {
    #[new]
    #[pyo3(signature = (link, seed))]
    fn py_new(link: &str, seed: &[u8]) -> PyResult<Self> {
        Self::new(link, seed)
    }

    /// The link's key (hex): how the node and the app name the link.
    #[getter]
    fn link(&self) -> String {
        self.inner.link()
    }

    /// The visitor's public key (64 lowercase hex): how the app names it.
    #[getter]
    fn public_key(&self) -> String {
        self.inner.public_key()
    }

    /// The bearer credential for a request at `at` (Unix seconds; default
    /// now): `vis1.<token>.<key>.<at>.<signature>`, valid within 5 minutes of
    /// the node's clock.
    #[pyo3(signature = (at = None))]
    fn bearer(&self, at: Option<u64>) -> String {
        self.inner.bearer(at.unwrap_or_else(unix_now))
    }

    fn __repr__(&self) -> String {
        format!("Visitor(public_key={:?})", self.inner.public_key())
    }
}

#[cfg(test)]
mod tests {
    use super::PyVisitor;

    /// The vector core's client and mero-js check (and mero-swift and
    /// mero-kotlin): seed `[9; 32]`, token `pl_token`, at 1000.
    #[test]
    fn the_shared_visitor_vector_holds() {
        let v = PyVisitor::new("pl_token", &[9; 32]).unwrap();
        assert_eq!(
            v.link(),
            "d4205f1ac7bb3fe43278131b6e621ebf952f0a5f51a38505327572d863c46c42"
        );
        assert_eq!(
            v.public_key(),
            "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618"
        );
        assert_eq!(v.bearer(Some(1000)), "vis1.pl_token.fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618.1000.9b5205e4deb4b534a612c3b03be773b60aea16192f2fb6d99496d194f2dd259173173731e56a46ec7b37cc3d2d2482fe7be5d70c0fadda54c22f5ea2b8fa4e09");
    }
}
