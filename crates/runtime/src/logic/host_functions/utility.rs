use borsh::from_slice as from_borsh_slice;
use rand::RngCore;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicResult},
};
use tracing::{debug, trace};

use ed25519_dalek::{Signature, Verifier, VerifyingKey, PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH};

impl VMHostFunctions<'_> {
    /// Fetches data from a URL.
    ///
    /// Performs an HTTP request. This is a synchronous, blocking operation.
    /// The response body is placed into the specified host register.
    ///
    /// # Arguments
    ///
    /// * `src_url_ptr` - Pointer to the URL string source-buffer in guest memory.
    /// * `src_method_ptr` - Pointer to the HTTP method string source-buffer (e.g., "GET", "POST")
    /// in guest memory.
    /// * `src_headers_ptr` - Pointer to a borsh-serialized `Vec<(String, String)>` source-buffer of
    /// headers in guest memory.
    /// * `src_body_ptr` - Pointer to the request body source-buffer in guest memory.
    /// * `dest_register_id` - The ID of the destination register in host memory where to store
    /// the response body.
    ///
    /// # Returns
    ///
    /// * Returns `0` on success (HTTP 2xx)
    /// * Returns `1` on failure.
    /// The response body or error message is placed in the host register.
    ///
    /// # Errors
    ///
    /// * `HostError::DeserializationError` if the headers cannot be deserialized.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    ///
    /// BLOCKED: This function is temporarily disabled.
    /// It will always return 1 (failure) and write an error message to the destination register.
    pub fn fetch(
        &mut self,
        src_url_ptr: u64,
        src_method_ptr: u64,
        src_headers_ptr: u64,
        src_body_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        const BLOCKED: bool = true;

        if BLOCKED {
            // Log a warning on the host side if an attempt to use this host fn is made
            tracing::warn!(target: "runtime::host::utility", "Blocked attempt to access 'fetch' host function (temporarily disabled)");

            // Message to return to the Wasm guest
            let error_message = "Host function 'fetch' is temporarily disabled.";

            // Write the error message to the destination register so the guest can read the reason
            self.with_logic_mut(|logic| {
                logic
                    .registers
                    .set(logic.limits, dest_register_id, error_message.as_bytes())
            })?;

            // Return 1 to indicate a "Network/HTTP Failure" to the guest, instead of panicking in the VM
            return Ok(1);
        }

        let url = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_url_ptr)? };
        let method = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_method_ptr)? };
        let headers = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_headers_ptr)? };
        let body = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_body_ptr)? };

        let url = self.read_guest_memory_str(&url)?;
        let method = self.read_guest_memory_str(&method)?;

        let headers = self.read_guest_memory_slice(&headers);
        let body = self.read_guest_memory_slice(&body);

        // TODO: clarify why the `fetch` function cannot be directly called by applications.
        // Note: The `fetch` function cannot be directly called by applications.
        // Therefore, the headers are generated exclusively by our code, ensuring
        // that it is safe to deserialize them.
        let headers: Vec<(String, String)> =
            from_borsh_slice(headers).map_err(|_| HostError::DeserializationError)?;

        let mut request = ureq::request(method, url);

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

        self.with_logic_mut(|logic| logic.registers.set(logic.limits, dest_register_id, data))?;
        Ok(status)
    }

    /// Fills a guest memory buffer with random bytes.
    ///
    /// # Arguments
    ///
    /// * `dest_ptr` - A destination pointer to a `sys::BufferMut` in guest memory to be filled.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn random_bytes(&mut self, dest_ptr: u64) -> VMLogicResult<()> {
        let dest_buf = unsafe { self.read_guest_memory_typed::<sys::BufferMut<'_>>(dest_ptr)? };

        rand::thread_rng().fill_bytes(self.read_guest_memory_slice_mut(&dest_buf));

        Ok(())
    }

    /// Gets the current Unix timestamp in nanoseconds.
    ///
    /// This function obtains the current time as a nanosecond timestamp, as
    /// [`SystemTime`] is not available inside the guest runtime. Therefore the
    /// guest needs to request this from the host.
    ///
    /// The result is written into a guest buffer as a `u64`.
    ///
    /// # Arguments
    ///
    /// * `dest_ptr` - A pointer to an 8-byte destination buffer `sys::BufferMut`
    /// in guest memory where the `u64` timestamp will be written.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the provided buffer is not exactly 8 bytes long
    /// or if memory access fails for a descriptor buffer.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Impossible to overflow in normal circumstances"
    )]
    #[expect(
        clippy::expect_used,
        clippy::unwrap_in_result,
        reason = "Effectively infallible here"
    )]
    pub fn time_now(&mut self, dest_ptr: u64) -> VMLogicResult<()> {
        let guest_time_ptr =
            unsafe { self.read_guest_memory_typed::<sys::BufferMut<'_>>(dest_ptr)? };

        if guest_time_ptr.len() != 8 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64;

        // Record the time into the guest memory buffer
        let guest_time_out_buf: &mut [u8] = self.read_guest_memory_slice_mut(&guest_time_ptr);
        guest_time_out_buf.copy_from_slice(&now.to_le_bytes());

        Ok(())
    }

    /// Verifies an ED25519 signature.
    ///
    /// # Arguments
    ///
    /// * `src_signature_ptr` - Pointer to a 64-byte source-buffer `sys::Buffer` in guest memory.
    /// * `src_public_key_ptr` - Pointer to a 32-byte source-buffer `sys::Buffer` in guest memory.
    /// * `src_message_ptr` - Pointer to a source-buffer `sys::Buffer` in guest memory containing the message.
    ///
    /// # Returns
    ///
    /// * Returns `1` if the signature is valid.
    /// * Returns `0` if the signature is invalid or an error occurred (e.g., wrong key/sig length).
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    /// * `HostError::Ed25519IncorrectPublicKey` if the provided public key is not a ED25519 Public
    /// Key.
    pub fn ed25519_verify(
        &mut self,
        src_signature_ptr: u64,
        src_public_key_ptr: u64,
        src_message_ptr: u64,
    ) -> VMLogicResult<u32> {
        // Read buffer descriptors from guest memory
        let signature_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_signature_ptr)? };
        let public_key_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_public_key_ptr)? };
        let message_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_message_ptr)? };

        // Read byte slices from guest memory and enforce correct length for signature and public
        // key.
        let signature_bytes = self.read_guest_memory_sized::<SIGNATURE_LENGTH>(&signature_buf)?;
        let public_key_bytes =
            self.read_guest_memory_sized::<PUBLIC_KEY_LENGTH>(&public_key_buf)?;
        let message_bytes = self.read_guest_memory_slice(&message_buf);

        // Parse signature
        let signature = Signature::from_bytes(signature_bytes);
        debug!(
            ?signature_bytes,
            ?public_key_bytes,
            ?message_bytes,
            ?signature,
            "Trying to verify Ed25519 signature"
        );

        // Parse public key
        let public_key = VerifyingKey::from_bytes(public_key_bytes)
            .map_err(|_| HostError::Ed25519IncorrectPublicKey)?;

        let verification_result = public_key.verify(message_bytes, &signature);
        debug!(
            ?signature_bytes,
            ?public_key_bytes,
            ?message_bytes,
            ?signature,
            ?verification_result,
            "Ed25519 signature verification complete"
        );

        match verification_result {
            Ok(_) => Ok(1),  // Signature is valid
            Err(_) => Ok(0), // Signature is invalid
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::logic::{
        tests::{prepare_guest_buf_descriptor, setup_vm, SimpleMockStorage},
        Cow, VMContext, VMLimits, VMLogic, VMLogicError, DIGEST_SIZE,
    };
    use wasmer::{AsStoreMut, Store};

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

    #[test]
    fn test_ed25519_verify() {
        use ed25519_dalek::Signer;
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        // 1. Setup host
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // 2. Generate a valid signature (on the host side, for testing)
        let mut csprng = OsRng;
        let keypair: SigningKey = SigningKey::generate(&mut csprng);
        let public_key = keypair.verifying_key();
        let message = b"test message";
        let signature = keypair.try_sign(message).expect("Signing failed");

        // 3. Write data to guest memory
        let sig_ptr = 100u64;
        let pk_ptr = 200u64;
        let msg_ptr = 300u64;
        host.borrow_memory()
            .write(sig_ptr, &signature.to_bytes())
            .unwrap();
        host.borrow_memory()
            .write(pk_ptr, public_key.as_ref())
            .unwrap();
        host.borrow_memory().write(msg_ptr, message).unwrap();

        // 4. Prepare descriptors
        let sig_buf_ptr = 16u64;
        let pk_buf_ptr = 32u64;
        let msg_buf_ptr = 48u64;
        prepare_guest_buf_descriptor(&host, sig_buf_ptr, sig_ptr, SIGNATURE_LENGTH as u64);
        prepare_guest_buf_descriptor(&host, pk_buf_ptr, pk_ptr, PUBLIC_KEY_LENGTH as u64);
        prepare_guest_buf_descriptor(&host, msg_buf_ptr, msg_ptr, message.len() as u64);

        // --- Test 1: valid signature
        let res_valid = host
            .ed25519_verify(sig_buf_ptr, pk_buf_ptr, msg_buf_ptr)
            .unwrap();
        assert_eq!(res_valid, 1, "Signature should be valid");

        // --- Test 2: invalid signature (wrong message)
        let invalid_message = b"wrong message";
        let msg_invalid_ptr = 400u64;
        let msg_invalid_buf_ptr = 64u64;
        host.borrow_memory()
            .write(msg_invalid_ptr, invalid_message)
            .unwrap();
        prepare_guest_buf_descriptor(
            &host,
            msg_invalid_buf_ptr,
            msg_invalid_ptr,
            invalid_message.len() as u64,
        );

        let res_invalid = host
            .ed25519_verify(sig_buf_ptr, pk_buf_ptr, msg_invalid_buf_ptr)
            .unwrap();
        assert_eq!(
            res_invalid, 0,
            "Signature should be invalid for wrong message"
        );

        // --- Test 3: Invalid Signature (Wrong Signature) ---
        let invalid_sig = [0u8; SIGNATURE_LENGTH];
        let sig_invalid_ptr = 500u64;
        let sig_invalid_buf_ptr = 80u64;
        host.borrow_memory()
            .write(sig_invalid_ptr, &invalid_sig)
            .unwrap();
        prepare_guest_buf_descriptor(
            &host,
            sig_invalid_buf_ptr,
            sig_invalid_ptr,
            SIGNATURE_LENGTH as u64,
        );

        let res_invalid_sig = host
            .ed25519_verify(sig_invalid_buf_ptr, pk_buf_ptr, msg_buf_ptr)
            .unwrap();
        assert_eq!(
            res_invalid_sig, 0,
            "Signature should be invalid for wrong signature"
        );

        // --- Test 4: Invalid Public Key Length ---
        let pk_invalid_len_ptr = 600u64;
        let pk_invalid_len_buf_ptr = 96u64;
        host.borrow_memory()
            .write(pk_invalid_len_ptr, &[1, 2, 3])
            .unwrap();
        prepare_guest_buf_descriptor(&host, pk_invalid_len_buf_ptr, pk_invalid_len_ptr, 3); // Incorrect length

        let res_invalid_len = host.ed25519_verify(sig_buf_ptr, pk_invalid_len_buf_ptr, msg_buf_ptr);
        assert!(
            matches!(
                res_invalid_len,
                Err(VMLogicError::HostError(HostError::InvalidMemoryAccess))
            ),
            "Should fail with InvalidMemoryAccess due to read_guest_memory_sized"
        );

        // --- Test 5: Invalid Signature Length ---
        let sig_invalid_len_ptr = 800u64;
        let sig_invalid_len_buf_ptr = 128u64;
        host.borrow_memory()
            .write(sig_invalid_len_ptr, &[1, 2, 3])
            .unwrap();
        prepare_guest_buf_descriptor(&host, sig_invalid_len_buf_ptr, sig_invalid_len_ptr, 3); // Incorrect length

        let res_invalid_sig_len =
            host.ed25519_verify(sig_invalid_len_buf_ptr, pk_buf_ptr, msg_buf_ptr);
        assert!(
            matches!(
                res_invalid_sig_len,
                Err(VMLogicError::HostError(HostError::InvalidMemoryAccess))
            ),
            "Should fail with InvalidMemoryAccess due to read_guest_memory_sized"
        )
    }
}
