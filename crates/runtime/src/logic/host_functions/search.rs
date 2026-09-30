//! `search_query`: the node's full-text search, from a view.

use tracing::trace;

use crate::errors::HostError;
use crate::logic::{sys, SearchOutput, VMHostFunctions, VMLogicResult};

/// `search_query` calls one execution may make. Each call is charged gas for
/// its work, and each is itself bounded (a page is at most 100 hits, a query
/// at most 256 bytes); the count bounds the work a run can make the host do
/// before the gas it owes is settled.
pub const MAX_SEARCH_CALLS: u64 = 32;

/// Largest request a guest may pass, in bytes.
const MAX_SEARCH_REQUEST: u64 = 4096;

/// Gas every call pays, whatever it finds: request decode, the index lookup,
/// the term-dictionary walk, and a searcher over the last commit.
///
/// The four constants turn measured host time into the gas the same time
/// costs the guest. On the reference machine the guest runs about 1.76 gas
/// per ns (the in-WASM scan baseline: 634 M gas in 360 ms). A least-squares
/// fit of `search_query`'s host work (the query and the borsh response) over
/// 2,608 queries of every mode against indexes of 2,000 to 200,000 documents
/// (`tools/search-bench`, the `gas` section of its README) gives 4.4 µs a
/// call, 13.9 ns a matched document, 10.5 µs a returned hit and 70 ns a
/// response byte; each constant is that time in gas, rounded.
pub const SEARCH_BASE_GAS: u64 = 8_000;

/// Gas per document the query matched and scored (tantivy visits every
/// matching document to count and rank it).
pub const SEARCH_GAS_PER_MATCH: u64 = 25;

/// Gas per hit returned: its stored fields read back and highlighted.
pub const SEARCH_GAS_PER_HIT: u64 = 18_500;

/// Gas per byte of response the host encodes and writes into the register.
pub const SEARCH_GAS_PER_BYTE: u64 = 125;

/// The gas one call costs, from the work it reports.
#[must_use]
pub fn search_gas(output: &SearchOutput) -> u64 {
    SEARCH_BASE_GAS
        .saturating_add(output.matched.saturating_mul(SEARCH_GAS_PER_MATCH))
        .saturating_add(output.hits.saturating_mul(SEARCH_GAS_PER_HIT))
        .saturating_add((output.response.len() as u64).saturating_mul(SEARCH_GAS_PER_BYTE))
}

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
    /// The call costs [`search_gas`] of the work it did (a refused one pays
    /// [`SEARCH_BASE_GAS`] and its reason's bytes), taken off the run's budget
    /// as the call returns. Views are the only runs that can call it and they
    /// never replicate, so this gas, which depends on this node's index, never
    /// has to agree between nodes.
    ///
    /// # Errors
    ///
    /// * `HostError::SearchUnavailable` outside a view, or on a node without
    ///   search — the node supplies the search handle to read-only runs only.
    /// * `HostError::SearchCallsExceeded` past [`MAX_SEARCH_CALLS`].
    /// * `HostError::HostGasExhausted` when the call's gas is more than the
    ///   run has left; the run then reports `GasExhausted`.
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

        let (status, output) = match search.search(context, &request) {
            Ok(output) => (1, output),
            Err(reason) => (
                0,
                SearchOutput {
                    response: reason.into_bytes(),
                    ..SearchOutput::default()
                },
            ),
        };
        let gas = search_gas(&output);
        trace!(
            target: "runtime::host::search",
            status,
            matched = output.matched,
            hits = output.hits,
            response_len = output.response.len(),
            gas,
            "search_query"
        );
        self.with_logic_mut(|logic| {
            logic.owe_gas(gas);
            logic
                .registers
                .set(logic.limits, register_id, output.response)
        })?;
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

    /// Records which context each call was bound to, and echoes the request
    /// as a response that matched 1,000 documents and returned 10 hits.
    #[derive(Debug, Default)]
    struct Recorder(Mutex<Vec<[u8; 32]>>);

    impl SearchHost for Recorder {
        fn search(&self, context: [u8; 32], request: &[u8]) -> Result<SearchOutput, String> {
            self.0.lock().unwrap().push(context);
            if request == b"bad" {
                return Err("refused".to_owned());
            }
            Ok(SearchOutput {
                response: request.to_vec(),
                matched: 1_000,
                hits: 10,
            })
        }
    }

    type Call = VMLogicResult<(u32, Vec<u8>, u64)>;

    fn run(search: Option<Arc<dyn SearchHost>>, calls: usize, request: &[u8]) -> Vec<Call> {
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
                let owed = host.with_logic_mut(|logic| core::mem::take(&mut logic.host_gas_owed));
                Ok((status, out, owed))
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
            let (status, out, _) = result.unwrap();
            assert_eq!((status, out), (1, b"any request".to_vec()));
        }
        assert_eq!(*recorder.0.lock().unwrap(), vec![[0xC1; 32]; 2]);
        let results = run(Some(recorder), 1, b"bad");
        let (status, out, _) = results[0].as_ref().unwrap();
        assert_eq!((*status, out.as_slice()), (0, &b"refused"[..]));
    }

    #[test]
    fn every_call_owes_its_fixed_cost_plus_its_work() {
        let results = run(Some(Arc::new(Recorder::default())), 1, b"any request");
        let (_, _, owed) = results[0].as_ref().unwrap();
        assert_eq!(
            *owed,
            SEARCH_BASE_GAS
                + 1_000 * SEARCH_GAS_PER_MATCH
                + 10 * SEARCH_GAS_PER_HIT
                + 11 * SEARCH_GAS_PER_BYTE
        );
        // A refused request did no index work: the fixed cost and its reason.
        let results = run(Some(Arc::new(Recorder::default())), 1, b"bad");
        let (_, _, owed) = results[0].as_ref().unwrap();
        assert_eq!(*owed, SEARCH_BASE_GAS + 7 * SEARCH_GAS_PER_BYTE);
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
