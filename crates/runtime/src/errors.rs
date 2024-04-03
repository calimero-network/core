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
#[serde(tag = "type", content = "data")]
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

#[cfg(test)]
mod tests {
    use assert_json_diff::assert_json_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn compilation_error() {
        let error = FunctionCallError::CompilationError {
            source: wasmer::CompileError::Validate("invalid wasm".to_string()),
        };

        let expected = json!({
            "type": "CompilationError",
            "data": {}
        });

        assert_eq!(
            error.to_string(),
            "compilation error: Validation error: invalid wasm"
        );
        assert_json_eq!(json!(error), expected);
    }

    #[test]
    fn link_error() {
        let error = FunctionCallError::LinkError {
            source: wasmer::LinkError::Resource("missing function".to_string()),
        };

        let expected = json!({
            "type": "LinkError",
            "data": {}
        });

        assert_eq!(
            error.to_string(),
            "link error: Insufficient resources: missing function"
        );
        assert_json_eq!(json!(error), expected);
    }

    #[test]
    fn invalid_signature() {
        let error =
            FunctionCallError::MethodResolutionError(MethodResolutionError::InvalidSignature {
                name: "foo".to_string(),
            });

        let expected = json!({
            "type": "MethodResolutionError",
            "data": {
                "type": "InvalidSignature",
                "data": {
                    "name": "foo"
                }
            }
        });

        assert_eq!(
            error.to_string(),
            "method \"foo\" has invalid signature: expected no arguments and no return value"
        );
        assert_json_eq!(json!(error), expected);
    }

    #[test]
    fn method_not_found() {
        let error =
            FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
                name: "bar".to_string(),
            });

        let expected = json!({
            "type": "MethodResolutionError",
            "data": {
                "type": "MethodNotFound",
                "data": {
                    "name": "bar"
                }
            }
        });

        assert_eq!(error.to_string(), "method \"bar\" not found");
        assert_json_eq!(json!(error), expected);
    }

    #[test]
    fn stack_overflow() {
        let error = FunctionCallError::WasmTrap(WasmTrap::StackOverflow);

        let expected = json!({
            "type": "WasmTrap",
            "data": "StackOverflow"
        });

        assert_eq!(error.to_string(), "stack overflow");
        assert_json_eq!(json!(error), expected);
    }

    #[test]
    fn invalid_memory_access() {
        let error = FunctionCallError::HostError(HostError::InvalidMemoryAccess);

        let expected = json!({
            "type": "HostError",
            "data": {
                "type": "InvalidMemoryAccess"
            }
        });

        assert_eq!(error.to_string(), "invalid memory access");
        assert_json_eq!(json!(error), expected);
    }

    #[test]
    fn panic() {
        let error = FunctionCallError::HostError(HostError::Panic {
            context: PanicContext::Guest,
            message: "explicit panic".to_string(),
        });

        let expected = json!({
            "type": "HostError",
            "data": {
                "type": "Panic",
                "data": {
                    "context": "guest",
                    "message": "explicit panic"
                }
            }
        });

        assert_eq!(error.to_string(), "guest panicked: explicit panic");
        assert_json_eq!(json!(error), expected);
    }
}
