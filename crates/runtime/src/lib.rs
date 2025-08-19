use calimero_node_primitives::client::NodeClient;
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

    pub fn headless() -> Self {
        let limits = VMLimits::default();

        let engine = wasmer::Engine::headless();

        Self::new(engine, limits)
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

    pub unsafe fn from_precompiled(&self, bytes: &[u8]) -> Result<Module, DeserializeError> {
        let module = wasmer::Module::deserialize(&self.engine, bytes)?;

        Ok(Module {
            limits: self.limits.clone(),
            engine: self.engine.clone(),
            module,
        })
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
        node_client: Option<NodeClient>,
    ) -> RuntimeResult<Outcome> {
        let context = VMContext::new(input.into(), *context, *executor);

        let mut logic = VMLogic::new(storage, context, &self.limits, node_client);

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
