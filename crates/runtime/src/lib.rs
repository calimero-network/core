use wasmer::{Instance, Module, NativeEngineExt, Store};

mod constraint;
pub mod errors;
pub mod logic;
mod memory;
pub mod store;

pub use constraint::Constraint;

pub type Result<T, E = errors::VMRuntimeError> = std::result::Result<T, E>;

pub fn run(
    code: &[u8],
    method_name: &str,
    context: logic::VMContext,
    storage: &mut dyn store::Storage,
    limits: &logic::VMLimits,
) -> Result<logic::Outcome> {
    // todo! calculate storage key for cached precompiled
    // todo! module, execute that, instead of recompiling
    let mut engine = wasmer::Engine::default();

    engine.set_tunables(memory::WasmerTunables::new(limits));

    let mut store = Store::new(engine);

    let mut logic = logic::VMLogic::new(storage, context, limits);

    // todo! apply a prepare step
    // todo! - parse the wasm blob, validate and apply transformations
    // todo!   - validations:
    // todo!     - there is no memory import
    // todo!     - there is no _start function
    // todo!   - transformations:
    // todo!     - remove memory export
    // todo!     - remove memory section
    // todo! cache the compiled module in storage for later

    let module = match Module::new(&store, code) {
        Ok(module) => module,
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let imports = logic.imports(&mut store);

    let instance = match Instance::new(&mut store, &module, &imports) {
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

    let signature = function.ty(&store);

    if !(signature.params().is_empty() && signature.results().is_empty()) {
        return Ok(
            logic.finish(Some(errors::FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::InvalidSignature {
                    name: method_name.to_string(),
                },
            ))),
        );
    }

    if let Err(err) = function.call(&mut store, &[]) {
        return match err.downcast::<logic::VMLogicError>() {
            Ok(err) => Ok(logic.finish(Some(err.try_into()?))),
            Err(err) => Ok(logic.finish(Some(err.into()))),
        };
    }

    Ok(logic.finish(None))
}
