use wasmer::{Engine, Instance, Module, NativeEngineExt, Store};

use crate::errors::{FunctionCallError, VMRuntimeError};
use crate::logic::{Outcome, VMContext, VMLimits, VMLogic, VMLogicError};
use crate::memory::WasmerTunables;
use crate::store::Storage;

mod constraint;
pub mod errors;
pub mod logic;
mod memory;
pub mod store;

pub use constraint::Constraint;

pub type RuntimeResult<T, E = VMRuntimeError> = Result<T, E>;

/// Compiles WASM code and returns both the compiled module and its serialized form
pub fn compile_and_serialize(code: &[u8], limits: &VMLimits) -> RuntimeResult<(Module, Vec<u8>)> {
    let mut engine = Engine::default();
    engine.set_tunables(WasmerTunables::new(limits));
    let store = Store::new(engine);

    let module = Module::new(&store, code)
        .map_err(|_err| VMRuntimeError::HostError(errors::HostError::DeserializationError))?;

    let serialized = module
        .serialize()
        .map_err(|_err| VMRuntimeError::HostError(errors::HostError::DeserializationError))?;

    Ok((module, serialized.to_vec()))
}

/// Runs a precompiled WASM module
pub fn run_precompiled(
    precompiled_data: &[u8],
    method_name: &str,
    context: VMContext,
    storage: &mut dyn Storage,
    limits: &VMLimits,
) -> RuntimeResult<Outcome> {
    let mut engine = Engine::default();
    engine.set_tunables(WasmerTunables::new(limits));
    let mut store = Store::new(engine);

    let mut logic = VMLogic::new(storage, context, limits);

    // Deserialize the precompiled module
    let module = unsafe {
        Module::deserialize(&store, precompiled_data).map_err(|_err| {
            // If deserialization fails, we should fall back to regular compilation
            VMRuntimeError::HostError(errors::HostError::DeserializationError)
        })?
    };

    execute_module(&mut store, &module, logic, method_name)
}

pub fn run(
    code: &[u8],
    method_name: &str,
    context: VMContext,
    storage: &mut dyn Storage,
    limits: &VMLimits,
) -> RuntimeResult<Outcome> {
    let mut engine = Engine::default();
    engine.set_tunables(WasmerTunables::new(limits));
    let mut store = Store::new(engine);

    let mut logic = VMLogic::new(storage, context, limits);

    // todo! apply a prepare step
    // todo! - parse the wasm blob, validate and apply transformations
    // todo!   - validations:
    // todo!     - there is no memory import
    // todo!     - there is no _start function
    // todo!   - transformations:
    // todo!     - remove memory export
    // todo!     - remove memory section

    let module = match Module::new(&store, code) {
        Ok(module) => module,
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    execute_module(&mut store, &module, logic, method_name)
}

/// Common execution logic for both regular and precompiled modules
fn execute_module(
    store: &mut Store,
    module: &Module,
    mut logic: VMLogic,
    method_name: &str,
) -> RuntimeResult<Outcome> {
    let imports = logic.imports(store);

    let instance = match Instance::new(store, module, &imports) {
        Ok(instance) => instance,
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let _ = match instance.exports.get_memory("memory") {
        Ok(memory) => logic.with_memory(memory.clone()),
        // todo! test memory returns MethodNotFound
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let function = match instance.exports.get_function(method_name) {
        Ok(function) => function,
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let signature = function.ty(store);

    if !(signature.params().is_empty() && signature.results().is_empty()) {
        return Ok(logic.finish(Some(FunctionCallError::MethodResolutionError(
            errors::MethodResolutionError::InvalidSignature {
                name: method_name.to_owned(),
            },
        ))));
    }

    if let Err(err) = function.call(store, &[]) {
        return match err.downcast::<VMLogicError>() {
            Ok(err) => Ok(logic.finish(Some(err.try_into()?))),
            Err(err) => Ok(logic.finish(Some(err.into()))),
        };
    }

    Ok(logic.finish(None))
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {eyre as _, owo_colors as _, rand as _};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::{VMContext, VMLimits};
    use crate::store::InMemoryStorage;

    fn get_test_limits() -> VMLimits {
        VMLimits {
            max_memory_pages: 1 << 10,
            max_stack_size: 200 << 10,
            max_registers: 100,
            max_register_size: (100 << 20).validate().unwrap(),
            max_registers_capacity: 1 << 30,
            max_logs: 100,
            max_log_size: 16 << 10,
            max_events: 100,
            max_event_kind_size: 100,
            max_event_data_size: 16 << 10,
            max_storage_key_size: (1 << 20).try_into().unwrap(),
            max_storage_value_size: (10 << 20).try_into().unwrap(),
        }
    }

    #[test]
    fn test_compile_and_serialize() {
        // Simple WASM module that exports a function
        let wasm_code = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_function"))
            )
        "#,
        )
        .unwrap();

        let limits = get_test_limits();
        let result = compile_and_serialize(&wasm_code, &limits);

        assert!(result.is_ok());
        let (_module, serialized) = result.unwrap();
        assert!(!serialized.is_empty());
    }

    #[test]
    fn test_precompiled_execution() {
        // Simple WASM module that exports a function
        let wasm_code = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_function"))
            )
        "#,
        )
        .unwrap();

        let limits = get_test_limits();

        // First compile and serialize
        let (_module, serialized) = compile_and_serialize(&wasm_code, &limits).unwrap();

        // Then try to run the precompiled version
        let mut storage = InMemoryStorage::default();
        let context = VMContext::new(vec![], [0; 32], [0; 32]);

        let result = run_precompiled(&serialized, "test_function", context, &mut storage, &limits);

        assert!(result.is_ok());
    }

    #[test]
    fn test_fallback_to_regular_execution() {
        // Test that regular execution still works
        let wasm_code = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_function"))
            )
        "#,
        )
        .unwrap();

        let limits = get_test_limits();
        let mut storage = InMemoryStorage::default();
        let context = VMContext::new(vec![], [0; 32], [0; 32]);

        let result = run(&wasm_code, "test_function", context, &mut storage, &limits);

        assert!(result.is_ok());
    }
}
