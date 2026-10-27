//! The engine contract (D3): engines run components, the host core decides everything else.

use std::any::Any;
use std::sync::Arc;

use thiserror::Error;

use crate::abi::Abi;
use crate::core::HostCore;
use crate::types::{Budget, Outcome};

/// Why a component can't be prepared.
#[derive(Debug, Error)]
pub enum PrepareError {
    /// The component imports an interface outside the world and the D44 stubs.
    #[error("forbidden import: {0}")]
    ForbiddenImport(String),
    /// The component carries no `calimero:abi` section.
    #[error("component has no calimero:abi section")]
    NoAbi,
    /// The ABI is malformed or carries an attribute the host does not know.
    #[error("bad abi: {0}")]
    BadAbi(String),
    /// The engine refused or failed to compile the component.
    #[error("compile failed: {0}")]
    Compile(String),
}

/// A validated, compiled component, ready for fresh instances.
#[derive(Clone)]
pub struct Prepared {
    /// The bundle hash it was prepared from.
    pub hash: String,
    pub abi: Abi,
    /// Engine-specific compiled form; only the engine that made it downcasts it.
    pub inner: Arc<dyn Any + Send + Sync>,
}

/// One per engine, shared by every app. Knows nothing about Raft, identity or policy:
/// every host call goes through `host`, and the outcome comes from `HostCore::finish`.
pub trait Engine: Send + Sync {
    /// Validate the component's imports against the world, read its ABI and compile it.
    fn prepare(&self, bundle_hash: &str, component: &[u8]) -> Result<Prepared, PrepareError>;

    /// Run `handle` for `host.invocation()` in a fresh instance under `budget`.
    fn invoke(&self, app: &Prepared, host: HostCore<'_>, budget: Budget) -> Outcome;
}
