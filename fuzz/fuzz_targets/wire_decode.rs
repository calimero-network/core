#![no_main]
//! The decoders a node runs on the first bytes a peer sends: a gossip message, a
//! sync stream frame, and the first frame of the blob transfer and announce protocols.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use calimero_network_primitives::blob_types::{BlobAnnouncement, BlobRequest};
use calimero_node_primitives::sync::snapshot::BroadcastMessage;
use calimero_node_primitives::sync::StreamMessage;
use libfuzzer_sys::fuzz_target;

const ALLOC_PER_INPUT_BYTE: usize = 64; // what one decoded byte may cost in memory
const ALLOC_SLACK: usize = (1024 + 64) * 1024; // borsh's 1 MiB byte-vector preallocation, plus headroom

static LIVE: AtomicUsize = AtomicUsize::new(0); // bytes allocated and not yet freed
static PEAK: AtomicUsize = AtomicUsize::new(0); // highest `LIVE` since the last reset

struct Counting;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        grow(new_size);
        let _ = LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn grow(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    let _ = PEAK.fetch_max(live, Ordering::Relaxed);
}

/// Runs `decode` and asserts the memory it held at its peak stays proportional to `input`.
fn bounded<T>(what: &str, input: &[u8], decode: impl FnOnce(&[u8]) -> T) {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let decoded = decode(input);
    let held = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    drop(decoded);
    let limit = input.len() * ALLOC_PER_INPUT_BYTE + ALLOC_SLACK;
    assert!(
        held <= limit,
        "decoding {} bytes as {what} held {held} bytes, over {limit}",
        input.len()
    );
}

fuzz_target!(|data: &[u8]| {
    bounded("a gossip message", data, |bytes| {
        borsh::from_slice::<BroadcastMessage<'_>>(bytes)
    });
    bounded("a sync stream frame", data, |bytes| {
        borsh::from_slice::<StreamMessage<'static>>(bytes)
    });
    bounded("a blob request", data, |bytes| {
        serde_json::from_slice::<BlobRequest>(bytes)
    });
    bounded("a blob announcement", data, |bytes| {
        serde_json::from_slice::<BlobAnnouncement>(bytes)
    });
});
