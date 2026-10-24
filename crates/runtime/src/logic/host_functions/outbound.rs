//! `http_request`: one outbound HTTPS request, from a run that writes.

use calimero_primitives::outbound::{
    MAX_HEADERS, MAX_HEADER_BYTES, MAX_REQUESTS_PER_RUN, MAX_REQUEST_BODY, MAX_URL,
};
use tracing::trace;

use crate::errors::HostError;
use crate::logic::{sys, VMHostFunctions, VMLogicResult};

/// Largest encoded request a guest may pass: the body, the URL and the
/// headers at their limits, and room for borsh's length prefixes.
const MAX_ENCODED_REQUEST: u64 =
    (MAX_REQUEST_BODY + MAX_URL + MAX_HEADERS * MAX_HEADER_BYTES + 256) as u64;

impl VMHostFunctions<'_> {
    /// Ask the node to make one outbound HTTPS request.
    ///
    /// Reads a borsh `OutboundRequest` from the buffer at `request_ptr`. On
    /// success writes a borsh `OutboundResponse` to `register_id` and returns
    /// `1`; when the node refuses or the request fails, writes the UTF-8 reason
    /// there and returns `0`. Whether a request may be made at all, and where
    /// to, is entirely the node's decision (see `VMContext::outbound`); this
    /// function only bounds how often and how much.
    ///
    /// Never charged gas: the work is node-local, and a write's gas must be the
    /// same on every node.
    ///
    /// # Errors
    ///
    /// * `HostError::OutboundUnavailable` when the node supplied no outbound
    ///   client for this run: outbound requests are off on this node, the app's
    ///   manifest declares no URL, or the run is a view, a migration, a check,
    ///   a peer's delta or on someone else's behalf.
    /// * `HostError::OutboundRequestsExceeded` past `MAX_REQUESTS_PER_RUN`.
    /// * `HostError::ValueLengthOverflow` for a request over the size limit.
    /// * `HostError::InvalidMemoryAccess` for a bad descriptor.
    pub fn http_request(&mut self, request_ptr: u64, register_id: u64) -> VMLogicResult<u32> {
        let Some(outbound) = self.borrow_logic().context.outbound.clone() else {
            return Err(HostError::OutboundUnavailable.into());
        };
        if self.borrow_logic().http_requests >= MAX_REQUESTS_PER_RUN {
            return Err(HostError::OutboundRequestsExceeded {
                max: MAX_REQUESTS_PER_RUN,
            }
            .into());
        }
        self.with_logic_mut(|logic| logic.http_requests += 1);

        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let request = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(request_ptr)? };
        if request.len() > MAX_ENCODED_REQUEST {
            return Err(HostError::ValueLengthOverflow.into());
        }
        let request = self.read_guest_memory_slice(&request)?.to_vec();

        let (status, body) = match outbound.send(&request) {
            Ok(response) => (1, response),
            Err(reason) => (0, reason.into_bytes()),
        };
        trace!(
            target: "runtime::host::outbound",
            status,
            request_len = request.len(),
            response_len = body.len(),
            "http_request"
        );
        self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, body))?;
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
    use crate::logic::{OutboundHost, VMContext, VMLimits, VMLogic, VMLogicError};

    /// Records every request and echoes it back, refusing `b"bad"`.
    #[derive(Debug, Default)]
    struct Echo(Mutex<Vec<Vec<u8>>>);

    impl OutboundHost for Echo {
        fn send(&self, request: &[u8]) -> Result<Vec<u8>, String> {
            self.0.lock().unwrap().push(request.to_vec());
            if request == b"bad" {
                return Err("refused".to_owned());
            }
            Ok(request.to_vec())
        }
    }

    fn run(
        outbound: Option<Arc<dyn OutboundHost>>,
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
        context.outbound = outbound;
        let mut store = Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(2, None, false)).unwrap();
        let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
        let _ = logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let data_ptr = 1024u64;
        host.borrow_memory().write(data_ptr, request).unwrap();
        let descriptor = 16u64;
        prepare_guest_buf_descriptor(&host, descriptor, data_ptr, request.len() as u64);

        (0..calls)
            .map(|_| {
                let status = host.http_request(descriptor, 1)?;
                let out = host.borrow_logic().registers.get(1).unwrap().to_vec();
                Ok((status, out))
            })
            .collect()
    }

    fn host_error(result: &VMLogicResult<(u32, Vec<u8>)>) -> &HostError {
        match result {
            Err(VMLogicError::HostError(e)) => e,
            other => panic!("expected a host error, got {other:?}"),
        }
    }

    #[test]
    fn without_the_node_s_client_the_call_traps() {
        let results = run(None, 1, b"anything");
        assert!(matches!(
            host_error(&results[0]),
            HostError::OutboundUnavailable
        ));
    }

    #[test]
    fn one_request_per_run_and_the_answer_lands_in_the_register() {
        let echo = Arc::new(Echo::default());
        let results = run(Some(echo.clone()), 2, b"hello");
        assert_eq!(results[0].as_ref().unwrap(), &(1, b"hello".to_vec()));
        assert!(matches!(
            host_error(&results[1]),
            HostError::OutboundRequestsExceeded { max: 1 }
        ));
        assert_eq!(
            echo.0.lock().unwrap().len(),
            1,
            "the second call never reached the node"
        );
    }

    #[test]
    fn a_refusal_is_a_reason_not_a_trap() {
        let results = run(Some(Arc::new(Echo::default())), 1, b"bad");
        assert_eq!(results[0].as_ref().unwrap(), &(0, b"refused".to_vec()));
    }

    #[test]
    fn an_oversized_request_never_reaches_the_node() {
        let echo = Arc::new(Echo::default());
        let big = vec![0u8; MAX_ENCODED_REQUEST as usize + 1];
        let results = run(Some(echo.clone()), 1, &big);
        assert!(matches!(
            host_error(&results[0]),
            HostError::ValueLengthOverflow
        ));
        assert!(echo.0.lock().unwrap().is_empty());
    }
}
