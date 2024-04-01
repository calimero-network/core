use serde::Serialize;
use thiserror::Error;
use wasmer_types::TrapCode;

#[derive(Debug, Error)]
pub enum VMRuntimeError {
    #[error(transparent)]
    StorageError(StorageError),

    #[error(transparent)]
    HostError(HostError),
}

#[derive(Debug, Error)]
pub enum StorageError {}

#[derive(Debug, Error, Serialize)]
#[serde(tag = "error", content = "data")]
pub enum FunctionCallError {
    #[error("compilation error: {}", .source)]
    CompilationError {
        #[from]
        #[serde(skip)]
        source: wasmer::CompileError,
    },
    #[error("link error: {}", .source)]
    LinkError {
        #[from]
        #[serde(skip)]
        source: wasmer::LinkError,
    },
    #[error(transparent)]
    MethodResolutionError(MethodResolutionError),
    #[error(transparent)]
    WasmTrap(WasmTrap),
    #[error(transparent)]
    HostError(HostError),
    #[error("the method call returned an error")]
    ExecutionError(Vec<u8>),
}

#[derive(Debug, Error, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum MethodResolutionError {
    #[error("method {name:?} has invalid signature: expected no arguments and no return value")]
    InvalidSignature { name: String },
    #[error("method {name:?} not found")]
    MethodNotFound { name: String },
}

#[derive(Debug, Error, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum HostError {
    #[error("invalid register id: {id}")]
    InvalidRegisterId { id: u64 },
    #[error("invalid memory access")]
    InvalidMemoryAccess,
    #[error("{} panicked: {message}", match .context {
        PanicContext::Guest => "guest",
        PanicContext::Host => "host",
    })]
    Panic {
        context: PanicContext,
        message: String,
    },
    #[error("invalid UTF-8 string")]
    BadUTF8,
    #[error("key length overflow")]
    KeyLengthOverflow,
    #[error("value length overflow")]
    ValueLengthOverflow,
    #[error("logs overflow")]
    LogsOverflow,
}

#[derive(Debug, Serialize)]
pub enum PanicContext {
    Guest,
    Host,
}

#[derive(Debug, Error, Serialize)]
#[error("{self:?}")]
pub enum WasmTrap {
    StackOverflow,
    MemoryOutOfBounds,
    HeapMisaligned,
    TableAccessOutOfBounds,
    IndirectCallToNull,
    BadSignature,
    IllegalArithmetic,
    Unreachable,
    UnalignedAtomic,
    Indeterminate,
}

impl From<wasmer::ExportError> for FunctionCallError {
    fn from(err: wasmer::ExportError) -> Self {
        match err {
            wasmer::ExportError::Missing(name) => {
                FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
                    name,
                })
            }
            wasmer::ExportError::IncompatibleType => unreachable!(),
        }
    }
}

impl From<wasmer::InstantiationError> for FunctionCallError {
    fn from(err: wasmer::InstantiationError) -> Self {
        match err {
            wasmer::InstantiationError::Link(err) => err.into(),
            wasmer::InstantiationError::Start(err) => err.into(),
            wasmer::InstantiationError::CpuFeature(err) => {
                panic!("host CPU does not support a required feature: {}", err)
            }
            wasmer::InstantiationError::DifferentStores => {
                panic!("one of the imports is incompatible with this execution instance")
            }
            wasmer::InstantiationError::DifferentArchOS => {
                panic!("the module was compiled for a different architecture or operating system")
            }
        }
    }
}

impl From<wasmer::RuntimeError> for FunctionCallError {
    fn from(err: wasmer::RuntimeError) -> Self {
        match err.to_trap() {
            Some(TrapCode::StackOverflow) => FunctionCallError::WasmTrap(WasmTrap::StackOverflow),
            Some(TrapCode::HeapAccessOutOfBounds | TrapCode::TableAccessOutOfBounds) => {
                FunctionCallError::WasmTrap(WasmTrap::MemoryOutOfBounds)
            }
            Some(TrapCode::HeapMisaligned) => FunctionCallError::WasmTrap(WasmTrap::HeapMisaligned),
            Some(TrapCode::IndirectCallToNull) => {
                FunctionCallError::WasmTrap(WasmTrap::IndirectCallToNull)
            }
            Some(TrapCode::BadSignature) => FunctionCallError::WasmTrap(WasmTrap::BadSignature),
            Some(
                TrapCode::IntegerOverflow
                | TrapCode::IntegerDivisionByZero
                | TrapCode::BadConversionToInteger,
            ) => FunctionCallError::WasmTrap(WasmTrap::IllegalArithmetic),
            Some(TrapCode::UnreachableCodeReached) => {
                FunctionCallError::WasmTrap(WasmTrap::Unreachable)
            }
            Some(TrapCode::UnalignedAtomic) => {
                FunctionCallError::WasmTrap(WasmTrap::UnalignedAtomic)
            }
            None => FunctionCallError::WasmTrap(WasmTrap::Indeterminate),
        }
    }
}
