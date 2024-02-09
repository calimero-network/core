use thiserror::Error;

use crate::errors::{FunctionCallError, HostError, StorageError, VMRuntimeError};

#[derive(Error, Debug)]
pub enum VMLogicError {
    #[error(transparent)]
    HostError(HostError),
    #[error(transparent)]
    StorageError(StorageError),
}

impl From<wasmer::MemoryAccessError> for VMLogicError {
    fn from(_: wasmer::MemoryAccessError) -> Self {
        VMLogicError::HostError(HostError::InvalidMemoryAccess)
    }
}

impl TryFrom<VMLogicError> for FunctionCallError {
    type Error = VMRuntimeError;

    fn try_from(err: VMLogicError) -> Result<Self, Self::Error> {
        match err {
            VMLogicError::StorageError(err) => Err(VMRuntimeError::StorageError(err)),
            VMLogicError::HostError(err) => Ok(FunctionCallError::HostError(err)),
        }
    }
}

impl From<HostError> for VMLogicError {
    fn from(value: HostError) -> Self {
        VMLogicError::HostError(value)
    }
}
