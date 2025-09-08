#![allow(single_use_lifetimes, unused_lifetimes, reason = "False positive")]
#![allow(clippy::mem_forget, reason = "Safe for now")]

use core::num::NonZeroU64;
use core::{fmt, slice};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read};
use std::mem::MaybeUninit;
use std::time::{SystemTime, UNIX_EPOCH};
use std::vec;

use borsh::from_slice as from_borsh_slice;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use calimero_sys as sys;
use futures_util::{StreamExt, TryStreamExt};
use ouroboros::self_referencing;
use rand::RngCore;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::constraint::{Constrained, MaxU64};
use crate::errors::{FunctionCallError, HostError, Location, PanicContext};
use crate::store::Storage;
use crate::Constraint;

mod errors;
mod imports;
mod registers;

pub use errors::VMLogicError;
use registers::Registers;

pub type VMLogicResult<T, E = VMLogicError> = Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub struct VMContext<'a> {
    pub input: Cow<'a, [u8]>,
    pub context_id: [u8; 32],
    pub executor_public_key: [u8; 32],
}

impl<'a> VMContext<'a> {
    #[must_use]
    pub const fn new(
        input: Cow<'a, [u8]>,
        context_id: [u8; 32],
        executor_public_key: [u8; 32],
    ) -> Self {
        Self {
            input,
            context_id,
            executor_public_key,
        }
    }
}

#[derive(Debug, Clone, Copy)]
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
    pub max_blob_handles: u64,
    pub max_blob_chunk_size: u64,
}

impl Default for VMLimits {
    fn default() -> Self {
        #[inline(always)]
        fn is_valid<T, E: fmt::Debug>(t: Result<T, E>) -> T {
            t.expect("is valid")
        }

        VMLimits {
            max_memory_pages: 1 << 10,                               // 1 KiB (64 KiB?)
            max_stack_size: 200 << 10,                               // 200 KiB
            max_registers: 100,                                      //
            max_register_size: is_valid((100 << 20).validate()),     // 100 MiB
            max_registers_capacity: 1 << 30,                         // 1 GiB
            max_logs: 100,                                           //
            max_log_size: 16 << 10,                                  // 16 KiB
            max_events: 100,                                         //
            max_event_kind_size: 100,                                //
            max_event_data_size: 16 << 10,                           // 16 KiB
            max_storage_key_size: is_valid((1 << 20).try_into()),    // 1 MiB
            max_storage_value_size: is_valid((10 << 20).try_into()), // 10 MiB
            max_blob_handles: 100,
            max_blob_chunk_size: 10 << 20, // 10 MiB
        }
    }
}

enum BlobHandle {
    Write(BlobWriteHandle),
    Read(BlobReadHandle),
}

#[derive(Debug)]
struct BlobWriteHandle {
    sender: mpsc::UnboundedSender<Vec<u8>>,
    completion_handle: tokio::task::JoinHandle<eyre::Result<(BlobId, u64)>>,
}

struct BlobReadHandle {
    blob_id: BlobId,
    // Stream state
    stream:
        Option<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin>>,
    // Cursor for current storage chunk - automatic position tracking!
    current_chunk_cursor: Option<Cursor<Vec<u8>>>,
    position: u64,
}

#[expect(
    missing_debug_implementations,
    reason = "storage and node_client can't impl Debug"
)]
pub struct VMLogic<'a> {
    storage: &'a mut dyn Storage,
    memory: Option<wasmer::Memory>,
    context: VMContext<'a>,
    limits: &'a VMLimits,
    registers: Registers,
    returns: Option<VMLogicResult<Vec<u8>, Vec<u8>>>,
    logs: Vec<String>,
    events: Vec<Event>,
    root_hash: Option<[u8; 32]>,
    artifact: Vec<u8>,
    proposals: BTreeMap<[u8; 32], Vec<u8>>,
    approvals: Vec<[u8; 32]>,

    // Blob functionality
    node_client: Option<NodeClient>,
    blob_handles: HashMap<u64, BlobHandle>,
    next_blob_fd: u64,
}

impl<'a> VMLogic<'a> {
    pub fn new(
        storage: &'a mut dyn Storage,
        context: VMContext<'a>,
        limits: &'a VMLimits,
        node_client: Option<NodeClient>,
    ) -> Self {
        VMLogic {
            storage,
            memory: None,
            context,
            limits,
            registers: Registers::default(),
            returns: None,
            logs: vec![],
            events: vec![],
            root_hash: None,
            artifact: vec![],
            proposals: BTreeMap::new(),
            approvals: vec![],

            // Blob functionality
            node_client,
            blob_handles: HashMap::new(),
            next_blob_fd: 1,
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
    pub logs: Vec<String>,
    pub events: Vec<Event>,
    pub root_hash: Option<[u8; 32]>,
    pub artifact: Vec<u8>,
    pub proposals: BTreeMap<[u8; 32], Vec<u8>>,
    //list of ids for approved proposals
    pub approvals: Vec<[u8; 32]>,
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
            logs: self.logs,
            events: self.events,
            root_hash: self.root_hash,
            artifact: self.artifact,
            proposals: self.proposals,
            approvals: self.approvals,
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
    fn read_slice(&mut self, slice: &sys::Buffer<'_>) -> &mut [u8] {
        let ptr = slice.ptr().value().as_usize();
        let len = slice.len() as usize;

        unsafe { &mut self.borrow_memory().data_unchecked_mut()[ptr..ptr + len] }
    }

    fn read_guest_memory_sized<const N: usize>(
        &self,
        ptr: u64,
        len: u64,
    ) -> VMLogicResult<[u8; N]> {
        let len = usize::try_from(len).map_err(|_| HostError::IntegerOverflow)?;

        if len != N {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let mut buf = [0; N];

        self.borrow_memory().read(ptr, &mut buf)?;

        Ok(buf)
    }

    /// Reads a sized type from guest memory.
    unsafe fn read_typed<T>(&self, ptr: u64) -> VMLogicResult<T> {
        let mut value = MaybeUninit::<T>::uninit();

        let raw = slice::from_raw_parts_mut(value.as_mut_ptr().cast::<u8>(), size_of::<T>());

        self.borrow_memory().read(ptr, raw)?;

        Ok(value.assume_init())
    }
}

impl VMHostFunctions<'_> {
    pub fn panic(&mut self, location_ptr: u64) -> VMLogicResult<()> {
        let location = unsafe { self.read_typed::<sys::Location<'_>>(location_ptr)? };

        let file = String::from_utf8(self.read_slice(&location.file()).to_vec())
            .map_err(|_| HostError::BadUTF8)?;
        let line = location.line();
        let column = location.column();

        Err(HostError::Panic {
            context: PanicContext::Guest,
            message: "explicit panic".to_owned(),
            location: Location::At { file, line, column },
        }
        .into())
    }

    pub fn panic_utf8(&mut self, msg_ptr: u64, location_ptr: u64) -> VMLogicResult<()> {
        let message_buf = unsafe { self.read_typed::<sys::Buffer<'_>>(msg_ptr)? };
        let location = unsafe { self.read_typed::<sys::Location<'_>>(location_ptr)? };

        let message = String::from_utf8(self.read_slice(&message_buf).to_vec())
            .map_err(|_| HostError::BadUTF8)?;
        let file = String::from_utf8(self.read_slice(&location.file()).to_vec())
            .map_err(|_| HostError::BadUTF8)?;
        let line = location.line();
        let column = location.column();

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

    pub fn read_register(&self, register_id: u64, register_ptr: u64) -> VMLogicResult<u32> {
        let register = unsafe { self.read_typed::<sys::BufferMut<'_>>(register_ptr)? };

        let data = self.borrow_logic().registers.get(register_id)?;
        if data.len() != usize::try_from(register.len()).map_err(|_| HostError::IntegerOverflow)? {
            return Ok(0);
        }
        self.borrow_memory()
            .write(register.ptr().value().as_usize() as u64, data)?;
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

    pub fn value_return(&mut self, value_ptr: u64) -> VMLogicResult<()> {
        let result = unsafe { self.read_typed::<sys::ValueReturn<'_>>(value_ptr)? };

        let result = match result {
            sys::ValueReturn::Ok(value) => Ok(self.read_slice(&value).to_vec()),
            sys::ValueReturn::Err(value) => Err(self.read_slice(&value).to_vec()),
        };

        self.with_logic_mut(|logic| logic.returns = Some(result));

        Ok(())
    }

    pub fn log_utf8(&mut self, log_ptr: u64) -> VMLogicResult<()> {
        let buf = unsafe { self.read_typed::<sys::Buffer<'_>>(log_ptr)? };

        let message =
            String::from_utf8(self.read_slice(&buf).to_vec()).map_err(|_| HostError::BadUTF8)?;

        let logic = self.borrow_logic();

        if logic.logs.len()
            >= usize::try_from(logic.limits.max_logs).map_err(|_| HostError::IntegerOverflow)?
        {
            return Err(HostError::LogsOverflow.into());
        }

        self.with_logic_mut(|logic| logic.logs.push(message));

        Ok(())
    }

    pub fn emit(&mut self, event_ptr: u64) -> VMLogicResult<()> {
        let event = unsafe { self.read_typed::<sys::Event<'_>>(event_ptr)? };

        let kind_len = event.kind().len();
        let data_len = event.data().len();

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

        let kind = String::from_utf8(self.read_slice(event.kind()).to_vec())
            .map_err(|_| HostError::BadUTF8)?;
        let data = self.read_slice(event.data()).to_vec();

        self.with_logic_mut(|logic| logic.events.push(Event { kind, data }));

        Ok(())
    }

    pub fn commit(&mut self, root_hash_ptr: u64, artifact_ptr: u64) -> VMLogicResult<()> {
        let root_hash = unsafe { self.read_typed::<sys::Buffer<'_>>(root_hash_ptr)? };
        let artifact = unsafe { self.read_typed::<sys::Buffer<'_>>(artifact_ptr)? };

        let root_hash = self
            .read_slice(&root_hash)
            .try_into()
            .map_err(|_| HostError::InvalidMemoryAccess)?;
        let artifact = self.read_slice(&artifact).to_vec();

        self.with_logic_mut(|logic| {
            if logic.root_hash.is_some() {
                return Err(HostError::InvalidMemoryAccess);
            }

            logic.root_hash = Some(root_hash);
            logic.artifact = artifact;

            Ok(())
        })?;

        Ok(())
    }

    pub fn storage_read(&mut self, key_ptr: u64, register_id: u64) -> VMLogicResult<u32> {
        let key = unsafe { self.read_typed::<sys::Buffer<'_>>(key_ptr)? };
        let key_len = key.len();

        let key = self
            .read_slice(&key)
            .try_into()
            .map_err(|_| HostError::InvalidMemoryAccess)?;

        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        if let Some(value) = logic.storage.get(&key) {
            self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, value))?;

            return Ok(1);
        }

        Ok(0)
    }

    pub fn storage_remove(&mut self, key_ptr: u64, register_id: u64) -> VMLogicResult<u32> {
        let key = unsafe { self.read_typed::<sys::Buffer<'_>>(key_ptr)? };
        let key_len = key.len();

        let key = self
            .read_slice(&key)
            .try_into()
            .map_err(|_| HostError::InvalidMemoryAccess)?;

        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

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
        value_ptr: u64,
        register_id: u64,
    ) -> VMLogicResult<u32> {
        let key = unsafe { self.read_typed::<sys::Buffer<'_>>(key_ptr)? };
        let key_len = key.len();

        let value = unsafe { self.read_typed::<sys::Buffer<'_>>(value_ptr)? };
        let value_len = value.len();

        let logic = self.borrow_logic();

        if key_len > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        if value_len > logic.limits.max_storage_value_size.get() {
            return Err(HostError::ValueLengthOverflow.into());
        }

        let key = self
            .read_slice(&key)
            .try_into()
            .map_err(|_| HostError::InvalidMemoryAccess)?;
        let value = self
            .read_slice(&value)
            .try_into()
            .map_err(|_| HostError::InvalidMemoryAccess)?;

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
        method_ptr: u64,
        headers_ptr: u64,
        body_ptr: u64,
        register_id: u64,
    ) -> VMLogicResult<u32> {
        let url = unsafe { self.read_typed::<sys::Buffer<'_>>(url_ptr)? };
        let method = unsafe { self.read_typed::<sys::Buffer<'_>>(method_ptr)? };
        let headers = unsafe { self.read_typed::<sys::Buffer<'_>>(headers_ptr)? };
        let body = unsafe { self.read_typed::<sys::Buffer<'_>>(body_ptr)? };

        let url =
            String::from_utf8(self.read_slice(&url).to_vec()).map_err(|_| HostError::BadUTF8)?;
        let method =
            String::from_utf8(self.read_slice(&method).to_vec()).map_err(|_| HostError::BadUTF8)?;

        let headers = self.read_slice(&headers).to_vec();

        // Note: The `fetch` function cannot be directly called by applications.
        // Therefore, the headers are generated exclusively by our code, ensuring
        // that it is safe to deserialize them.
        let headers: Vec<(String, String)> =
            from_borsh_slice(&headers).map_err(|_| HostError::DeserializationError)?;

        let body = self.read_slice(&body).to_vec();
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

    pub fn random_bytes(&mut self, ptr: u64) -> VMLogicResult<()> {
        let byte_slice = unsafe { self.read_typed::<sys::BufferMut<'_>>(ptr)? };
        let mut buf = self.read_slice(&byte_slice).to_vec();

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
    pub fn time_now(&mut self, ptr: u64) -> VMLogicResult<()> {
        let time = unsafe { self.read_typed::<sys::BufferMut<'_>>(ptr)? };

        if time.len() != 8 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64;

        self.borrow_memory()
            .write(time.ptr().value().as_usize() as u64, &now.to_le_bytes())?;

        Ok(())
    }

    /// Call the contract's `send_proposal()` function through the bridge.
    ///
    /// The proposal actions are obtained as raw data and pushed onto a list of
    /// proposals to be sent to the host.
    ///
    /// Note that multiple actions are received, and the entire batch is pushed
    /// onto the proposal list to represent one proposal.
    ///
    /// # Parameters
    ///
    /// * `actions_ptr` - Pointer to the start of the action data in WASM
    ///                   memory.
    /// * `actions_len` - Length of the action data.
    /// * `id_ptr`      - Pointer to the start of the id data in WASM memory.
    /// * `id_len`      - Length of the action data. This should be 32 bytes.
    ///
    pub fn send_proposal(&mut self, actions_ptr: u64, id_ptr: u64) -> VMLogicResult<()> {
        let actions = unsafe { self.read_typed::<sys::Buffer<'_>>(actions_ptr)? };
        let id = unsafe { self.read_typed::<sys::BufferMut<'_>>(id_ptr)? };

        if id.len() != 32 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let actions_bytes = self.read_slice(&actions).to_vec();
        let mut proposal_id = [0; 32];

        rand::thread_rng().fill_bytes(&mut proposal_id);
        drop(self.with_logic_mut(|logic| logic.proposals.insert(proposal_id, actions_bytes)));

        self.borrow_memory()
            .write(id.ptr().value().as_usize() as u64, &proposal_id)?;

        Ok(())
    }

    pub fn approve_proposal(&mut self, approval_ptr: u64) -> VMLogicResult<()> {
        let approval = unsafe { self.read_typed::<sys::Buffer<'_>>(approval_ptr)? };
        let approval_len = approval.len();

        if approval_len != 32 {
            return Err(HostError::InvalidMemoryAccess.into());
        }
        let approval = self.read_guest_memory_sized::<32>(approval_ptr, approval_len)?;
        let _ = self.with_logic_mut(|logic| logic.approvals.push(approval));

        Ok(())
    }

    // ========== BLOB FUNCTIONS ==========

    /// Create a new blob for writing
    /// Returns: file descriptor (u64) for writing operations
    pub fn blob_create(&mut self) -> VMLogicResult<u64> {
        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        if self.borrow_logic().blob_handles.len()
            >= self.borrow_logic().limits.max_blob_handles as usize
        {
            return Err(VMLogicError::HostError(HostError::TooManyBlobHandles {
                max: self.borrow_logic().limits.max_blob_handles,
            }));
        }

        let fd = self.with_logic_mut(|logic| -> VMLogicResult<u64> {
            let Some(node_client) = logic.node_client.clone() else {
                return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
            };

            let fd = logic.next_blob_fd;
            logic.next_blob_fd += 1;

            let (data_sender, data_receiver) = mpsc::unbounded_channel();

            let completion_handle = tokio::spawn(async move {
                let stream = UnboundedReceiverStream::new(data_receiver);

                let byte_stream =
                    stream.map(|data: Vec<u8>| Ok::<bytes::Bytes, std::io::Error>(data.into()));
                let reader = byte_stream.into_async_read();

                node_client.add_blob(reader, None, None).await
            });

            let handle = BlobHandle::Write(BlobWriteHandle {
                sender: data_sender,
                completion_handle,
            });

            drop(logic.blob_handles.insert(fd, handle));
            Ok(fd)
        })?;

        Ok(fd)
    }

    /// Write a chunk of data to a blob
    /// Returns: number of bytes written (u64)
    pub fn blob_write(&mut self, fd: u64, data_ptr: u64) -> VMLogicResult<u64> {
        let data = unsafe { self.read_typed::<sys::Buffer<'_>>(data_ptr)? };
        let data_len = data.len();

        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        // Validate chunk size
        if data_len > self.borrow_logic().limits.max_blob_chunk_size {
            return Err(VMLogicError::HostError(HostError::BlobWriteTooLarge {
                size: data_len,
                max: self.borrow_logic().limits.max_blob_chunk_size,
            }));
        }

        let data = self.read_slice(&data).to_vec();

        self.with_logic_mut(|logic| {
            let handle = logic
                .blob_handles
                .get(&fd)
                .ok_or(VMLogicError::HostError(HostError::InvalidBlobHandle))?;

            match handle {
                BlobHandle::Write(_) => Ok(()),
                BlobHandle::Read(_) => Err(VMLogicError::HostError(HostError::InvalidBlobHandle)),
            }
        })?;

        self.with_logic_mut(|logic| {
            let handle = logic
                .blob_handles
                .get_mut(&fd)
                .ok_or(VMLogicError::HostError(HostError::InvalidBlobHandle))?;
            match handle {
                BlobHandle::Write(w) => {
                    w.sender
                        .send(data.clone())
                        .map_err(|_| VMLogicError::HostError(HostError::InvalidBlobHandle))?;
                }
                _ => return Err(VMLogicError::HostError(HostError::InvalidBlobHandle)),
            }
            Ok::<(), VMLogicError>(())
        })?;

        Ok(data_len)
    }

    /// Close a blob handle and get the resulting blob ID
    /// Returns: 1 on success
    pub fn blob_close(&mut self, fd: u64, blob_id_ptr: u64) -> VMLogicResult<u32> {
        let blob_id = unsafe { self.read_typed::<sys::BufferMut<'_>>(blob_id_ptr)? };
        let blob_id_ptr = blob_id.ptr().value().as_usize() as u64;

        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        if blob_id.len() != 32 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let handle = self.with_logic_mut(|logic| {
            logic
                .blob_handles
                .remove(&fd)
                .ok_or(VMLogicError::HostError(HostError::InvalidBlobHandle))
        })?;

        match handle {
            BlobHandle::Write(write_handle) => {
                drop(write_handle.sender);

                let (blob_id, _size) = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(write_handle.completion_handle)
                })
                .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?
                .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?;

                self.borrow_memory().write(blob_id_ptr, blob_id.as_ref())?;
            }
            BlobHandle::Read(read_handle) => {
                self.borrow_memory()
                    .write(blob_id_ptr, read_handle.blob_id.as_ref())?;
            }
        }

        Ok(1)
    }

    /// Announce a blob to a specific context for network discovery
    pub fn blob_announce_to_context(
        &mut self,
        blob_id_ptr: u64,
        context_id_ptr: u64,
    ) -> VMLogicResult<u32> {
        // Check if blob functionality is available
        let node_client = match &self.borrow_logic().node_client {
            Some(client) => client.clone(),
            None => return Err(VMLogicError::HostError(HostError::BlobsNotSupported)),
        };

        let blob_id = unsafe { self.read_typed::<sys::Buffer<'_>>(blob_id_ptr)? };
        let context_id = unsafe { self.read_typed::<sys::Buffer<'_>>(context_id_ptr)? };

        let blob_id_len = blob_id.ptr().value().as_usize() as u64;
        let context_id_len = context_id.ptr().value().as_usize() as u64;

        // Validate input lengths
        if blob_id_len != 32 || context_id_len != 32 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        // Read blob_id and context_id from memory
        let blob_id_bytes = self.read_guest_memory_sized::<32>(blob_id_ptr, blob_id_len)?;
        let context_id_bytes =
            self.read_guest_memory_sized::<32>(context_id_ptr, context_id_len)?;

        let blob_id = BlobId::from(blob_id_bytes);
        let context_id = calimero_primitives::context::ContextId::from(context_id_bytes);

        // Get blob metadata to get size
        let blob_info = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                node_client
                    .get_blob_info(blob_id)
                    .await
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))
            })
        })?;

        let blob_info =
            blob_info.ok_or_else(|| VMLogicError::HostError(HostError::BlobsNotSupported))?;

        // Announce blob to network
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                node_client
                    .announce_blob_to_network(&blob_id, &context_id, blob_info.size)
                    .await
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))
            })
        })?;

        Ok(1)
    }

    /// Open a blob for reading
    /// Returns: file descriptor (u64) for reading operations  
    pub fn blob_open(&mut self, blob_id_ptr: u64) -> VMLogicResult<u64> {
        let blob_id = unsafe { self.read_typed::<sys::Buffer<'_>>(blob_id_ptr)? };
        let blob_id_len = blob_id.len();

        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        if self.borrow_logic().blob_handles.len()
            >= self.borrow_logic().limits.max_blob_handles as usize
        {
            return Err(VMLogicError::HostError(HostError::TooManyBlobHandles {
                max: self.borrow_logic().limits.max_blob_handles,
            }));
        }

        if blob_id_len != 32 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let blob_id_bytes = self
            .read_guest_memory_sized::<32>(blob_id.ptr().value().as_usize() as u64, blob_id_len)?;
        let blob_id = BlobId::from(blob_id_bytes);

        let fd = self.with_logic_mut(|logic| {
            let fd = logic.next_blob_fd;
            logic.next_blob_fd += 1;

            let handle = BlobHandle::Read(BlobReadHandle {
                blob_id,
                stream: None,
                current_chunk_cursor: None,
                position: 0,
            });
            drop(logic.blob_handles.insert(fd, handle));
            fd
        });

        Ok(fd)
    }

    /// Read a chunk of data from a blob
    /// Returns: number of bytes read (u64)  
    pub fn blob_read(&mut self, fd: u64, data_ptr: u64) -> VMLogicResult<u64> {
        let data = unsafe { self.read_typed::<sys::BufferMut<'_>>(data_ptr)? };
        let data_len = data.len();

        // Check if blob functionality is available
        let node_client = match &self.borrow_logic().node_client {
            Some(client) => client.clone(),
            None => return Err(VMLogicError::HostError(HostError::BlobsNotSupported)),
        };

        // Validate buffer size
        if data_len > self.borrow_logic().limits.max_blob_chunk_size {
            return Err(VMLogicError::HostError(HostError::BlobBufferTooLarge {
                size: data_len,
                max: self.borrow_logic().limits.max_blob_chunk_size,
            }));
        }

        if data_len == 0 {
            return Ok(0);
        }

        let mut output_buffer = Vec::with_capacity(data_len as usize);

        let bytes_read = self.with_logic_mut(|logic| -> VMLogicResult<u64> {
            let handle = logic
                .blob_handles
                .get_mut(&fd)
                .ok_or(VMLogicError::HostError(HostError::InvalidBlobHandle))?;

            let read_handle = match handle {
                BlobHandle::Read(r) => r,
                BlobHandle::Write(_) => {
                    return Err(VMLogicError::HostError(HostError::InvalidBlobHandle))
                }
            };

            let needed = data_len as usize;

            // First, try to read from current chunk cursor if available
            if let Some(cursor) = &mut read_handle.current_chunk_cursor {
                let mut temp_buffer = vec![0u8; needed];
                match cursor.read(&mut temp_buffer) {
                    Ok(bytes_from_cursor) => {
                        output_buffer.extend_from_slice(&temp_buffer[..bytes_from_cursor]);

                        // If cursor is exhausted, remove it
                        if bytes_from_cursor == 0
                            || cursor.position() >= cursor.get_ref().len() as u64
                        {
                            read_handle.current_chunk_cursor = None;
                        }

                        // If we satisfied the request entirely from cursor, we're done
                        if output_buffer.len() >= needed {
                            read_handle.position += output_buffer.len() as u64;
                            return Ok(output_buffer.len() as u64);
                        }
                    }
                    Err(_) => {
                        // Cursor error, remove it
                        read_handle.current_chunk_cursor = None;
                    }
                }
            }

            if read_handle.stream.is_none() {
                let blob_stream = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        node_client
                            .get_blob(&read_handle.blob_id, None)
                            .await
                            .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))
                    })
                })?;

                match blob_stream {
                    Some(stream) => {
                        let mapped_stream = stream.map(|result| match result {
                            Ok(chunk) => Ok(bytes::Bytes::copy_from_slice(&chunk)),
                            Err(_) => Err(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "blob read error",
                            )),
                        });
                        read_handle.stream = Some(Box::new(mapped_stream));
                    }
                    None => {
                        read_handle.position += output_buffer.len() as u64;
                        return Ok(output_buffer.len() as u64);
                    }
                }
            }

            if let Some(stream) = &mut read_handle.stream {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        while output_buffer.len() < needed {
                            match stream.next().await {
                                Some(Ok(chunk)) => {
                                    let chunk_bytes = chunk.as_ref();
                                    let remaining_needed = needed - output_buffer.len();

                                    if chunk_bytes.len() <= remaining_needed {
                                        output_buffer.extend_from_slice(chunk_bytes);
                                    } else {
                                        // Use part of chunk, save rest in cursor for next time
                                        output_buffer
                                            .extend_from_slice(&chunk_bytes[..remaining_needed]);

                                        // Create cursor with remaining data
                                        let remaining_data =
                                            chunk_bytes[remaining_needed..].to_vec();
                                        read_handle.current_chunk_cursor =
                                            Some(Cursor::new(remaining_data));
                                        break;
                                    }
                                }
                                Some(Err(_)) | None => {
                                    break;
                                }
                            }
                        }
                        Ok::<(), VMLogicError>(())
                    })
                })?;
            }

            read_handle.position += output_buffer.len() as u64;
            Ok(output_buffer.len() as u64)
        })?;

        if bytes_read > 0 {
            self.borrow_memory()
                .write(data.ptr().value().as_usize() as u64, &output_buffer)?;
        }

        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Key, Value};
    use core::ops::Deref;
    use wasmer::{AsStoreMut, Memory, MemoryType, Store};

    // This implementation is more suitable for testing host-side components
    // in comparison to `store::MockedStorage` which is a better for guest-side
    // tests - e.g. testing Calimero application contracts.
    // This version minimally satisfies the `Storage` trait without introducing
    // the global state, guaranteed having a proper test isolation and not having a risk
    // to collide with other tests due to developer error, too (i.e. developer accidentally
    // used the same scope for the global state of `store::MockedStorage` in two different
    // tests).
    struct SimpleMockStorage {
        data: HashMap<Vec<u8>, Vec<u8>>,
    }

    impl SimpleMockStorage {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl Storage for SimpleMockStorage {
        fn get(&self, key: &Key) -> Option<Value> {
            self.data.get(key).cloned()
        }

        fn set(&mut self, key: Key, value: Value) -> Option<Value> {
            self.data.insert(key, value)
        }

        fn remove(&mut self, key: &Key) -> Option<Value> {
            self.data.remove(key)
        }

        fn has(&self, key: &Key) -> bool {
            self.data.contains_key(key)
        }
    }

    // A macro to set up the VM environment within a test.
    // It takes references to storage and limits, which are owned by the test function,
    // ensuring that all lifetimes are valid.
    macro_rules! setup_vm {
        ($storage:expr, $limits:expr, $input:expr) => {{
            let context = VMContext::new(Cow::Owned($input), [0u8; 32], [0u8; 32]);
            let mut store = Store::default();
            let memory = Memory::new(&mut store, MemoryType::new(1, None, false)).unwrap();
            let mut logic = VMLogic::new($storage, context, $limits, None);
            let _ = logic.with_memory(memory);
            (logic, store)
        }};
    }

    /// Helper to write a `sys::Buffer` struct representation to memory.
    /// Simulates a WASM guest preparing a memory descriptor for a host call.
    ///
    /// # Why this is necessary
    /// When a WASM guest needs the host to read/write a slice of its memory, it cannot
    /// pass a slice directly. Instead, it must pass a pointer to a "descriptor" structure
    /// that exists within guest's memory. This descriptor tells the host where the
    /// actual data is (`ptr`) and how long it is (`len`). This function simulates the guest
    /// writing that descriptor into the mock memory.
    ///
    /// # Parameters
    /// - `host`: A reference to the `VMHostFunctions` to get access to the guest memory view.
    /// - `offset`: The address **of the descriptor struct itself** in the guest memory.
    ///   This is the pointer that the guest would pass to the host function.
    /// - `ptr`: The address **of the actual data payload** (e.g., a string or byte array) in the
    ///   guest memory. This value is written inside the descriptor structure.
    /// - `len`: The length of the data payload. This value is also written inside the
    ///   descriptor structure.
    ///
    /// # ABI and Memory Layout
    /// Although the guest is `wasm32` and uses `u32` pointers internally, the host-guest
    /// ABI often standardizes on `u64` for all pointers and lengths for consistency and
    /// forward-compatibility with `wasm64`. Therefore, this function writes both `ptr` and `len`
    /// as `u64`, creating a 16-byte descriptor in memory with the layout `{ ptr: u64, len: u64 }`.
    fn prepare_guest_buf_descriptor(host: &VMHostFunctions<'_>, offset: u64, ptr: u64, len: u64) {
        let data: Vec<u8> = [ptr.to_le_bytes(), len.to_le_bytes()].concat();

        host.borrow_memory()
            .write(offset, &data)
            .expect("Failed to write buffer");
    }

    // Helper to write a string to memory.
    fn write_str(host: &VMHostFunctions<'_>, offset: u64, s: &str) {
        host.borrow_memory()
            .write(offset, s.as_bytes())
            .expect("Failed to write string");
    }

    #[test]
    fn test_default_limits() {
        let limits = VMLimits::default();
        assert_eq!(limits.max_memory_pages, 1 << 10);
        assert_eq!(limits.max_stack_size, 200 << 10);
        assert_eq!(*limits.max_register_size.deref(), 100 << 20);
    }

    #[test]
    fn test_input() {
        let input = vec![1u8, 2, 3];
        let input_len = input.len() as u64;
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, input.clone());

        {
            let mut host = logic.host_functions(store.as_store_mut());
            let register_id = 1u64;

            host.input(register_id).expect("Input call failed");
            assert_eq!(host.register_len(register_id).unwrap(), input_len);

            let buf_ptr = 100u64;
            let data_output_ptr = 200u64;
            prepare_guest_buf_descriptor(&host, buf_ptr, data_output_ptr, input_len);

            let res = host.read_register(register_id, buf_ptr).unwrap();
            assert_eq!(res, 1);

            let mut mem_buffer = vec![0u8; input_len as usize];
            host.borrow_memory()
                .read(data_output_ptr, &mut mem_buffer)
                .unwrap();
            assert_eq!(mem_buffer, input);
        }
    }

    #[test]
    fn test_log_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "test log";
        let msg_ptr = 200u64;
        write_str(&host, msg_ptr, msg);

        let buf_ptr = 10u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, msg_ptr, msg.len() as u64);
        host.log_utf8(buf_ptr).expect("Log failed");

        assert_eq!(host.borrow_logic().logs.len(), 1);
        assert_eq!(host.borrow_logic().logs[0], "test log");
    }

    #[test]
    fn test_log_utf8_overflow() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "log";
        let msg_ptr = 200u64;
        write_str(&host, msg_ptr, msg);
        let buf_ptr = 10u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, msg_ptr, msg.len() as u64);

        for _ in 0..100 {
            host.log_utf8(buf_ptr).expect("Log failed");
        }
        let err = host.log_utf8(buf_ptr).unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::LogsOverflow)
        ));
    }

    fn test_panic_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let context = VMContext::new(Cow::Owned(vec![]), [0u8; 32], [0u8; 32]);
        let mut logic = VMLogic::new(&mut storage, context, &limits, None);

        let mut store = Store::default();
        let memory = Memory::new(&mut store, MemoryType::new(1, None, false)).unwrap();
        logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "panic message";
        let msg_ptr = 200u64;
        write_str(&host, msg_ptr, msg);
        let msg_buf_ptr = 16u64; // Use aligned address
        prepare_guest_buf_descriptor(&host, msg_buf_ptr, msg_ptr, msg.len() as u64);

        let file = "file.rs";
        let file_ptr = 400u64;
        write_str(&host, file_ptr, file);

        let loc_data_ptr = 304u64; // Use aligned address
        prepare_guest_buf_descriptor(&host, loc_data_ptr, file_ptr, file.len() as u64);

        let line: u32 = 10;
        let column: u32 = 5;
        host.borrow_memory()
            .write(loc_data_ptr + 16, &line.to_le_bytes())
            .unwrap();
        host.borrow_memory()
            .write(loc_data_ptr + 20, &column.to_le_bytes())
            .unwrap();

        let err = host.panic_utf8(msg_buf_ptr, loc_data_ptr).unwrap_err();
        match err {
            VMLogicError::HostError(HostError::Panic {
                message, location, ..
            }) => {
                assert_eq!(message, "panic message");
                match location {
                    Location::At { file, line, column } => {
                        assert_eq!(file, "file.rs");
                        assert_eq!(line, 10);
                        assert_eq!(column, 5);
                    }
                    _ => panic!("Unexpected location variant"),
                }
            }
            _ => panic!("Unexpected error variant"),
        }
    }

    #[test]
    fn test_storage_write_read() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "key";
        let key_ptr = 200u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 10u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let value = "value";
        let value_ptr = 300u64;
        write_str(&host, value_ptr, value);
        let value_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

        let register_id = 1u64;
        let res = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap();
        assert_eq!(res, 0);

        let res = host.storage_read(key_buf_ptr, register_id).unwrap();
        assert_eq!(res, 1);
        assert_eq!(host.register_len(register_id).unwrap(), value.len() as u64);
    }

    #[test]
    fn test_random_bytes() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let buf_ptr = 10u64;
        let data_ptr = 200u64;
        let data_len = 32u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, data_len);

        // Call the host function to fill the buffer
        host.random_bytes(buf_ptr).unwrap();

        // Read the bytes back from guest memory
        let mut random_data = vec![0u8; data_len as usize];
        host.borrow_memory()
            .read(data_ptr, &mut random_data)
            .unwrap();

        // Verify that the buffer is not empty and doesn't contain all zeros.
        assert!(!random_data.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_time_now() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let buf_ptr = 16u64;
        let time_data_ptr = 200u64;
        // The `time_now()` function expects an 8-byte buffer to write the u64 timestamp.
        let time_data_len = 8u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, time_data_ptr, time_data_len);

        let time_before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Call the host function
        host.time_now(buf_ptr).unwrap();

        let time_after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Read the timestamp back from guest memory
        let mut time_buffer = [0u8; 8];
        host.borrow_memory()
            .read(time_data_ptr, &mut time_buffer)
            .unwrap();
        let timestamp_from_host = u64::from_le_bytes(time_buffer);

        // Verify the timestamp is current and valid
        assert!(timestamp_from_host >= time_before);
        assert!(timestamp_from_host <= time_after);
    }

    #[test]
    fn test_blob_create_without_client() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());
        let err = host.blob_create().unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::BlobsNotSupported)
        ));
    }

    #[test]
    fn test_smoke_finish_with_error() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (logic, _) = setup_vm!(&mut storage, &limits, vec![]);
        let outcome = logic.finish(Some(FunctionCallError::ExecutionError(vec![])));
        assert!(outcome.returns.is_err());
    }
}
