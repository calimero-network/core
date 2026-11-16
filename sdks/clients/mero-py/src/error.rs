//! `CloudError`, the exception the bindings raise for a node's answer, and
//! the subclasses a caller acts on without reading the code: `LoginRequired`
//! (`401 UNAUTHENTICATED`), `WrongNode` (`421 WRONG_NODE`) and
//! `FeatureNotActive` (`409 FEATURE_NOT_ACTIVE`).

use calimero_client::Error;
use pyo3::PyErr;

// pyo3 0.22's macro checks a `gil-refs` feature this crate does not declare.
#[allow(unexpected_cfgs)]
mod exception {
    use pyo3::create_exception;
    use pyo3::exceptions::PyException;

    create_exception!(
        calimero_client_py,
        CloudError,
        PyException,
        "An error from a cloud node: args are (message, status, code, details); status is 0 when the node could not be reached or attested."
    );
    create_exception!(
        calimero_client_py,
        LoginRequired,
        CloudError,
        "401 UNAUTHENTICATED that the client's own refresh did not answer (or there was no refresh token): sign in again."
    );
    create_exception!(
        calimero_client_py,
        WrongNode,
        CloudError,
        "421 WRONG_NODE: the node's release may not serve this account, and the client knows no other node that took the request: use another node of the cluster."
    );
    create_exception!(
        calimero_client_py,
        FeatureNotActive,
        CloudError,
        "409 FEATURE_NOT_ACTIVE: the command needs a cluster version not every node supports yet (a rollout from an older release is under way); nothing was done. details names the feature, the version it needs and the cluster's (null when not every node said). Retry once the old nodes have left."
    );
}
pub use exception::{CloudError, FeatureNotActive, LoginRequired, WrongNode};

pub(crate) fn cloud_err(e: Error) -> PyErr {
    match e {
        Error::Api {
            status,
            code,
            message,
            details,
        } => {
            let args = (
                format!("{status} {code}: {message}"),
                status,
                code.clone(),
                details.to_string(),
            );
            match (status, code.as_str()) {
                (401, "UNAUTHENTICATED") => LoginRequired::new_err(args),
                (421, "WRONG_NODE") => WrongNode::new_err(args),
                (409, "FEATURE_NOT_ACTIVE") => FeatureNotActive::new_err(args),
                _ => CloudError::new_err(args),
            }
        }
        // Core's client names a `421 WRONG_NODE` itself (core #4774).
        Error::WrongNode { message, details } => WrongNode::new_err((
            format!("421 WRONG_NODE: {message}"),
            421_u16,
            "WRONG_NODE".to_owned(),
            details.to_string(),
        )),
        Error::Transport(r) => CloudError::new_err((format!("{r:#}"), 0_u16, "TRANSPORT", "null")),
    }
}

/// The node directory gave no usable node (`Client.connect`): a `CloudError`
/// with code `DIRECTORY` and the directory's status (0 when it could not be
/// reached or answered no JSON).
pub(crate) fn directory_err(e: crate::client::DirectoryError) -> PyErr {
    CloudError::new_err((e.message, e.status, "DIRECTORY", "null"))
}
