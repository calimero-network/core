use std::io::{Cursor, Error, Read};

use calimero_primitives::{blobs::BlobId, context::ContextId};

use futures_util::{StreamExt, TryStreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicError, VMLogicResult, DIGEST_SIZE},
};

/// An enum representing a handle to a blob, which can be for reading or writing.
#[derive(Debug)]
pub enum BlobHandle {
    /// A handle for writing data to a blob.
    Write(BlobWriteHandle),
    /// A handle for reading data from an existing blob.
    Read(BlobReadHandle),
}

/// A handle for managing an asynchronous blob write operation.
#[derive(Debug)]
pub struct BlobWriteHandle {
    /// The sender part of a channel to stream data chunks to the writer task.
    sender: mpsc::UnboundedSender<Vec<u8>>,
    /// A handle to the spawned task that performs the blob writing,
    /// which will eventually yield the `BlobId` and total size of the data written.
    completion_handle: tokio::task::JoinHandle<eyre::Result<(BlobId, u64)>>,
}

/// A handle for managing a blob read operation.
pub struct BlobReadHandle {
    /// The ID of the blob being read.
    blob_id: BlobId,
    /// The asynchronous stream of data chunks from the blob storage.
    stream: Option<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, Error>> + Unpin>>,
    /// A cursor for the current data chunk to handle partial reads efficiently.
    /// Cursor for current storage chunk - automatic position tracking!
    // TODO: clarify the "automatic position tracking".
    current_chunk_cursor: Option<Cursor<Vec<u8>>>,
    /// The current reading position within the blob.
    position: u64,
}

impl std::fmt::Debug for BlobReadHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobReadHandle")
            .field("blob_id", &self.blob_id)
            .field("stream", &"<stream>")
            .field("current_chunk_cursor", &self.current_chunk_cursor)
            .field("position", &self.position)
            .finish()
    }
}

impl VMHostFunctions<'_> {
    /// Creates a new blob for writing.
    ///
    /// This initializes a blob upload stream and returns a file descriptor (`fd`) that
    /// can be used with `blob_write` and `blob_close`.
    ///
    /// # Returns
    ///
    /// A `u64` file descriptor for the new blob write handle.
    ///
    /// # Errors
    ///
    /// * `HostError::BlobsNotSupported` if the node client is not configured.
    /// * `HostError::TooManyBlobHandles` if the maximum number of handles is exceeded.
    /// * `HostError::IntegerOverflow` on `u64` overflow.
    pub fn blob_create(&mut self) -> VMLogicResult<u64> {
        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        // The error should never happen as unlikely we have limits set with a value >= u32::MAX.
        // Still, the check is essential as downcasting on 32-bit systems might lead to
        // undefined behavior.
        let Ok(limits_max_blob_handles) =
            usize::try_from(self.borrow_logic().limits.max_blob_handles)
        else {
            return Err(VMLogicError::HostError(HostError::IntegerOverflow));
        };

        if self.borrow_logic().blob_handles.len() >= limits_max_blob_handles {
            return Err(VMLogicError::HostError(HostError::TooManyBlobHandles {
                max: self.borrow_logic().limits.max_blob_handles,
            }));
        }

        let fd = self.with_logic_mut(|logic| -> VMLogicResult<u64> {
            let Some(node_client) = logic.node_client.clone() else {
                return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
            };

            let fd = logic.next_blob_fd;
            logic.next_blob_fd = logic
                .next_blob_fd
                .checked_add(1)
                .ok_or(VMLogicError::HostError(HostError::IntegerOverflow))?;

            let (data_sender, data_receiver) = mpsc::unbounded_channel();

            let completion_handle = tokio::spawn(async move {
                let stream = UnboundedReceiverStream::new(data_receiver);

                let byte_stream =
                    stream.map(|data: Vec<u8>| Ok::<bytes::Bytes, Error>(data.into()));
                let reader = byte_stream.into_async_read();

                node_client.add_blob(reader, None, None).await
            });

            //TODO: add assert that no bytes were written during the creation of an empty blob.

            let handle = BlobHandle::Write(BlobWriteHandle {
                sender: data_sender,
                completion_handle,
            });

            drop(logic.blob_handles.insert(fd, handle));
            Ok(fd)
        })?;

        Ok(fd)
    }

    /// Writes a chunk of data to a blob.
    ///
    /// # Arguments
    ///
    /// * `fd` - The file descriptor obtained from `blob_create()` operation.
    /// * `src_data_ptr` - A pointer to a source-buffer `sys::Buffer` in guest memory
    /// containing the data chunk to write.
    ///
    /// # Returns
    ///
    /// The number of bytes written as `u64`, which is equal to the length of the input data buffer.
    ///
    /// # Errors
    ///
    /// * `HostError::BlobsNotSupported` if the node client is not configured.
    /// * `HostError::InvalidBlobHandle` if the `fd` is invalid or not a write handle.
    /// * `HostError::BlobWriteTooLarge` if the data chunk exceeds `max_blob_chunk_size`.
    pub fn blob_write(&mut self, fd: u64, src_data_ptr: u64) -> VMLogicResult<u64> {
        let data = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_data_ptr)? };
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

        let data = self.read_guest_memory_slice(&data).to_vec();

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
                BlobHandle::Read(_) => {
                    return Err(VMLogicError::HostError(HostError::InvalidBlobHandle))
                }
            }
            Ok::<(), VMLogicError>(())
        })?;

        Ok(data_len)
    }

    /// Closes a blob handle and gets the resulting blob ID.
    ///
    /// For a write handle, this finalizes the upload and writes the resulting `BlobId`
    /// into the guest's memory buffer. For a read handle, it simply closes it.
    ///
    /// # Arguments
    ///
    /// * `fd` - The file descriptor to close.
    /// * `dest_blob_id_ptr` - A pointer to a 32-byte destination buffer `sys::BufferMut`
    /// in guest memory where the final `BlobId` will be written (for write handles).
    ///
    /// # Returns
    ///
    /// Returns `1` on success.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the `blob_id_ptr` buffer is not 32 bytes
    /// or if memory access fails for a descriptor buffer.
    /// * `HostError::InvalidBlobHandle` if the `fd` is invalid.
    /// * `HostError::BlobsNotSupported` if the node client is not supported or upload operation fails.
    pub fn blob_close(&mut self, fd: u64, dest_blob_id_ptr: u64) -> VMLogicResult<u32> {
        let guest_blob_id_ptr =
            unsafe { self.read_guest_memory_typed::<sys::BufferMut<'_>>(dest_blob_id_ptr)? };

        if guest_blob_id_ptr.len() != DIGEST_SIZE as u64 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        let handle = self.with_logic_mut(|logic| {
            logic
                .blob_handles
                .remove(&fd)
                .ok_or(VMLogicError::HostError(HostError::InvalidBlobHandle))
        })?;

        let guest_blob_id_out_buf: &mut [u8] = self.read_guest_memory_slice_mut(&guest_blob_id_ptr);

        match handle {
            BlobHandle::Write(write_handle) => {
                let _ignored = write_handle.sender;

                let (blob_id_, _size) = tokio::runtime::Handle::current()
                    .block_on(write_handle.completion_handle)
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?
                    .map_err(|_| VMLogicError::HostError(HostError::BlobsNotSupported))?;

                // Record the Blob ID into the guest memory buffer
                guest_blob_id_out_buf.copy_from_slice(blob_id_.as_ref());
            }
            // Record the Blob ID into the guest memory buffer
            BlobHandle::Read(read_handle) => {
                guest_blob_id_out_buf.copy_from_slice(read_handle.blob_id.as_ref())
            }
        }

        Ok(1)
    }

    /// Announces a blob to a specific context for network discovery.
    ///
    /// # Arguments
    ///
    /// * `src_blob_id_ptr` - pointer to a 32-byte source-buffer `sys::Buffer` in guest memory,
    /// containing the 32-byte `BlobId`.
    /// * `src_context_id_ptr` - pointer to a 32-byte source-buffer `sys::Buffer` in guest memory,
    /// containing the 32-byte `ContextId`.
    ///
    /// # Returns
    ///
    /// Returns `1` on successful announcement.
    ///
    /// # Errors
    ///
    /// * `HostError::BlobsNotSupported` if blob functionality is disabled or a network
    ///   error occurs.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn blob_announce_to_context(
        &mut self,
        src_blob_id_ptr: u64,
        src_context_id_ptr: u64,
    ) -> VMLogicResult<u32> {
        // Check if blob functionality is available
        let node_client = match &self.borrow_logic().node_client {
            Some(client) => client.clone(),
            None => return Err(VMLogicError::HostError(HostError::BlobsNotSupported)),
        };

        let blob_id = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_blob_id_ptr)? };
        let context_id =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_context_id_ptr)? };

        let blob_id = BlobId::from(*self.read_guest_memory_sized::<DIGEST_SIZE>(&blob_id)?);
        let context_id =
            ContextId::from(*self.read_guest_memory_sized::<DIGEST_SIZE>(&context_id)?);

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

    /// Opens an existing blob for reading.
    ///
    /// # Arguments
    ///
    /// * `src_blob_id_ptr` - pointer to a 32-byte source-buffer `sys::Buffer` in guest memory,
    /// containing the 32-byte `BlobId`.
    ///
    /// # Returns
    ///
    /// A `u64` file descriptor for the new blob read handle.
    ///
    /// # Errors
    ///
    /// * `HostError::BlobsNotSupported` if the node client is not configured.
    /// * `HostError::TooManyBlobHandles` if the maximum number of handles is exceeded.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn blob_open(&mut self, src_blob_id_ptr: u64) -> VMLogicResult<u64> {
        let blob_id = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_blob_id_ptr)? };

        if self.borrow_logic().node_client.is_none() {
            return Err(VMLogicError::HostError(HostError::BlobsNotSupported));
        }

        // The error should never happen as unlikely we have limits set with a value >= u32::MAX.
        // Still, the check is essential as downcasting on 32-bit systems might lead to
        // undefined behavior.
        let Ok(limits_max_blob_handles) =
            usize::try_from(self.borrow_logic().limits.max_blob_handles)
        else {
            return Err(VMLogicError::HostError(HostError::IntegerOverflow));
        };

        if self.borrow_logic().blob_handles.len() >= limits_max_blob_handles {
            return Err(VMLogicError::HostError(HostError::TooManyBlobHandles {
                max: self.borrow_logic().limits.max_blob_handles,
            }));
        }

        let blob_id = BlobId::from(*self.read_guest_memory_sized::<DIGEST_SIZE>(&blob_id)?);

        let fd = self.with_logic_mut(|logic| -> VMLogicResult<u64> {
            let fd = logic.next_blob_fd;
            logic.next_blob_fd = logic
                .next_blob_fd
                .checked_add(1)
                .ok_or(VMLogicError::HostError(HostError::IntegerOverflow))?;

            let handle = BlobHandle::Read(BlobReadHandle {
                blob_id,
                stream: None,
                current_chunk_cursor: None,
                position: 0,
            });

            // TODO: verify if we need to drop it here or just ignore the value:
            // `let _ignored = logic.blob_handles.insert(fd, handle));`
            drop(logic.blob_handles.insert(fd, handle));
            Ok(fd)
        })?;

        Ok(fd)
    }

    /// Reads a chunk of data from an open blob.
    ///
    /// Data is read from the blob and copied into the provided guest memory buffer.
    ///
    /// # Arguments
    ///
    /// * `fd` - The file descriptor obtained from `blob_open`.
    /// * `dest_data_ptr` - A pointer to a destination buffer `sys::BufferMut` in guest memory where
    /// the read data will be stored
    ///
    /// # Returns
    ///
    /// The number of bytes actually read as `u64`. This can be less than the buffer size if the
    /// end of the blob is reached.
    ///
    /// # Errors
    ///
    /// * `HostError::BlobsNotSupported` if blob functionality is unavailable.
    /// * `HostError::InvalidBlobHandle` if the `fd` is invalid or not a read handle.
    /// * `HostError::BlobBufferTooLarge` if the guest buffer exceeds `max_blob_chunk_size`.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn blob_read(&mut self, fd: u64, dest_data_ptr: u64) -> VMLogicResult<u64> {
        let dest_data =
            unsafe { self.read_guest_memory_typed::<sys::BufferMut<'_>>(dest_data_ptr)? };
        let data_len = dest_data.len();

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

        // The error should never happen as we already validated the buffer size before.
        // Still, the check is essential as downcasting on 32-bit systems might lead to
        // undefined behavior.
        let Ok(data_len) = usize::try_from(data_len) else {
            return Err(VMLogicError::HostError(HostError::IntegerOverflow));
        };
        // Local output buffer.
        let mut output_buffer = Vec::with_capacity(data_len);

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

            // First, try to read from current chunk cursor if available
            if let Some(cursor) = &mut read_handle.current_chunk_cursor {
                let mut temp_buffer = vec![0_u8; data_len];
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
                        if output_buffer.len() >= data_len {
                            read_handle.position = read_handle
                                .position
                                .checked_add(output_buffer.len() as u64)
                                .ok_or(VMLogicError::HostError(HostError::IntegerOverflow))?;

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

                if let Some(stream) = blob_stream {
                    let mapped_stream = stream.map(|result| {
                        result
                            .map(|chunk| bytes::Bytes::copy_from_slice(&chunk))
                            .map_err(|_| Error::other("blob read error"))
                    });
                    read_handle.stream = Some(Box::new(mapped_stream));
                } else {
                    read_handle.position = read_handle
                        .position
                        .checked_add(output_buffer.len() as u64)
                        .ok_or(VMLogicError::HostError(HostError::IntegerOverflow))?;

                    return Ok(output_buffer.len() as u64);
                }
            }

            if let Some(stream) = &mut read_handle.stream {
                tokio::runtime::Handle::current().block_on(async {
                    while output_buffer.len() < data_len {
                        match stream.next().await {
                            Some(Ok(chunk)) => {
                                let chunk_bytes = chunk.as_ref();

                                let remaining_needed = data_len
                                    .checked_sub(output_buffer.len())
                                    .ok_or(VMLogicError::HostError(HostError::IntegerOverflow))?;

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

            read_handle.position = read_handle
                .position
                .checked_add(output_buffer.len() as u64)
                .ok_or(VMLogicError::HostError(HostError::IntegerOverflow))?;
            Ok(output_buffer.len() as u64)
        })?;

        if bytes_read > 0 {
            // Copy data from the local output buffer to destination buffer located in guest
            // memory.
            self.read_guest_memory_slice_mut(&dest_data)
                .copy_from_slice(&output_buffer);
        }

        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::logic::{
        tests::{setup_vm, SimpleMockStorage},
        Cow, VMContext, VMLimits, VMLogic,
    };
    use wasmer::{AsStoreMut, Store};

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
}
