use thiserror::Error as ThisError;
use wasmer::MemoryAccessError;

use crate::errors::{FunctionCallError, HostError, StorageError, VMRuntimeError};

#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum VMLogicError {
    #[error(transparent)]
    HostError(#[from] HostError),
    #[error(transparent)]
    StorageError(StorageError),
}

impl From<MemoryAccessError> for VMLogicError {
    fn from(_: MemoryAccessError) -> Self {
        Self::HostError(HostError::InvalidMemoryAccess)
    }
}

impl TryFrom<VMLogicError> for FunctionCallError {
    type Error = VMRuntimeError;

    fn try_from(err: VMLogicError) -> Result<Self, Self::Error> {
        match err {
            VMLogicError::StorageError(err) => Err(VMRuntimeError::StorageError(err)),
            // Host errors surface as an ordinary failed call
            // (`FunctionCallError::HostError`); they never panic or halt the
            // node. Host-error paths are reachable while running guest code, so
            // turning them into a node panic would let a crafted app that can
            // provoke one take the node down — a denial-of-service. Keeping them
            // as call failures also matches the `catch_unwind` boundary in
            // `Module::run_with_origin`, whose whole purpose is that nothing in
            // execution can crash the node. Surfacing host-side bugs through a
            // durable/on-chain record is a separate concern, not handled here.
            VMLogicError::HostError(err) => Ok(Self::HostError(err)),
        }
    }
}
