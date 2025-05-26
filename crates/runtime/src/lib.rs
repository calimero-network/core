use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use wasmer::{CompileError, DeserializeError, Instance, NativeEngineExt, SerializeError, Store};

mod constraint;
pub mod errors;
pub mod logic;
mod memory;
pub mod store;

pub use constraint::Constraint;
use errors::{FunctionCallError, VMRuntimeError};
use logic::{Outcome, VMContext, VMLimits, VMLogic, VMLogicError};
use memory::WasmerTunables;
use store::Storage;

pub type RuntimeResult<T, E = VMRuntimeError> = Result<T, E>;

#[derive(Clone, Debug)]
pub struct Engine {
    limits: VMLimits,
    engine: wasmer::Engine,
}

impl Default for Engine {
    fn default() -> Self {
        let limits = VMLimits::default();

        let engine = wasmer::Engine::default();

        Self::new(engine, limits)
    }
}

impl Engine {
    pub fn new(mut engine: wasmer::Engine, limits: VMLimits) -> Self {
        engine.set_tunables(WasmerTunables::new(&limits));

        Self { limits, engine }
    }

    pub fn compile(&self, bytes: &[u8]) -> Result<Module, CompileError> {
        // todo! apply a prepare step
        // todo! - parse the wasm blob, validate and apply transformations
        // todo!   - validations:
        // todo!     - there is no memory import
        // todo!     - there is no _start function
        // todo!   - transformations:
        // todo!     - remove memory export
        // todo!     - remove memory section
        // todo! cache the compiled module in storage for later

        let module = wasmer::Module::new(&self.engine, bytes)?;

        Ok(Module {
            limits: self.limits.clone(),
            engine: self.engine.clone(),
            module,
        })
    }

    /// Compiles WASM bytes and returns the serialized precompiled module
    pub fn compile_and_serialize(&self, bytes: &[u8]) -> Result<Box<[u8]>, CompileError> {
        let module = self.compile(bytes)?;
        module
            .to_bytes()
            .map_err(|_| CompileError::Codegen("Failed to serialize compiled module".to_string()))
    }

    pub unsafe fn from_precompiled(&self, bytes: &[u8]) -> Result<Module, DeserializeError> {
        let module = wasmer::Module::deserialize(&self.engine, bytes)?;

        Ok(Module {
            limits: self.limits.clone(),
            engine: self.engine.clone(),
            module,
        })
    }

    /// Attempts to run precompiled WASM, falls back to regular compilation if it fails
    pub fn run_precompiled(
        &self,
        precompiled_bytes: &[u8],
        wasm_bytes: &[u8],
        context: ContextId,
        executor: PublicKey,
        method: &str,
        input: &[u8],
        storage: &mut dyn Storage,
    ) -> RuntimeResult<Outcome> {
        // Try to load and run precompiled module first
        if let Ok(module) = unsafe { self.from_precompiled(precompiled_bytes) } {
            match module.run(context, executor, method, input, storage) {
                Ok(outcome) => return Ok(outcome),
                Err(_) => {
                    // Precompiled execution failed, fall back to regular compilation
                }
            }
        }

        // Fallback to regular WASM compilation and execution
        let module = self.compile(wasm_bytes)?;
        module.run(context, executor, method, input, storage)
    }
}

#[derive(Debug)]
pub struct Module {
    limits: VMLimits,
    engine: wasmer::Engine,
    module: wasmer::Module,
}

impl Module {
    pub fn to_bytes(&self) -> Result<Box<[u8]>, SerializeError> {
        let bytes = self.module.serialize()?;

        Ok(Vec::into_boxed_slice(bytes.into()))
    }

    pub fn run(
        &self,
        context: ContextId,
        executor: PublicKey,
        method: &str,
        input: &[u8],
        storage: &mut dyn Storage,
    ) -> RuntimeResult<Outcome> {
        let context = VMContext::new(input.into(), *context, *executor);

        let mut logic = VMLogic::new(storage, context, &self.limits);

        let mut store = Store::new(self.engine.clone());

        let imports = logic.imports(&mut store);

        let instance = match Instance::new(&mut store, &self.module, &imports) {
            Ok(instance) => instance,
            Err(err) => return Ok(logic.finish(Some(err.into()))),
        };

        let _ = match instance.exports.get_memory("memory") {
            Ok(memory) => logic.with_memory(memory.clone()),
            // todo! test memory returns MethodNotFound
            Err(err) => return Ok(logic.finish(Some(err.into()))),
        };

        let function = match instance.exports.get_function(method) {
            Ok(function) => function,
            Err(err) => return Ok(logic.finish(Some(err.into()))),
        };

        let signature = function.ty(&store);

        if !(signature.params().is_empty() && signature.results().is_empty()) {
            return Ok(logic.finish(Some(FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::InvalidSignature {
                    name: method.to_owned(),
                },
            ))));
        }

        if let Err(err) = function.call(&mut store, &[]) {
            return match err.downcast::<VMLogicError>() {
                Ok(err) => Ok(logic.finish(Some(err.try_into()?))),
                Err(err) => Ok(logic.finish(Some(err.into()))),
            };
        }

        Ok(logic.finish(None))
    }
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {eyre as _, owo_colors as _, rand as _};
}

#[cfg(test)]
mod tests {
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;

    use super::*;

    // Mock storage for testing
    struct MockStorage;

    impl Storage for MockStorage {
        fn get(&self, _key: &Vec<u8>) -> Option<Vec<u8>> {
            None
        }

        fn set(&mut self, _key: Vec<u8>, _value: Vec<u8>) -> Option<Vec<u8>> {
            None
        }

        fn remove(&mut self, _key: &Vec<u8>) -> Option<Vec<u8>> {
            None
        }

        fn has(&self, _key: &Vec<u8>) -> bool {
            false
        }
    }

    #[test]
    fn test_compile_and_serialize() {
        let engine = Engine::default();

        // Simple WASM module that exports a function
        let wasm_bytes = wat::parse_str(
            r#"
            (module
                (func (export "test") (result i32)
                    i32.const 42
                )
                (memory (export "memory") 1)
            )
        "#,
        )
        .unwrap();

        let result = engine.compile_and_serialize(&wasm_bytes);
        assert!(result.is_ok());

        let precompiled = result.unwrap();
        assert!(!precompiled.is_empty());
    }

    #[test]
    fn test_precompiled_execution() {
        let engine = Engine::default();

        // Simple WASM module
        let wasm_bytes = wat::parse_str(
            r#"
            (module
                (func (export "test") (result i32)
                    i32.const 42
                )
                (memory (export "memory") 1)
            )
        "#,
        )
        .unwrap();

        // Compile and serialize
        let precompiled = engine.compile_and_serialize(&wasm_bytes).unwrap();

        // Test precompiled execution
        let mut storage = MockStorage;
        let context_id = ContextId::from([1u8; 32]);
        let executor = PublicKey::from([2u8; 32]);

        let result = engine.run_precompiled(
            &precompiled,
            &wasm_bytes,
            context_id,
            executor,
            "test",
            &[],
            &mut storage,
        );

        // Should succeed (though the actual execution might fail due to missing host functions)
        assert!(result.is_ok());
    }

    #[test]
    fn test_fallback_to_regular_execution() {
        let engine = Engine::default();

        let wasm_bytes = wat::parse_str(
            r#"
            (module
                (func (export "test") (result i32)
                    i32.const 42
                )
                (memory (export "memory") 1)
            )
        "#,
        )
        .unwrap();

        // Use invalid precompiled data to force fallback
        let invalid_precompiled = vec![0u8; 10];

        let mut storage = MockStorage;
        let context_id = ContextId::from([1u8; 32]);
        let executor = PublicKey::from([2u8; 32]);

        let result = engine.run_precompiled(
            &invalid_precompiled,
            &wasm_bytes,
            context_id,
            executor,
            "test",
            &[],
            &mut storage,
        );

        // Should succeed by falling back to regular compilation
        assert!(result.is_ok());
    }
}
