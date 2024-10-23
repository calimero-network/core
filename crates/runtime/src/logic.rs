#![allow(single_use_lifetimes, unused_lifetimes, reason = "False positive")]
#![allow(clippy::mem_forget, reason = "Safe for now")]

use core::num::NonZeroU64;
use std::time::{SystemTime, UNIX_EPOCH};

use borsh::from_slice as from_borsh_slice;
use ouroboros::self_referencing;
use rand::RngCore;
use serde::Serialize;

use crate::constraint::{Constrained, MaxU64};
use crate::errors::{FunctionCallError, HostError, Location, PanicContext};
use crate::store::Storage;

mod errors;
mod imports;
mod registers;

pub use errors::VMLogicError;
use registers::Registers;

pub type VMLogicResult<T, E = VMLogicError> = Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub struct VMContext {
    pub input: Vec<u8>,
    pub context_id: [u8; 32],
    pub executor_public_key: [u8; 32],
}

impl VMContext {
    #[must_use]
    pub const fn new(input: Vec<u8>, context_id: [u8; 32], executor_public_key: [u8; 32]) -> Self {
        Self {
            input,
            context_id,
            executor_public_key,
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
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
    pub max_events: u64,
    pub max_event_kind_size: u64,
    pub max_event_data_size: u64,
    pub max_storage_key_size: NonZeroU64,
    pub max_storage_value_size: NonZeroU64,
    // pub max_execution_time: u64,
    // number of functions per contract
}

impl VMLimits {
    #[expect(clippy::too_many_arguments, reason = "Acceptable here")]
    #[must_use]
    pub const fn new(
        max_memory_pages: u32,
        max_stack_size: usize,
        max_registers: u64,
        max_register_size: Constrained<u64, MaxU64<{ u64::MAX - 1 }>>,
        max_registers_capacity: u64,
        max_logs: u64,
        max_log_size: u64,
        max_events: u64,
        max_event_kind_size: u64,
        max_event_data_size: u64,
        max_storage_key_size: NonZeroU64,
        max_storage_value_size: NonZeroU64,
    ) -> Self {
        Self {
            max_memory_pages,
            max_stack_size,
            max_registers,
            max_register_size,
            max_registers_capacity,
            max_logs,
            max_log_size,
            max_events,
            max_event_kind_size,
            max_event_data_size,
            max_storage_key_size,
            max_storage_value_size,
        }
    }
}

#[derive(Debug)]
pub struct VMLogic<'a> {
    storage: &'a mut dyn Storage,
    memory: Option<wasmer::Memory>,
    context: VMContext,
    limits: &'a VMLimits,
    registers: Registers,
    returns: Option<VMLogicResult<Vec<u8>, Vec<u8>>>,
    actions: Vec<Vec<u8>>,
    logs: Vec<String>,
    events: Vec<Event>,
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
            actions: vec![],
            logs: vec![],
            events: vec![],
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

#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Outcome {
    pub returns: VMLogicResult<Option<Vec<u8>>, FunctionCallError>,
    pub actions: Vec<Vec<u8>>,
    pub logs: Vec<String>,
    pub events: Vec<Event>,
    // execution runtime
    // current storage usage of the app
}

#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Event {
    pub kind: String,
    pub data: Vec<u8>,
}

impl VMLogic<'_> {
    #[must_use]
    pub fn finish(self, err: Option<FunctionCallError>) -> Outcome {
        let returns = match err {
            Some(err) => Err(err),
            None => self
                .returns
                .map(|t| t.map_err(FunctionCallError::ExecutionError))
                .transpose(),
        };

        Outcome {
            returns,
            actions: self.actions,
            logs: self.logs,
            events: self.events,
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

impl VMHostFunctions<'_> {
    fn read_guest_memory(&self, ptr: u64, len: u64) -> VMLogicResult<Vec<u8>> {
        let mut buf = vec![0; usize::try_from(len).map_err(|_| HostError::IntegerOverflow)?];

        self.borrow_memory().read(ptr, &mut buf)?;

        Ok(buf)
    }

    fn get_string(&self, ptr: u64, len: u64) -> VMLogicResult<String> {
        let buf = self.read_guest_memory(ptr, len)?;

        String::from_utf8(buf).map_err(|_| HostError::BadUTF8.into())
    }
}

impl VMHostFunctions<'_> {
    pub fn panic(&self, file_ptr: u64, file_len: u64, line: u32, column: u32) -> VMLogicResult<()> {
        let file = self.get_string(file_ptr, file_len)?;
        Err(HostError::Panic {
            context: PanicContext::Guest,
            message: "explicit panic".to_owned(),
            location: Location::At { file, line, column },
        }
        .into())
    }

    pub fn panic_utf8(
        &self,
        msg_ptr: u64,
        msg_len: u64,
        file_ptr: u64,
        file_len: u64,
        line: u32,
        column: u32,
    ) -> VMLogicResult<()> {
        let message = self.get_string(msg_ptr, msg_len)?;
        let file = self.get_string(file_ptr, file_len)?;

        Err(HostError::Panic {
            context: PanicContext::Guest,
            message,
            location: Location::At { file, line, column },
        }
        .into())
    }

    pub fn register_len(&self, register_id: u64) -> VMLogicResult<u64> {
        Ok(self
            .borrow_logic()
            .registers
            .get_len(register_id)
            .unwrap_or(u64::MAX))
    }

    pub fn read_register(&self, register_id: u64, ptr: u64, len: u64) -> VMLogicResult<u32> {
        let data = self.borrow_logic().registers.get(register_id)?;
        if data.len() != usize::try_from(len).map_err(|_| HostError::IntegerOverflow)? {
            return Ok(0);
        }
        self.borrow_memory().write(ptr, data)?;
        Ok(1)
    }

    pub fn context_id(&mut self, register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(logic.limits, register_id, logic.context.context_id)
        })
    }

    pub fn executor_id(&mut self, register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(logic.limits, register_id, logic.context.executor_public_key)
        })
    }

    pub fn input(&mut self, register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(logic.limits, register_id, &*logic.context.input)
        })?;

        Ok(())
    }

    pub fn value_return(&mut self, tag: u64, ptr: u64, len: u64) -> VMLogicResult<()> {
        let buf = self.read_guest_memory(ptr, len)?;

        let result = match tag {
            0 => Ok(buf),
            1 => Err(buf),
            _ => return Err(HostError::InvalidMemoryAccess.into()),
        };

        self.with_logic_mut(|logic| logic.returns = Some(result));

        Ok(())
    }

    pub fn log_utf8(&mut self, ptr: u64, len: u64) -> VMLogicResult<()> {
        let logic = self.borrow_logic();

        if logic.logs.len()
            >= usize::try_from(logic.limits.max_logs).map_err(|_| HostError::IntegerOverflow)?
        {
            return Err(HostError::LogsOverflow.into());
        }

        let message = self.get_string(ptr, len)?;

        self.with_logic_mut(|logic| logic.logs.push(message));

        Ok(())
    }

    pub fn emit(
        &mut self,
        kind_ptr: u64,
        kind_len: u64,
        data_ptr: u64,
        data_len: u64,
    ) -> VMLogicResult<()> {
        let logic = self.borrow_logic();

        if kind_len > logic.limits.max_event_kind_size {
            return Err(HostError::EventKindSizeOverflow.into());
        }

        if data_len > logic.limits.max_event_data_size {
            return Err(HostError::EventDataSizeOverflow.into());
        }

        if logic.events.len()
            >= usize::try_from(logic.limits.max_events).map_err(|_| HostError::IntegerOverflow)?
        {
            return Err(HostError::EventsOverflow.into());
        }

        let kind = self.get_string(kind_ptr, kind_len)?;
        let data = self.read_guest_memory(data_ptr, data_len)?;

        self.with_logic_mut(|logic| logic.events.push(Event { kind, data }));

        Ok(())
    }

    /// Sends an action to the host.
    ///
    /// After a storage event, other nodes need to be updated. Consequently, the
    /// host must be informed about the action that was taken. This function
    /// sends that action to the host, which can then be used to update the
    /// network.
    ///
    pub fn send_action(&mut self, action_ptr: u64, action_len: u64) -> VMLogicResult<()> {
        let action_bytes = self.read_guest_memory(action_ptr, action_len)?;

        self.with_logic_mut(|logic| logic.actions.push(action_bytes));

        Ok(())
    }

    pub fn storage_read(
        &mut self,
        key_ptr: u64,
        key_len: u64,
        register_id: u64,
    ) -> VMLogicResult<u32> {
        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        let key = self.read_guest_memory(key_ptr, key_len)?;

        if let Some(value) = logic.storage.get(&key) {
            self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, value))?;

            return Ok(1);
        }

        Ok(0)
    }

    pub fn storage_remove(
        &mut self,
        key_ptr: u64,
        key_len: u64,
        register_id: u64,
    ) -> VMLogicResult<u32> {
        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        let key = self.read_guest_memory(key_ptr, key_len)?;

        if let Some(value) = logic.storage.get(&key) {
            self.with_logic_mut(|logic| {
                drop(logic.storage.remove(&key));
                logic.registers.set(logic.limits, register_id, value)
            })?;

            return Ok(1);
        }

        Ok(0)
    }

    pub fn storage_write(
        &mut self,
        key_ptr: u64,
        key_len: u64,
        value_ptr: u64,
        value_len: u64,
        register_id: u64,
    ) -> VMLogicResult<u32> {
        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        if value_len > logic.limits.max_storage_value_size.get() {
            return Err(HostError::ValueLengthOverflow.into());
        }

        let key = self.read_guest_memory(key_ptr, key_len)?;
        let value = self.read_guest_memory(value_ptr, value_len)?;

        let evicted = self.with_logic_mut(|logic| logic.storage.set(key, value));

        if let Some(evicted) = evicted {
            self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, evicted))?;

            return Ok(1);
        };

        Ok(0)
    }

    #[expect(clippy::too_many_arguments, reason = "Acceptable here")]
    pub fn fetch(
        &mut self,
        url_ptr: u64,
        url_len: u64,
        method_ptr: u64,
        method_len: u64,
        headers_ptr: u64,
        headers_len: u64,
        body_ptr: u64,
        body_len: u64,
        register_id: u64,
    ) -> VMLogicResult<u32> {
        let url = self.get_string(url_ptr, url_len)?;
        let method = self.get_string(method_ptr, method_len)?;
        let headers = self.read_guest_memory(headers_ptr, headers_len)?;

        // Note: The `fetch` function cannot be directly called by applications.
        // Therefore, the headers are generated exclusively by our code, ensuring
        // that it is safe to deserialize them.
        let headers: Vec<(String, String)> =
            from_borsh_slice(&headers).map_err(|_| HostError::DeserializationError)?;
        let body = self.read_guest_memory(body_ptr, body_len)?;
        let mut request = ureq::request(&method, &url);

        for (key, value) in &headers {
            request = request.set(key, value);
        }

        let response = if body.is_empty() {
            request.call()
        } else {
            request.send_bytes(&body)
        };

        let (status, data) = match response {
            Ok(response) => {
                let mut buffer = vec![];
                match response.into_reader().read_to_end(&mut buffer) {
                    Ok(_) => (0, buffer),
                    Err(_) => (1, "Failed to read the response body.".into()),
                }
            }
            Err(e) => (1, e.to_string().into_bytes()),
        };

        self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, data))?;
        Ok(status)
    }

    pub fn random_bytes(&mut self, ptr: u64, len: u64) -> VMLogicResult<()> {
        let mut buf = vec![0; usize::try_from(len).map_err(|_| HostError::IntegerOverflow)?];

        rand::thread_rng().fill_bytes(&mut buf);
        self.borrow_memory().write(ptr, &buf)?;

        Ok(())
    }

    /// Gets the current time.
    ///
    /// This function obtains the current time as a nanosecond timestamp, as
    /// [`SystemTime`] is not available inside the guest runtime. Therefore the
    /// guest needs to request this from the host.
    ///
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Impossible to overflow in normal circumstances"
    )]
    #[expect(
        clippy::expect_used,
        clippy::unwrap_in_result,
        reason = "Effectively infallible here"
    )]
    pub fn time_now(&mut self, ptr: u64, len: u64) -> VMLogicResult<()> {
        if len != 8 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64;

        self.borrow_memory().write(ptr, &now.to_le_bytes())?;

        Ok(())
    }
}
