#![allow(single_use_lifetimes, unused_lifetimes, reason = "False positive")]
#![allow(clippy::mem_forget, reason = "Safe for now")]

use core::fmt;
use core::num::NonZeroU64;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};
use std::vec;

use borsh::from_slice as from_borsh_slice;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
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
    // Blob limits
    pub max_blob_handles: u64,
    pub max_blob_chunk_size: u64,
    // pub max_execution_time: u64,
    // number of functions per contract
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
            max_blob_handles: 100,                                   // Max blob handles
            max_blob_chunk_size: 10 << 20,                           // 10 MiB max chunk size
        }
    }
}

// Blob descriptor management structures
#[derive(Debug)]
enum BlobHandle {
    Write(BlobWriteHandle),
    Read(BlobReadHandle),
}

#[derive(Debug)]
struct BlobWriteHandle {
    sender: mpsc::UnboundedSender<Vec<u8>>,
    completion_handle: tokio::task::JoinHandle<eyre::Result<(BlobId, u64)>>,
}

#[derive(Debug)]
struct BlobReadHandle {
    blob_id: BlobId,
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
    fn read_guest_memory(&self, ptr: u64, len: u64) -> VMLogicResult<Vec<u8>> {
        let mut buf = vec![0; usize::try_from(len).map_err(|_| HostError::IntegerOverflow)?];

        self.borrow_memory().read(ptr, &mut buf)?;

        Ok(buf)
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

    pub fn commit(
        &mut self,
        root_hash_ptr: u64,
        root_hash_len: u64,
        artifact_ptr: u64,
        artifact_len: u64,
    ) -> VMLogicResult<()> {
        let root_hash = self.read_guest_memory_sized::<32>(root_hash_ptr, root_hash_len)?;
        let artifact = self.read_guest_memory(artifact_ptr, artifact_len)?;

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
    pub fn send_proposal(
        &mut self,
        actions_ptr: u64,
        actions_len: u64,
        id_ptr: u64,
        id_len: u64,
    ) -> VMLogicResult<()> {
        if id_len != 32 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let actions_bytes: Vec<u8> = self.read_guest_memory(actions_ptr, actions_len)?;
        let mut proposal_id = [0; 32];

        rand::thread_rng().fill_bytes(&mut proposal_id);
        drop(self.with_logic_mut(|logic| logic.proposals.insert(proposal_id, actions_bytes)));

        self.borrow_memory().write(id_ptr, &proposal_id)?;

        Ok(())
    }

    pub fn approve_proposal(&mut self, approval_ptr: u64, approval_len: u64) -> VMLogicResult<()> {
        if approval_len != 32 {
            return Err(HostError::InvalidMemoryAccess.into());
        }
        let approval = self.read_guest_memory_sized::<32>(approval_ptr, approval_len)?;
        let _ = self.with_logic_mut(|logic| logic.approvals.push(approval));

        Ok(())
    }

    // ========== CHUNKED BLOB HOST FUNCTIONS ==========

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

        let fd = self.with_logic_mut(|logic| {
            let fd = logic.next_blob_fd;
            logic.next_blob_fd += 1;

            let (data_sender, data_receiver) = mpsc::unbounded_channel();

            let node_client = logic.node_client.clone().unwrap();

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
            fd
        });

        Ok(fd)
    }

    /// Write a chunk of data to a blob
    /// Returns: number of bytes written (u64)
    pub fn blob_write(&mut self, fd: u64, data_ptr: u64, data_len: u64) -> VMLogicResult<u64> {
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

        let data = self.read_guest_memory(data_ptr, data_len)?;

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

        let data_len = data.len() as u64;
        self.with_logic_mut(|logic| {
            let handle = logic.blob_handles.get_mut(&fd).unwrap(); // We already validated it exists
            match handle {
                BlobHandle::Write(w) => {
                    w.sender
                        .send(data.clone())
                        .map_err(|_| VMLogicError::HostError(HostError::InvalidBlobHandle))?;
                }
                _ => unreachable!(),
            }
            Ok::<(), VMLogicError>(())
        })?;

        Ok(data_len)
    }

    /// Close a blob handle and get the resulting blob ID
    /// Returns: 1 on success
    pub fn blob_close(
        &mut self,
        fd: u64,
        blob_id_ptr: u64,
        blob_id_len: u64,
    ) -> VMLogicResult<u32> {
        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        if blob_id_len != 32 {
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

    /// Open a blob for reading
    /// Returns: file descriptor (u64) for reading operations  
    pub fn blob_open(&mut self, blob_id_ptr: u64, blob_id_len: u64) -> VMLogicResult<u64> {
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

        let blob_id_bytes = self.read_guest_memory_sized::<32>(blob_id_ptr, blob_id_len)?;
        let blob_id = BlobId::from(blob_id_bytes);

        let fd = self.with_logic_mut(|logic| {
            let fd = logic.next_blob_fd;
            logic.next_blob_fd += 1;

            let handle = BlobHandle::Read(BlobReadHandle { blob_id });
            drop(logic.blob_handles.insert(fd, handle));
            fd
        });

        Ok(fd)
    }

    /// Read a chunk of data from a blob
    /// Returns: number of bytes read (u64)
    pub fn blob_read(&mut self, fd: u64, data_ptr: u64, data_len: u64) -> VMLogicResult<u64> {
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

        // Get blob_id and validate handle once upfront
        let blob_id = self.with_logic_mut(|logic| {
            let handle = logic
                .blob_handles
                .get(&fd)
                .ok_or(VMLogicError::HostError(HostError::InvalidBlobHandle))?;

            match handle {
                BlobHandle::Read(r) => Ok(r.blob_id),
                BlobHandle::Write(_) => Err(VMLogicError::HostError(HostError::InvalidBlobHandle)),
            }
        })?;

        // Stream blob data directly - just get next chunk
        let blob_data = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // Get the blob stream using the proper blobstore API
                let blob_stream = node_client
                    .get_blob(&blob_id)
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?;

                let Some(mut blob_stream) = blob_stream else {
                    return Ok::<Vec<u8>, VMLogicError>(Vec::new()); // Blob not found, return empty
                };

                // Just get the next chunk from the stream
                if let Some(chunk_result) = blob_stream.next().await {
                    let chunk = chunk_result
                        .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?;
                    Ok(chunk.to_vec())
                } else {
                    Ok(Vec::new()) // End of stream
                }
            })
        })?;

        // Determine how much to copy (limited by requested length)
        let to_read = std::cmp::min(data_len as usize, blob_data.len());

        // Copy data to guest memory
        if to_read > 0 {
            self.borrow_memory()
                .write(data_ptr, &blob_data[..to_read])?;
        }

        Ok(to_read as u64)
    }
}
