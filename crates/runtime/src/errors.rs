#[cfg(test)]
#[path = "tests/errors.rs"]
mod tests;

use core::panic::Location as PanicLocation;

use serde::Serialize;
use thiserror::Error as ThisError;
use wasmer::{ExportError, InstantiationError, LinkError, RuntimeError};
use wasmer_types::{CompileError, TrapCode};

#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum VMRuntimeError {
    #[error(transparent)]
    StorageError(StorageError),

    #[error(transparent)]
    HostError(HostError),

    #[error(transparent)]
    FunctionCallError(FunctionCallError),
}

#[derive(Copy, Clone, Debug, ThisError)]
#[non_exhaustive]
pub enum StorageError {}

#[derive(Debug, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum FunctionCallError {
    #[error("compilation error: {}", .source)]
    CompilationError {
        #[from]
        #[serde(skip)]
        source: CompileError,
    },
    #[error("link error: {}", .source)]
    LinkError {
        #[from]
        #[serde(skip)]
        source: LinkError,
    },
    #[error(transparent)]
    MethodResolutionError(MethodResolutionError),
    #[error(transparent)]
    WasmTrap(WasmTrap),
    #[error(transparent)]
    HostError(HostError),
    #[error("the method call returned an error: {0:?}")]
    ExecutionError(Vec<u8>),
}

#[derive(Debug, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum MethodResolutionError {
    #[error("method {name:?} has invalid signature: expected no arguments and no return value")]
    InvalidSignature { name: String },
    #[error("method {name:?} not found")]
    MethodNotFound { name: String },
}

#[derive(Debug, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum HostError {
    #[error("invalid register id: {id}")]
    InvalidRegisterId { id: u64 },
    #[error("invalid memory access")]
    InvalidMemoryAccess,
    #[error(
        "{} panicked: {message}{}",
        match .context {
            PanicContext::Guest => "guest",
            PanicContext::Host => "host",
        },
        match .location {
            Location::Unknown => String::new(),
            Location::At { file, line, column } => format!(" at {file}:{line}:{column}"),
        }
    )]
    Panic {
        context: PanicContext,
        message: String,
        #[serde(skip_serializing_if = "Location::is_unknown")]
        location: Location,
    },
    #[error("invalid UTF-8 string")]
    BadUTF8,
    #[error("deserialization error")]
    DeserializationError,
    #[error("integer overflow")]
    IntegerOverflow,
    #[error("key length overflow")]
    KeyLengthOverflow,
    #[error("value length overflow")]
    ValueLengthOverflow,
    #[error("logs overflow")]
    LogsOverflow,
    #[error("events overflow")]
    EventsOverflow,
    #[error("event kind size overflow")]
    EventKindSizeOverflow,
    #[error("event data size overflow")]
    EventDataSizeOverflow,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "There are no more possible variants"
)]
pub enum PanicContext {
    Guest,
    Host,
}

#[derive(Copy, Clone, Debug, Serialize, ThisError)]
#[non_exhaustive]
pub enum WasmTrap {
    #[error("stack overflow")]
    StackOverflow,
    #[error("memory out of bounds")]
    MemoryOutOfBounds,
    #[error("heap misaligned")]
    HeapMisaligned,
    #[error("table access out of bounds")]
    TableAccessOutOfBounds,
    #[error("indirect call to null")]
    IndirectCallToNull,
    #[error("bad signature")]
    BadSignature,
    #[error("illegal arithmetic operation")]
    IllegalArithmetic,
    #[error("unreachable code reached")]
    Unreachable,
    #[error("unaligned atomic operation")]
    UnalignedAtomic,
    #[error("indeterminate trap")]
    Indeterminate,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Location {
    At {
        file: String,
        line: u32,
        column: u32,
    },
    Unknown,
}

impl Location {
    const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl From<&PanicLocation<'_>> for Location {
    fn from(location: &PanicLocation<'_>) -> Self {
        Self::At {
            file: location.file().to_owned(),
            line: location.line(),
            column: location.column(),
        }
    }
}

impl From<ExportError> for FunctionCallError {
    fn from(err: ExportError) -> Self {
        match err {
            ExportError::Missing(name) => {
                Self::MethodResolutionError(MethodResolutionError::MethodNotFound { name })
            }
            ExportError::IncompatibleType => unreachable!(),
        }
    }
}

// TODO: We should change this to use TryFrom instead of panicking in a From.
#[expect(
    clippy::fallible_impl_from,
    reason = "TODO: This needs to be refactored"
)]
impl From<InstantiationError> for FunctionCallError {
    fn from(err: InstantiationError) -> Self {
        match err {
            InstantiationError::Link(err) => err.into(),
            InstantiationError::Start(err) => err.into(),
            InstantiationError::CpuFeature(err) => {
                panic!("host CPU does not support a required feature: {err}")
            }
            InstantiationError::DifferentStores => {
                panic!("one of the imports is incompatible with this execution instance")
            }
            InstantiationError::DifferentArchOS => {
                panic!("the module was compiled for a different architecture or operating system")
            }
        }
    }
}

impl From<RuntimeError> for FunctionCallError {
    fn from(err: RuntimeError) -> Self {
        match err.to_trap() {
            Some(TrapCode::StackOverflow) => Self::WasmTrap(WasmTrap::StackOverflow),
            Some(TrapCode::HeapAccessOutOfBounds | TrapCode::TableAccessOutOfBounds) => {
                Self::WasmTrap(WasmTrap::MemoryOutOfBounds)
            }
            Some(TrapCode::HeapMisaligned) => Self::WasmTrap(WasmTrap::HeapMisaligned),
            Some(TrapCode::IndirectCallToNull) => Self::WasmTrap(WasmTrap::IndirectCallToNull),
            Some(TrapCode::BadSignature) => Self::WasmTrap(WasmTrap::BadSignature),
            Some(
                TrapCode::IntegerOverflow
                | TrapCode::IntegerDivisionByZero
                | TrapCode::BadConversionToInteger,
            ) => Self::WasmTrap(WasmTrap::IllegalArithmetic),
            Some(TrapCode::UnreachableCodeReached) => Self::WasmTrap(WasmTrap::Unreachable),
            Some(TrapCode::UnalignedAtomic) => Self::WasmTrap(WasmTrap::UnalignedAtomic),
            None => Self::WasmTrap(WasmTrap::Indeterminate),
        }
    }
}

impl From<CompileError> for VMRuntimeError {
    fn from(err: CompileError) -> Self {
        Self::FunctionCallError(FunctionCallError::CompilationError { source: err })
    }
}
