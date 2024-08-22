use thiserror::Error;

use crate::errors::{FunctionCallError, HostError, StorageError, VMRuntimeError};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VMLogicError {
    #[error(transparent)]
    HostError(#[from] HostError),
    #[error(transparent)]
    StorageError(StorageError),
}

impl From<wasmer::MemoryAccessError> for VMLogicError {
    fn from(_: wasmer::MemoryAccessError) -> Self {
        Self::HostError(HostError::InvalidMemoryAccess)
    }
}

impl TryFrom<VMLogicError> for FunctionCallError {
    type Error = VMRuntimeError;

    fn try_from(err: VMLogicError) -> Result<Self, Self::Error> {
        match err {
            VMLogicError::StorageError(err) => Err(VMRuntimeError::StorageError(err)),
            // todo! is it fine to panic the node on host errors
            // todo! because that is a bug in the node, or do we
            // todo! include it in the result? and record it on chain
            // VMLogicError::HostError(HostError::Panic {
            //     context: PanicContext::Host,
            //     message,
            // }) => Err(VMRuntimeError::HostError(err)),
            VMLogicError::HostError(err) => Ok(Self::HostError(err)),
        }
    }
}
