use std::num::NonZeroU64;

use ouroboros::self_referencing;

use super::store::Storage;

mod errors;
mod imports;
mod registers;

use crate::constraint::{Constrained, MaxU64};
use crate::errors::{FunctionCallError, HostError};
pub use errors::VMLogicError;
use registers::Registers;

pub type Result<T, E = errors::VMLogicError> = std::result::Result<T, E>;

pub struct VMContext {
    pub input: Vec<u8>,
}

pub struct VMLimits {
    pub max_memory_pages: u32,
    pub max_stack_size: usize,
    pub max_registers: u64,
    // constrained to be less than u64::MAX
    // because register_len returns u64::MAX if the register is not found
    pub max_register_size: Constrained<u64, MaxU64<{ u64::MAX - 1 }>>,
    pub max_registers_capacity: u64, // todo! must not be less than max_register_size
    pub max_logs: u64,
    pub max_log_size: u64,
    pub max_storage_key_size: NonZeroU64,
    pub max_storage_value_size: NonZeroU64,
    // pub max_execution_time: u64,
    // number of functions per contract
}

pub struct VMLogic<'a> {
    storage: &'a mut dyn Storage,
    memory: Option<wasmer::Memory>,
    context: VMContext,
    limits: &'a VMLimits,
    registers: Registers,
    returns: Option<Vec<u8>>,
    logs: Vec<String>,
}

impl<'a> VMLogic<'a> {
    pub fn new(storage: &'a mut dyn Storage, context: VMContext, limits: &'a VMLimits) -> Self {
        VMLogic {
            storage,
            memory: None,
            context,
            limits,
            registers: Registers::default(),
            returns: None,
            logs: vec![],
        }
    }

    pub fn with_memory(&mut self, memory: wasmer::Memory) -> &mut Self {
        self.memory = Some(memory);
        self
    }

    pub fn host_functions(&'a mut self, store: wasmer::StoreMut<'a>) -> VMHostFunctions<'a> {
        let memory = self.memory.clone().expect("VM Memory not initialized");

        VMHostFunctionsBuilder {
            logic: self,
            store,

            memory_builder: |store| memory.view(store),
        }
        .build()
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub returns: Option<Result<Vec<u8>, FunctionCallError>>,
    pub logs: Vec<String>,
    // execution runtime
    // current storage usage of the app
}

impl<'a> VMLogic<'a> {
    pub fn finish(self, err: Option<FunctionCallError>) -> Outcome {
        Outcome {
            returns: err.map(Err).or_else(|| self.returns.map(Ok)),
            logs: self.logs,
        }
    }
}

#[self_referencing]
pub struct VMHostFunctions<'a> {
    logic: &'a mut VMLogic<'a>,
    store: wasmer::StoreMut<'a>,

    #[covariant]
    #[borrows(store)]
    memory: wasmer::MemoryView<'this>,
}

impl<'a> VMHostFunctions<'a> {
    pub fn read_register(&mut self, register_id: u64, ptr: u64) -> Result<()> {
        let data = self.borrow_logic().registers.get(register_id)?;
        self.borrow_memory().write(ptr, data)?;
        Ok(())
    }

    pub fn register_len(&self, register_id: u64) -> Result<u64> {
        Ok(self
            .borrow_logic()
            .registers
            .get_len(register_id)
            .unwrap_or(u64::MAX))
    }

    pub fn input(&mut self, register_id: u64) -> Result<()> {
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(&logic.limits, register_id, &logic.context.input[..])
        })?;

        Ok(())
    }

    pub fn panic(&self) -> Result<()> {
        Err(HostError::GuestPanic {
            message: "explicit guest panic".to_string(),
        }
        .into())
    }

    fn get_string(&self, len: u64, ptr: u64) -> Result<String> {
        let mut buf = vec![0; len as usize];

        self.borrow_memory().read(ptr, &mut buf)?;

        String::from_utf8(buf).map_err(|_| HostError::BadUTF8.into())
    }

    pub fn panic_utf8(&self, len: u64, ptr: u64) -> Result<()> {
        let message = self.get_string(len, ptr)?;

        Err(HostError::GuestPanic { message }.into())
    }

    pub fn value_return(&mut self, len: u64, ptr: u64) -> Result<()> {
        let mut buf = vec![0; len as usize];

        self.borrow_memory().read(ptr, &mut buf)?;

        self.with_logic_mut(|logic| logic.returns = Some(buf));

        Ok(())
    }

    pub fn log_utf8(&mut self, len: u64, ptr: u64) -> Result<()> {
        let logic = self.borrow_logic();

        if logic.logs.len() >= logic.limits.max_logs as usize {
            return Err(HostError::LogsOverflow.into());
        }

        let message = self.get_string(len, ptr)?;

        self.with_logic_mut(|logic| logic.logs.push(message));

        Ok(())
    }

    pub fn storage_write(
        &mut self,
        key_len: u64,
        key_ptr: u64,
        value_len: u64,
        value_ptr: u64,
        register_id: u64,
    ) -> Result<u64> {
        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        if value_len > logic.limits.max_storage_value_size.get() {
            return Err(HostError::ValueLengthOverflow.into());
        }

        let key = self.get_string(key_len, key_ptr)?;
        let value = self.get_string(value_len, value_ptr)?;

        let evicted =
            self.with_logic_mut(|logic| logic.storage.set(key.as_bytes(), value.as_bytes()));

        if let Some(evicted) = evicted {
            self.with_logic_mut(|logic| logic.registers.set(&logic.limits, register_id, evicted))?;

            return Ok(1);
        };

        Ok(0)
    }

    pub fn storage_read(&mut self, key_len: u64, key_ptr: u64, register_id: u64) -> Result<u64> {
        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        let key = self.get_string(key_len, key_ptr)?;

        if let Some(value) = logic.storage.get(key.as_bytes()) {
            self.with_logic_mut(|logic| logic.registers.set(&logic.limits, register_id, value))?;

            return Ok(1);
        }

        Ok(0)
    }
}
