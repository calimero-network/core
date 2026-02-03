use assert_json_diff::assert_json_eq;
use serde_json::json;

use super::*;

#[test]
fn compilation_error() {
    let error = FunctionCallError::CompilationError {
        source: CompileError::Validate("invalid wasm".to_owned()),
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
        source: LinkError::Resource("missing function".to_owned()),
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
    let error = FunctionCallError::MethodResolutionError(MethodResolutionError::InvalidSignature {
        name: "foo".to_owned(),
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
    let error = FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
        name: "bar".to_owned(),
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
fn panic_host() {
    let error = FunctionCallError::HostError(HostError::Panic {
        context: PanicContext::Host,
        message: "explicit panic".to_owned(),
        location: Location::At {
            file: "path/to/file.rs".to_owned(),
            line: 42,
            column: 24,
        },
    });

    let expected = json!({
        "type": "HostError",
        "data": {
            "type": "Panic",
            "data": {
                "context": "Host",
                "message": "explicit panic",
                "location": {
                    "file": "path/to/file.rs",
                    "line": 42,
                    "column": 24
                }
            }
        }
    });

    assert_eq!(
        error.to_string(),
        "host panicked: explicit panic at path/to/file.rs:42:24"
    );
    assert_json_eq!(json!(error), expected);
}

#[test]
fn panic_guest() {
    let error = FunctionCallError::HostError(HostError::Panic {
        context: PanicContext::Guest,
        message: "explicit panic".to_owned(),
        location: Location::Unknown,
    });

    let expected = json!({
        "type": "HostError",
        "data": {
            "type": "Panic",
            "data": {
                "context": "Guest",
                "message": "explicit panic"
            }
        }
    });

    assert_eq!(error.to_string(), "guest panicked: explicit panic");
    assert_json_eq!(json!(error), expected);
}

#[test]
fn instantiation_failure_cpu_feature() {
    let error = FunctionCallError::InstantiationFailure(InstantiationFailure::CpuFeature {
        feature: "sse4.2".to_owned(),
    });

    let expected = json!({
        "type": "InstantiationFailure",
        "data": {
            "type": "CpuFeature",
            "data": {
                "feature": "sse4.2"
            }
        }
    });

    assert_eq!(
        error.to_string(),
        "host CPU does not support a required feature: sse4.2"
    );
    assert_json_eq!(json!(error), expected);
}

#[test]
fn instantiation_failure_different_stores() {
    let error = FunctionCallError::InstantiationFailure(InstantiationFailure::DifferentStores);

    let expected = json!({
        "type": "InstantiationFailure",
        "data": {
            "type": "DifferentStores"
        }
    });

    assert_eq!(
        error.to_string(),
        "one of the imports is incompatible with this execution instance"
    );
    assert_json_eq!(json!(error), expected);
}

#[test]
fn instantiation_failure_different_arch_os() {
    let error = FunctionCallError::InstantiationFailure(InstantiationFailure::DifferentArchOS);

    let expected = json!({
        "type": "InstantiationFailure",
        "data": {
            "type": "DifferentArchOS"
        }
    });

    assert_eq!(
        error.to_string(),
        "the module was compiled for a different architecture or operating system"
    );
    assert_json_eq!(json!(error), expected);
}
