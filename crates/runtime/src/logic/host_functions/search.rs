//! `search_query`: the node's full-text search, from a view (PoC).

use tracing::trace;

use crate::errors::HostError;
use crate::logic::{sys, VMHostFunctions, VMLogicResult};

/// `search_query` calls one execution may make. The host-side work of a call
/// is not metered by gas, so the count is what bounds it (each call is itself
/// bounded: a page is at most 100 hits and a query at most 256 bytes).
pub const MAX_SEARCH_CALLS: u64 = 32;

/// Largest request a guest may pass, in bytes.
const MAX_SEARCH_REQUEST: u64 = 4096;

impl VMHostFunctions<'_> {
    /// Run a search against the *current* context's index.
    ///
    /// Reads a borsh `SearchRequest` from the buffer at `request_ptr`. On
    /// success writes a borsh `SearchResponse` to `register_id` and returns
    /// `1`; on a request the index refuses (unknown field, too-short
    /// substring) writes the UTF-8 reason there and returns `0`.
    ///
    /// There is no context argument. The host passes its own
    /// `VMContext::context_id`, so a view can only ever search the context it
    /// runs in.
    ///
    /// # Errors
    ///
    /// * `HostError::SearchUnavailable` outside a view, or on a node without
    ///   search — the node supplies the search handle to read-only runs only.
    /// * `HostError::SearchCallsExceeded` past [`MAX_SEARCH_CALLS`].
    /// * `HostError::InvalidMemoryAccess` for a bad descriptor.
    pub fn search_query(&mut self, request_ptr: u64, register_id: u64) -> VMLogicResult<u32> {
        let Some(search) = self.borrow_logic().context.search.clone() else {
            return Err(HostError::SearchUnavailable.into());
        };
        if self.borrow_logic().search_calls >= MAX_SEARCH_CALLS {
            return Err(HostError::SearchCallsExceeded {
                max: MAX_SEARCH_CALLS,
            }
            .into());
        }
        self.with_logic_mut(|logic| logic.search_calls += 1);

        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let request = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(request_ptr)? };
        if request.len() > MAX_SEARCH_REQUEST {
            return Err(HostError::ValueLengthOverflow.into());
        }
        let request = self.read_guest_memory_slice(&request)?.to_vec();
        let context = self.borrow_logic().context.context_id;

        let (status, out) = match search.search(context, &request) {
            Ok(response) => (1, response),
            Err(reason) => (0, reason.into_bytes()),
        };
        trace!(
            target: "runtime::host::search",
            status,
            response_len = out.len(),
            "search_query"
        );
        self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, out))?;
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::{Arc, Mutex};

    use wasmer::{AsStoreMut, Store};

    use super::*;
    use crate::logic::tests::{prepare_guest_buf_descriptor, SimpleMockStorage};
    use crate::logic::{SearchHost, VMContext, VMLimits, VMLogic, VMLogicError};

    /// Records which context each call was bound to, and echoes the request.
    #[derive(Debug, Default)]
    struct Recorder(Mutex<Vec<[u8; 32]>>);

    impl SearchHost for Recorder {
        fn search(&self, context: [u8; 32], request: &[u8]) -> Result<Vec<u8>, String> {
            self.0.lock().unwrap().push(context);
            if request == b"bad" {
                return Err("refused".to_owned());
            }
            Ok(request.to_vec())
        }
    }

    fn run(
        search: Option<Arc<dyn SearchHost>>,
        calls: usize,
        request: &[u8],
    ) -> Vec<VMLogicResult<(u32, Vec<u8>)>> {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let mut context = VMContext::new(
            Cow::Owned(vec![]),
            [0xC1; 32],
            [0; 32],
            calimero_account::AccountId::from([0; 32]),
        );
        context.search = search;
        let mut store = Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
        let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
        let _ = logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());
        prepare_guest_buf_descriptor(&host, 16, 200, request.len() as u64);
        host.borrow_memory().write(200, request).unwrap();
        (0..calls)
            .map(|_| {
                let status = host.search_query(16, 1)?;
                let out = host.borrow_logic().registers.get(1)?.to_vec();
                Ok((status, out))
            })
            .collect()
    }

    #[test]
    fn a_run_without_the_search_handle_cannot_search() {
        let results = run(None, 1, b"q");
        assert!(matches!(
            results[0],
            Err(VMLogicError::HostError(HostError::SearchUnavailable))
        ));
    }

    #[test]
    fn the_host_binds_every_call_to_the_running_context() {
        let recorder = Arc::new(Recorder::default());
        let results = run(Some(recorder.clone()), 2, b"any request");
        for result in results {
            assert_eq!(result.unwrap(), (1, b"any request".to_vec()));
        }
        assert_eq!(*recorder.0.lock().unwrap(), vec![[0xC1; 32]; 2]);
        let results = run(Some(recorder), 1, b"bad");
        assert_eq!(results[0].as_ref().unwrap(), &(0, b"refused".to_vec()));
    }

    #[test]
    fn calls_are_capped_per_execution() {
        let results = run(
            Some(Arc::new(Recorder::default())),
            MAX_SEARCH_CALLS as usize + 1,
            b"q",
        );
        assert!(results[..MAX_SEARCH_CALLS as usize]
            .iter()
            .all(Result::is_ok));
        assert!(matches!(
            results.last(),
            Some(Err(VMLogicError::HostError(
                HostError::SearchCallsExceeded { .. }
            )))
        ));
    }
}
