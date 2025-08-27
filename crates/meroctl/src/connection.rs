//! ConnectionInfo from client, specialized for meroctl's needs.

use client::connection::ConnectionInfo as GenericConnectionInfo;

use crate::auth::CliAuthenticator;
use crate::storage::FileTokenStorage;

/// Type alias for ConnectionInfo specialized for meroctl
///
/// This uses the generic ConnectionInfo from client with
/// meroctl's concrete authenticator and storage implementations.
pub type ConnectionInfo = GenericConnectionInfo<CliAuthenticator, FileTokenStorage>;
