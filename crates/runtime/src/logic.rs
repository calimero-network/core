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
use calimero_primitives::context::ContextId;
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
    #[allow(
        clippy::mut_from_ref,
        reason = "We need to be able to modify self while referencing self"
    )]
    fn read_slice(&self, slice: &sys::Buffer<'_>) -> &mut [u8] {
        let ptr = slice.ptr().value().as_usize();
        let len = slice.len() as usize;

        unsafe { &mut self.borrow_memory().data_unchecked_mut()[ptr..ptr + len] }
    }

    fn read_str(&self, slice: &sys::Buffer<'_>) -> VMLogicResult<&mut str> {
        let buf = self.read_slice(slice);

        std::str::from_utf8_mut(buf).map_err(|_| HostError::BadUTF8.into())
    }

    fn read_sized<const N: usize>(&self, slice: &sys::Buffer<'_>) -> VMLogicResult<&mut [u8; N]> {
        let buf = self.read_slice(slice);

        buf.try_into()
            .map_err(|_| HostError::InvalidMemoryAccess.into())
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

        let file = self.read_str(&location.file())?.to_owned();
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

        let message = self.read_str(&message_buf)?.to_owned();
        let file = self.read_str(&location.file())?.to_owned();
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

        self.read_slice(&register).copy_from_slice(data);

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

        let logic = self.borrow_logic();

        if logic.logs.len()
            >= usize::try_from(logic.limits.max_logs).map_err(|_| HostError::IntegerOverflow)?
        {
            return Err(HostError::LogsOverflow.into());
        }

        let message = self.read_str(&buf)?.to_owned();

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

        let kind = self.read_str(event.kind())?.to_owned();
        let data = self.read_slice(event.data()).to_vec();

        self.with_logic_mut(|logic| logic.events.push(Event { kind, data }));

        Ok(())
    }

    pub fn commit(&mut self, root_hash_ptr: u64, artifact_ptr: u64) -> VMLogicResult<()> {
        let root_hash = unsafe { self.read_typed::<sys::Buffer<'_>>(root_hash_ptr)? };
        let artifact = unsafe { self.read_typed::<sys::Buffer<'_>>(artifact_ptr)? };

        let root_hash = *self.read_sized::<32>(&root_hash)?;
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

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }
        let key = self.read_slice(&key).to_vec();

        if let Some(value) = logic.storage.get(&key) {
            self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, value))?;

            return Ok(1);
        }

        Ok(0)
    }

    pub fn storage_remove(&mut self, key_ptr: u64, register_id: u64) -> VMLogicResult<u32> {
        let key = unsafe { self.read_typed::<sys::Buffer<'_>>(key_ptr)? };

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        let key = self.read_slice(&key).to_vec();

        if let Some(value) = logic.storage.get(&key) {
            self.with_logic_mut(|logic| {
                let _ignored = logic.storage.remove(&key);
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

        let value = unsafe { self.read_typed::<sys::Buffer<'_>>(value_ptr)? };

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        if value.len() > logic.limits.max_storage_value_size.get() {
            return Err(HostError::ValueLengthOverflow.into());
        }

        let key = self.read_slice(&key).to_vec();
        let value = self.read_slice(&value).to_vec();

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

        let url = self.read_str(&url)?;
        let method = self.read_str(&method)?;

        let headers = self.read_slice(&headers);
        let body = self.read_slice(&body);

        // Note: The `fetch` function cannot be directly called by applications.
        // Therefore, the headers are generated exclusively by our code, ensuring
        // that it is safe to deserialize them.
        let headers: Vec<(String, String)> =
            from_borsh_slice(headers).map_err(|_| HostError::DeserializationError)?;

        let mut request = ureq::request(&method, &url);

        for (key, value) in &headers {
            request = request.set(key, value);
        }

        let response = if body.is_empty() {
            request.call()
        } else {
            request.send_bytes(body)
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
        let buf = unsafe { self.read_typed::<sys::BufferMut<'_>>(ptr)? };

        rand::thread_rng().fill_bytes(self.read_slice(&buf));

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

        let time = self.read_sized::<8>(&time)?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64;

        *time = now.to_le_bytes();

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
        let id = unsafe { self.read_typed::<sys::BufferMut<'_>>(id_ptr)? };
        let actions = unsafe { self.read_typed::<sys::Buffer<'_>>(actions_ptr)? };

        let id = self.read_sized::<32>(&id)?;

        rand::thread_rng().fill_bytes(id);

        let id = *id;

        let actions = self.read_slice(&actions).to_vec();

        let _ignored = self.with_logic_mut(|logic| logic.proposals.insert(id, actions));
        Ok(())
    }

    pub fn approve_proposal(&mut self, approval_ptr: u64) -> VMLogicResult<()> {
        let approval = unsafe { self.read_typed::<sys::Buffer<'_>>(approval_ptr)? };
        let approval = *self.read_sized::<32>(&approval)?;

        let _ignored = self.with_logic_mut(|logic| logic.approvals.push(approval));
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

        let blob_id = self.read_sized::<32>(&blob_id)?;

        match handle {
            BlobHandle::Write(write_handle) => {
                let _ignored = write_handle.sender;

                let (blob_id_, _size) = tokio::runtime::Handle::current()
                    .block_on(write_handle.completion_handle)
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?;

                *blob_id = *blob_id_;
            }
            BlobHandle::Read(read_handle) => *blob_id = *read_handle.blob_id,
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

        let blob_id = BlobId::from(*self.read_sized::<32>(&blob_id)?);
        let context_id = ContextId::from(*self.read_sized::<32>(&context_id)?);

        // Get blob metadata to get size
        let blob_info = tokio::runtime::Handle::current()
            .block_on(node_client.get_blob_info(blob_id))
            .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?
            .ok_or_else(|| VMLogicError::HostError(HostError::BlobsNotSupported))?;
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

        let blob_id = BlobId::from(*self.read_sized::<32>(&blob_id)?);

        let fd = self.with_logic_mut(|logic| {
            let fd = logic.next_blob_fd;
            logic.next_blob_fd += 1;

            let handle = BlobHandle::Read(BlobReadHandle {
                blob_id,
                stream: None,
                current_chunk_cursor: None,
                position: 0,
            });
            let _ignored = logic.blob_handles.insert(fd, handle);
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
                let blob_stream = tokio::runtime::Handle::current()
                    .block_on(node_client.get_blob(&read_handle.blob_id, None))
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?;

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
                                    let remaining_data = chunk_bytes[remaining_needed..].to_vec();
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
                })?;
            }

            read_handle.position += output_buffer.len() as u64;
            Ok(output_buffer.len() as u64)
        })?;

        if bytes_read > 0 {
            self.read_slice(&data).copy_from_slice(&output_buffer);
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

    // The descriptor has the size of 16-bytes with the layout `{ ptr: u64, len: u64 }`
    // See below: [`prepare_guest_buf_descriptor`]
    const DESCRIPTOR_SIZE: usize = u64::BITS as usize / 8 * 2;

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

    /// Helper to write a similar to `sys::Buffer` struct representation to memory.
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
    /// - `offset`: The address of the descriptor struct itself in the guest memory.
    ///   This is the pointer that the guest would pass to the host function.
    /// - `ptr`: The address of the actual data payload (e.g., a string or byte array) in the
    ///   guest memory. This value is written inside the descriptor structure.
    /// - `len`: The length of the data payload. This value is also written inside the
    ///   descriptor structure.
    ///
    /// # ABI and Memory Layout
    /// Although the guest is `wasm32` and uses `u32` pointers internally, the host-guest
    /// ABI often standardizes on `u64` for all pointers and lengths for consistency and
    /// forward-compatibility with `wasm64`. Therefore, this function writes both `ptr` and `len`
    /// as `u64`, creating a 16-byte descriptor in memory with the layout `{ ptr: u64, len: u64 }`.
    /// All values are little-endian, as required by the WebAssembly specification.
    fn prepare_guest_buf_descriptor(host: &VMHostFunctions<'_>, offset: u64, ptr: u64, len: u64) {
        let data: Vec<u8> = [ptr.to_le_bytes(), len.to_le_bytes()].concat();

        host.borrow_memory()
            .write(offset, &data)
            .expect("Failed to write buffer");
    }

    /// A test helper to write a string slice directly into the guest's mock memory.
    ///
    /// This simulates the guest having string data (e.g., a log message, a storage key)
    /// in its linear memory, making it available for the host to read.
    ///
    /// # Parameters
    /// - `host`: A reference to the `VMHostFunctions` to get access to the guest memory view.
    /// - `offset`: The memory address where the string's byte data will be written.
    /// - `s`: The string slice to write into the guest's memory.
    fn write_str(host: &VMHostFunctions<'_>, offset: u64, s: &str) {
        host.borrow_memory()
            .write(offset, s.as_bytes())
            .expect("Failed to write string");
    }

    /// A simple sanity check to ensure the default `VMLimits` are configured as expected.
    /// This test helps prevent accidental changes to the default limits.
    #[test]
    fn test_default_limits() {
        let limits = VMLimits::default();
        assert_eq!(limits.max_memory_pages, 1 << 10);
        assert_eq!(limits.max_stack_size, 200 << 10);
        assert_eq!(limits.max_registers, 100);
        assert_eq!(*limits.max_register_size.deref(), 100 << 20);
        assert_eq!(limits.max_registers_capacity, 1 << 30); // 1 GiB
        assert_eq!(limits.max_logs, 100);
        assert_eq!(limits.max_log_size, 16 << 10); // 16 KiB
        assert_eq!(limits.max_events, 100);
        assert_eq!(limits.max_event_kind_size, 100);
        assert_eq!(limits.max_event_data_size, 16 << 10); // 16 KiB
        assert_eq!(limits.max_storage_key_size.get(), 1 << 20); // 1 MiB
        assert_eq!(limits.max_storage_value_size.get(), 10 << 20); // 10 MiB
        assert_eq!(limits.max_blob_handles, 100);
        assert_eq!(limits.max_blob_chunk_size, 10 << 20); // 10 MiB
    }

    /// Tests the `input()`, `register_len()`, `read_register()` host functions.
    #[test]
    fn test_input_and_basic_registers_api() {
        let input = vec![1u8, 2, 3];
        let input_len = input.len() as u64;
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, input.clone());

        {
            let mut host = logic.host_functions(store.as_store_mut());
            let register_id = 1u64;

            // Guest: load the context data into a host-side register.
            host.input(register_id).expect("Input call failed");
            // Guest: verify the byte length of the host-side register's data matches the input length.
            assert_eq!(host.register_len(register_id).unwrap(), input_len);

            let buf_ptr = 100u64;
            let data_output_ptr = 200u64;
            // Guest: prepare the descriptor for the destination buffer so host can write there.
            prepare_guest_buf_descriptor(&host, buf_ptr, data_output_ptr, input_len);

            // Guest: read the register from the host into `buf_ptr`.
            let res = host.read_register(register_id, buf_ptr).unwrap();
            // Guest: assert the host successfully wrote the data from its register to our `buf_ptr`.
            assert_eq!(res, 1);

            let mut mem_buffer = vec![0u8; input_len as usize];
            // Host: perform a priveleged read of the contents of guest's memory to verify it
            // matches the `input`.
            host.borrow_memory()
                .read(data_output_ptr, &mut mem_buffer)
                .unwrap();
            assert_eq!(mem_buffer, input);
        }
    }

    /// Tests the `context_id()` and `executor_id()` host functions.
    ///
    /// This test verifies that the guest can request and receive context and executor IDs.
    #[test]
    fn test_context_and_executor_id() {
        let context_id = [3u8; 32];
        let executor_id = [5u8; 32];
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let context = VMContext::new(Cow::Owned(vec![]), context_id, executor_id);
        let mut logic = VMLogic::new(&mut storage, context, &limits, None);

        let mut store = Store::default();
        let memory = Memory::new(&mut store, MemoryType::new(1, None, false)).unwrap();
        let _ = logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let context_id_register = 1;
        // Guest: ask the host to put the context ID into host register
        // that has a value `context_id_register`.
        host.context_id(context_id_register).unwrap();
        // Very the `context_id` is correctly written into its host-side register.
        let requested_context_id = host
            .borrow_logic()
            .registers
            .get(context_id_register)
            .unwrap();
        assert_eq!(requested_context_id, context_id);

        let executor_id_register = 2;
        // Guest: ask the host to put the executor ID into host register
        // that has a value `executor_id_register`.
        host.executor_id(executor_id_register).unwrap();
        // Verify the `executor_id` is correctly written into its host-side register.
        let requested_executor_id = host
            .borrow_logic()
            .registers
            .get(executor_id_register)
            .unwrap();
        assert_eq!(requested_executor_id, executor_id);
    }

    /// Tests the `value_return()` host function for both `Ok` and `Err` variants.
    ///
    /// This test verifies the primary mechanism for a guest to finish its execution
    /// and return a final value to the host. It checks that both successful (`Ok`) and
    /// unsuccessful (`Err`) return values are correctly stored in the `VMLogic` state.
    #[test]
    fn test_value_return() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Test returning an Ok value
        let ok_value = "this is Ok value";
        let ok_value_ptr = 200u64;
        // Guest: write ok
        write_str(&host, ok_value_ptr, ok_value);

        // Write a `sys::ValueReturn::Ok` enum representation (0) to memory.
        // The value then is followed by the buffer.
        let ok_discriminant = 0u8;
        let ok_return_ptr = 32u64;
        host.borrow_memory()
            .write(ok_return_ptr, &[ok_discriminant])
            .unwrap();
        // Guest: prepare the descriptor for the buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            ok_return_ptr + 8,
            ok_value_ptr,
            ok_value.len() as u64,
        );

        // Guest: ask host to read the return value.
        host.value_return(ok_return_ptr).unwrap();
        let returned_ok_value = host.borrow_logic().returns.clone().unwrap().unwrap();
        let returned_ok_value_str = std::str::from_utf8(&returned_ok_value).unwrap();
        // Verify the returned value matches the one from the guest.
        assert_eq!(returned_ok_value_str, ok_value);

        // Test returning an Err value
        let err_value = "this is Err value";
        let err_value_ptr = 400u64;
        write_str(&host, err_value_ptr, err_value);

        // Write a `sys::ValueReturn::Ok` enum representation (1) to memory.
        // The value then is followed by the buffer.
        let err_discriminant = 1u8;
        let err_return_ptr = 64u64;
        host.borrow_memory()
            .write(err_return_ptr, &[err_discriminant])
            .unwrap();
        // Guest: prepare the descriptor for the buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            err_return_ptr + 8,
            err_value_ptr,
            err_value.len() as u64,
        );

        // Guest: ask host to read the return value.
        host.value_return(err_return_ptr).unwrap();
        let returned_err_value = host.borrow_logic().returns.clone().unwrap().unwrap_err();
        let returned_err_value_str = std::str::from_utf8(&returned_err_value).unwrap();
        // Verify the returned value matches the one from the guest.
        assert_eq!(returned_err_value_str, err_value);
    }

    /// Tests the `log_utf8()` host function for a successful log operation.
    #[test]
    fn test_log_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "test log";
        let msg_ptr = 200u64;
        // Guest: write msg to its memory.
        write_str(&host, msg_ptr, msg);

        let buf_ptr = 10u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, msg_ptr, msg.len() as u64);
        // Guest: ask the host to log the contents of `buf_ptr`'s descriptor.
        host.log_utf8(buf_ptr).expect("Log failed");

        // Guest: verify the host successfully logged the message
        assert_eq!(host.borrow_logic().logs.len(), 1);
        assert_eq!(host.borrow_logic().logs[0], "test log");
    }

    /// Tests that the `log_utf8()` host function correctly handles the log limit and properly returns
    /// an error `HostError::LogOverflow` when the logs limit is exceeded.
    #[test]
    fn test_log_utf8_overflow() {
        let mut storage = SimpleMockStorage::new();
        let mut limits = VMLimits::default();
        limits.max_logs = 5;
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "log";
        let msg_ptr = 200u64;
        // Guest: write msg to its memory.
        write_str(&host, msg_ptr, msg);
        let buf_ptr = 10u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, msg_ptr, msg.len() as u64);

        // Guest: ask the host to log for a max limit of logs
        for _ in 0..limits.max_logs {
            host.log_utf8(buf_ptr).expect("Log failed");
        }

        // Guest: verify the host successfully logged `limits.max_logs` msgs.
        assert_eq!(host.borrow_logic().logs.len(), limits.max_logs as usize);
        // Guest: do over-the limit log
        let err = host.log_utf8(buf_ptr).unwrap_err();
        // Guest: verify the host didn't log over the limit and returned an error.
        assert_eq!(host.borrow_logic().logs.len(), limits.max_logs as usize);
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::LogsOverflow)
        ));
    }

    /// Tests that the `log_utf8()` host function correctly handles the bad UTF8 and properly returns
    /// an error `HostError::BadUTF8` when the incorrect string is provided (the failure occurs
    /// because of the verification happening inside the private `read_str` function).
    #[test]
    fn test_log_utf8_with_bad_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Prepare invalid UTF-8 bytes in guest memory.
        let invalid_utf8: &[u8] = &[0, 159, 146, 150];
        let data_ptr = 200u64;
        host.borrow_memory().write(data_ptr, invalid_utf8).unwrap();

        let buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, invalid_utf8.len() as u64);

        // `log_utf8` calls `read_str` internally. We expect it to fail.
        let err = host.log_utf8(buf_ptr).unwrap_err();
        assert!(matches!(err, VMLogicError::HostError(HostError::BadUTF8)));
    }

    /// Tests the `panic()` host function (without a custom message).
    #[test]
    fn test_panic() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let expected_file_name = "simple_panic.rs";
        let file_ptr = 400u64;
        // Guest: write file name to its memory.
        write_str(&host, file_ptr, expected_file_name);

        let loc_data_ptr = 300u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(
            &host,
            loc_data_ptr,
            file_ptr,
            expected_file_name.len() as u64,
        );

        let expected_line: u32 = 10;
        let expected_column: u32 = 5;
        let u32_size: u64 = (u32::BITS / 8).into();
        // Host: perform a priveleged write to the contents of guest's memory with a line and column
        // of the expected panic message. We write the `line` after the descriptor, and the `column` -
        // after the `line`.
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64,
                &expected_line.to_le_bytes(),
            )
            .unwrap();
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64 + u32_size,
                &expected_column.to_le_bytes(),
            )
            .unwrap();

        // Guest: ask the host to panic with the given location data.
        let err = host.panic(loc_data_ptr).unwrap_err();
        // Guest: assert the host panics with a "explicit panic" message, and `Location` (consisting
        // of file name, line, and column).
        match err {
            VMLogicError::HostError(HostError::Panic {
                message, location, ..
            }) => {
                assert_eq!(message, "explicit panic");
                match location {
                    Location::At { file, line, column } => {
                        assert_eq!(file, expected_file_name);
                        assert_eq!(line, expected_line);
                        assert_eq!(column, expected_column);
                    }
                    _ => panic!("Unexpected location variant"),
                }
            }
            _ => panic!("Unexpected error variant"),
        }
    }

    /// Tests the `panic_utf8()` host function.
    #[test]
    fn test_panic_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let expected_msg = "panic message";
        let msg_ptr = 200u64;
        // Guest: write msg to its memory.
        write_str(&host, msg_ptr, expected_msg);
        let msg_buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, msg_buf_ptr, msg_ptr, expected_msg.len() as u64);

        let expected_file_name = "file.rs";
        let file_ptr = 400u64;
        // Guest: write file name to its memory.
        write_str(&host, file_ptr, expected_file_name);

        let loc_data_ptr = 300u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(
            &host,
            loc_data_ptr,
            file_ptr,
            expected_file_name.len() as u64,
        );

        let expected_line: u32 = 10;
        let expected_column: u32 = 5;
        let u32_size: u64 = (u32::BITS / 8).into();
        // Host: perform a priveleged write to the contents of guest's memory with a line and column
        // of the expected panic message. We write the `line` after the descriptor, and the `column` -
        // after the `line`.
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64,
                &expected_line.to_le_bytes(),
            )
            .unwrap();
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64 + u32_size,
                &expected_column.to_le_bytes(),
            )
            .unwrap();

        // Guest: ask the host to panic with the given msg and location.
        let err = host.panic_utf8(msg_buf_ptr, loc_data_ptr).unwrap_err();
        // Guest: assert the host panics with a specified panic message, and `Location` (consisting
        // of file name, line, and column).
        match err {
            VMLogicError::HostError(HostError::Panic {
                message, location, ..
            }) => {
                assert_eq!(message, expected_msg);
                match location {
                    Location::At { file, line, column } => {
                        assert_eq!(file, expected_file_name);
                        assert_eq!(line, expected_line);
                        assert_eq!(column, expected_column);
                    }
                    _ => panic!("Unexpected location variant"),
                }
            }
            _ => panic!("Unexpected error variant"),
        }
    }

    /// Tests the `emit()` host function for event creation and events overflow.
    #[test]
    fn test_emit_and_events_overflow() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Prepare a valid event
        let kind = "my-event";
        let data = vec![1, 2, 3];
        let kind_ptr = 200u64;
        let data_ptr = 300u64;
        // Guest: write msg to its memory.
        write_str(&host, kind_ptr, kind);
        host.borrow_memory().write(data_ptr, &data).unwrap();

        // Prepare the sys::Event struct in memory.
        let event_struct_ptr = 48u64;
        let kind_buf_ptr = event_struct_ptr;
        let data_buf_ptr = event_struct_ptr + DESCRIPTOR_SIZE as u64;
        prepare_guest_buf_descriptor(&host, kind_buf_ptr, kind_ptr, kind.len() as u64);
        prepare_guest_buf_descriptor(&host, data_buf_ptr, data_ptr, data.len() as u64);

        // Guest: ask host to emit the event located at `event_struct_ptr`.
        host.emit(event_struct_ptr).unwrap();
        // Test successful event emission
        assert_eq!(host.borrow_logic().events.len(), 1);
        assert_eq!(host.borrow_logic().events[0].kind, kind);
        assert_eq!(host.borrow_logic().events[0].data, data);

        // Test events overflow
        for _ in 1..limits.max_events {
            host.emit(event_struct_ptr).unwrap();
        }
        assert_eq!(host.borrow_logic().events.len() as u64, limits.max_events);
        // Guest: ask the host to do over the limit event emission.
        let err = host.emit(event_struct_ptr).unwrap_err();
        // Guest: verify the host didn't emit over the limit and returned an error.
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::EventsOverflow)
        ));
    }

    /// Tests the `commit()` host function.
    #[test]
    fn test_commit() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let root_hash = [1u8; 32];
        let artifact = vec![1, 2, 3];
        let root_hash_ptr = 200u64;
        let artifact_ptr = 300u64;
        host.borrow_memory()
            .write(root_hash_ptr, &root_hash)
            .unwrap();
        host.borrow_memory().write(artifact_ptr, &artifact).unwrap();

        let root_hash_buf_ptr = 16u64;
        let artifact_buf_ptr = 32u64;
        // Guest: prepare the descriptor for the root_hash and artifact buffers so host can access them.
        prepare_guest_buf_descriptor(
            &host,
            root_hash_buf_ptr,
            root_hash_ptr,
            root_hash.len() as u64,
        );
        prepare_guest_buf_descriptor(&host, artifact_buf_ptr, artifact_ptr, artifact.len() as u64);

        // Guest: ask host to commit with the given root hash and artifact.
        host.commit(root_hash_buf_ptr, artifact_buf_ptr).unwrap();
        // Verify the host successfully stored the root hash and artifact in the `VMLogic` state.
        assert_eq!(host.borrow_logic().root_hash, Some(root_hash));
        assert_eq!(host.borrow_logic().artifact, artifact);
    }

    /// Tests the basic `storage_write` and `storage_read` host functions.
    #[test]
    fn test_storage_write_read() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "key";
        let key_ptr = 200u64;
        // Guest: write `key` to its memory.
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 10u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let value = "value";
        let value_ptr = 300u64;
        // Guest: write `value` to its memory.
        write_str(&host, value_ptr, value);
        let value_buf_ptr = 32u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

        let register_id = 1u64;
        // Guest: as host to write key-value pair to host storage.
        let res = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap();
        // Guest: verify the storage writing was successful.
        assert_eq!(res, 0);

        // Guest: ask the host to read from it's storage with a key located at `key_buf_ptr` and
        // put the result into `register_id`.
        let res = host.storage_read(key_buf_ptr, register_id).unwrap();
        // Ensure, the storage read was successful
        assert_eq!(res, 1);
        // Verify that the register length has the proper size
        assert_eq!(host.register_len(register_id).unwrap(), value.len() as u64);

        // Guest: ask the host to read the register and verify that the register has the proper
        // content after the `storage_read()` successfully exectued.
        let buf_ptr = 400u64;
        let data_output_ptr = 500u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_output_ptr, value.len() as u64);

        // Guest: read the register from the host into `buf_ptr`.
        let res = host.read_register(register_id, buf_ptr).unwrap();
        // Guest: assert the host successfully wrote the data from its register to our `buf_ptr`.
        assert_eq!(res, 1);

        let mut mem_buffer = vec![0u8; value.len()];
        // Host: perform a priveleged read of the contents of guest's memory to verify it
        // matches the `value`.
        host.borrow_memory()
            .read(data_output_ptr, &mut mem_buffer)
            .unwrap();
        let mem_buffer_str = std::str::from_utf8(&mem_buffer).unwrap();
        // Verify that the value from the register, after the successfull `storage_read()`
        // operation matches the same `value` when we initially wrote to the storage.
        assert_eq!(mem_buffer_str, value);
    }

    /// Tests the `storage_remove()` host function.
    #[test]
    fn test_storage_remove() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "to_remove";
        let value = "old_value";
        // Manually write into host storage for simplicity reasons.
        let _unused = host.with_logic_mut(|logic| {
            logic
                .storage
                .set(key.as_bytes().to_vec(), value.as_bytes().to_vec())
        });

        let key_ptr = 100u64;
        // Guest: write key to its memory
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let register_id = 1u64;
        // Guest: ask host to remove from storage the value with the given key.
        let res = host.storage_remove(key_buf_ptr, register_id).unwrap();
        // Verify the storage removal was successful.
        assert_eq!(res, 1);
        // Verify the storage doesn't have a specified key anymore.
        assert_eq!(
            host.borrow_logic().storage.has(&key.as_bytes().to_vec()),
            false
        );
        // Verify the removed value was put into the host register.
        assert_eq!(
            host.borrow_logic().registers.get(register_id).unwrap(),
            value.as_bytes()
        );

        // Verify the host register contains the removed value.
        let buf_ptr = 200u64;
        let data_output_ptr = 300u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_output_ptr, value.len() as u64);

        // Guest: read the register from the host into `buf_ptr`.
        let res = host.read_register(register_id, buf_ptr).unwrap();
        // Guest: assert the host successfully wrote the data from its register to our `buf_ptr`.
        assert_eq!(res, 1);

        let mut mem_buffer = vec![0u8; value.len()];
        // Host: perform a priveleged read of the contents of guest's memory to verify it
        // matches the `value`.
        host.borrow_memory()
            .read(data_output_ptr, &mut mem_buffer)
            .unwrap();
        assert_eq!(std::str::from_utf8(&mem_buffer).unwrap(), value);
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

        // Explicitly fill the memory with some pattern before the host call.
        // This makes the test deterministic (for CI) and ensures it fails
        // correctly if the function under this test does not write to the buffer.
        let initial_pattern = vec![0xAB; data_len as usize];
        host.borrow_memory()
            .write(data_ptr, &initial_pattern)
            .unwrap();

        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, data_len);

        // Guest: ask host to fill the buffer with random bytes
        host.random_bytes(buf_ptr).unwrap();

        // Host: perform a priveleged read of the contents of guest's memory to extract the buffer
        // back.
        let mut random_data = vec![0u8; data_len as usize];
        host.borrow_memory()
            .read(data_ptr, &mut random_data)
            .unwrap();

        // Assert that the memory content has changed from our initial pattern.
        assert_ne!(
            random_data, initial_pattern,
            "The data buffer should have been overwritten with random bytes, but it was not."
        );
    }

    /// Tests the `time_now()` host function.
    #[test]
    fn test_time_now() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let buf_ptr = 16u64;
        let time_data_ptr = 200u64;
        // The `time_now()` function expects an 8-byte buffer to write the u64 timestamp.
        let time_data_len = u64::BITS as u64 / 8;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, time_data_ptr, time_data_len);

        // Record the host's system time before the host-function call.
        let time_before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Guest: ask the host to provide the current timestamp and write it to the buffer.
        host.time_now(buf_ptr).unwrap();

        // Record the host's system time after the host-function call.
        let time_after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Host: read the timestamp back from guest memory.
        let mut time_buffer = [0u8; 8];
        host.borrow_memory()
            .read(time_data_ptr, &mut time_buffer)
            .unwrap();
        let timestamp_from_host = u64::from_le_bytes(time_buffer);

        // Verify the timestamp is current and valid (within the before-after range).
        assert!(timestamp_from_host >= time_before);
        assert!(timestamp_from_host <= time_after);
    }

    /// Tests the `send_proposal()` and `approve_proposal()` host functions.
    #[test]
    fn test_proposals_send_approve() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Test sending a proposal.
        let actions = vec![1, 2, 3, 4, 5, 6];
        let actions_ptr = 100u64;
        // Write actions to guest memory.
        host.borrow_memory().write(actions_ptr, &actions).unwrap();
        let actions_buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, actions_buf_ptr, actions_ptr, actions.len() as u64);

        let id_out_ptr = 300u64;
        let id_buf_ptr = 32u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, id_buf_ptr, id_out_ptr, 32);
        // Guest: send proposal to host with actions `actions_buf_ptr` and get back the proposal ID
        // in `id_buf_ptr`.
        host.send_proposal(actions_buf_ptr, id_buf_ptr).unwrap();

        // Verify the proposal with the given actions were successfully added.
        assert_eq!(host.borrow_logic().proposals.len(), 1);
        assert_eq!(
            host.borrow_logic().proposals.values().next().unwrap(),
            &actions
        );
        // Verify there are no approvals yet.
        assert_eq!(host.borrow_logic().approvals.len(), 0);

        // Test approving a proposal.
        // Approval ID is the Answer to the Ultimate Question of Life, the Universe, and Everything.
        let approval_id = [42u8; 32];
        let approval_ptr = 500u64;
        // Write approval to guest memory.
        host.borrow_memory()
            .write(approval_ptr, &approval_id)
            .unwrap();

        let approval_buf_ptr = 48u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            approval_buf_ptr,
            approval_ptr,
            approval_id.len() as u64,
        );

        // Guest: send a proposal approval to host.
        host.approve_proposal(approval_buf_ptr).unwrap();

        // Verify the host successfully stored the approval and its ID matches the one we sent.
        assert_eq!(host.borrow_logic().approvals.len(), 1);
        assert_eq!(host.borrow_logic().approvals[0], approval_id);
    }

    /// Verifies that `blob_create` host function correctly returns an error when
    /// the node client is not configured.
    #[test]
    fn test_blob_create_without_client_returns_an_error() {
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

    /// A smoke test for the successful path of the `finish` method.
    ///
    /// This test simulates a VM execution that successfully finished by
    /// calling `finish(None)` and asserts that the `returns` field in
    /// the final `Outcome` is an `Ok`, ensuring the error is propagated correctly.
    #[test]
    fn test_smoke_finish() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (logic, _) = setup_vm!(&mut storage, &limits, vec![1, 2, 3]);
        let outcome = logic.finish(None);

        assert!(outcome.returns.is_ok());
    }

    /// A smoke test for the error-handling path of the `finish` method.
    ///
    /// This test simulates a VM execution that failed by calling `finish(Some(Error))`
    /// and asserts that the `returns` field in the final `Outcome` is an `Err`,
    /// ensuring the error is propagated correctly.
    #[test]
    fn test_smoke_finish_with_error() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (logic, _) = setup_vm!(&mut storage, &limits, vec![]);
        let outcome = logic.finish(Some(FunctionCallError::ExecutionError(vec![])));
        assert!(outcome.returns.is_err());
    }

    // ===========================================================================
    // Tests for private functions
    // ===========================================================================

    /// Verifies the success path of the private `read_slice` and `read_str` functions.
    #[test]
    fn test_private_read_slice_and_str_success() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        let expected_str = "hello world";
        let data_ptr = 100u64;
        // Write msg to guest memory.
        write_str(&host, data_ptr, expected_str);
        let buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, expected_str.len() as u64);

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr).unwrap() };

        // Guest: ask host to read str from the `buffer` located in guest memory.
        let result_str = host.read_str(&buffer).unwrap();
        assert_eq!(result_str, expected_str);

        // Guest: ask host to read slice from the `buffer` located in guest memory.
        let result_slice = host.read_slice(&buffer);
        assert_eq!(result_slice, expected_str.as_bytes());
    }

    /// Verifies the error handling of the private `read_str` function for invalid UTF-8.
    #[test]
    fn test_private_read_str_invalid_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        let invalid_utf8: &[u8] = &[0, 159, 146, 150];
        let data_ptr = 100u64;
        // Write invalid utf8 buffer to the guest memory
        host.borrow_memory().write(data_ptr, invalid_utf8).unwrap();
        let buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, invalid_utf8.len() as u64);

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr).unwrap() };

        // Test that `read_str` fails as expected.
        let err = host.read_str(&buffer).unwrap_err();
        assert!(matches!(err, VMLogicError::HostError(HostError::BadUTF8)));
    }

    /// Verifies the success and failure paths of the private `read_guest_memory_sized` function.
    #[test]
    fn test_private_read_guest_memory_sized() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        // Test success case.
        let correct_data = [42u8; 32];
        let data_ptr_ok = 100u64;
        // Write correct data to guest memory.
        host.borrow_memory()
            .write(data_ptr_ok, &correct_data)
            .unwrap();
        let buf_ptr_ok = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, buf_ptr_ok, data_ptr_ok, correct_data.len() as u64);

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer_ok = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr_ok).unwrap() };
        let result_sized_ok = host.read_sized::<32>(&buffer_ok).unwrap();
        assert_eq!(result_sized_ok, &correct_data);

        // Test failure case (incorrect length).
        let incorrect_data = [1u8; 31];
        let data_ptr_err = 300u64;
        // Write incorrect data to guest memory.
        host.borrow_memory()
            .write(data_ptr_err, &incorrect_data)
            .unwrap();
        let buf_ptr_err = 32u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            buf_ptr_err,
            data_ptr_err,
            incorrect_data.len() as u64,
        );

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer_err = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr_err).unwrap() };
        // Guest: ask host to read the guest memory sized.
        let err = host.read_sized::<32>(&buffer_err).unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::InvalidMemoryAccess)
        ));
    }
}
