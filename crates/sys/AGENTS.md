# calimero-sys - WASM Host ABI

Low-level `#[repr(C)]` types and `extern "C"` host-import declarations shared by the WASM guest SDK and the runtime host that executes it.

## Package Identity

- **Crate**: `calimero-sys`
- **Entry**: `src/lib.rs`
- **Key deps**: `cfg-if` (only dependency; picks the wasm32 vs. host-native code path per type)
- **Consumers**: `calimero-sdk` (guest side - imports the `sys::*` functions as `unsafe extern "C"` calls from WASM app code), `calimero-runtime` (host side - reads/writes the same `#[repr(C)]` layouts out of guest linear memory via wasmer/wasmtime)

## Commands

```bash
# Build (native stub path - most types are unimplemented!() here, this only checks it compiles)
cargo build -p calimero-sys

# Build for the real target (guest path - this is what actually exercises the ABI)
cargo build -p calimero-sys --target wasm32-unknown-unknown

# Test
cargo test -p calimero-sys
```

There are no unit tests in this crate (no `#[test]` anywhere in `src/`) - correctness is exercised indirectly through `calimero-sdk` and `calimero-runtime` integration tests, since this crate is pure ABI plumbing with almost no logic of its own.

## Mental Model

This crate defines one wire format for the boundary between a WASM guest (an app compiled with `calimero-sdk`) and its host (the `calimero-runtime` VM). Every type in `src/types/` is `#[repr(C)]` so its layout is identical on both sides of that boundary, and every module with guest/host asymmetry is split into a `guest.rs` (real implementation, `#[cfg(target_arch = "wasm32")]`) and a `host.rs` (`unimplemented!()` stub, `#[cfg(not(target_arch = "wasm32"))]`). The stub side exists purely so the crate compiles as a normal dependency of host-side crates on non-wasm targets (needed for type-checking, `read_guest_memory_typed::<sys::Buffer<'_>>`, etc.) - calling a guest-only constructor natively panics immediately.

`src/lib.rs` declares the actual host import surface via the `wasm_imports!` macro: on `target_arch = "wasm32"` it emits `#[link(wasm_import_module = "env")] extern "C" { ... }` blocks (the real imports resolved by the WASM linker at instantiation); on any other target it emits safe-looking `pub unsafe fn` stubs that just `panic!("host function ... is only available when compiled for wasm32")`. `calimero-sdk`'s `env` module (`crates/sdk/src/env.rs`) is the only caller of these `sys::*` functions - it wraps each raw import in a safe, ergonomic API for app authors.

## Type Inventory

| Type | File | Purpose |
| --- | --- | --- |
| `Bool` | `types/bool.rs` | `#[repr(C)] struct Bool(u32)`; host booleans cross the ABI as `u32` (0/1), converted via `TryFrom<Bool> for bool` (any other value is `Err`) |
| `PtrSizedInt` | `types/pointer.rs` | `#[repr(C)] struct { value: u64 }`; pointer/size-sized integer, always 64 bits regardless of host/guest pointer width |
| `Pointer<T>` | `types/pointer.rs` | Typed wrapper around a `PtrSizedInt` address; `guest.rs` builds it from a real `*const T`/`*mut T`, `host.rs` only exposes `.value()` |
| `Ref<T>` | `types/ref.rs` | `#[repr(C)] struct { ptr: PtrSizedInt, .. }`; address-of wrapper passed by value to host imports instead of a raw `&T`, since `extern "C"` fns can't take Rust references |
| `RegisterId` | `types/register.rs` | Newtype over `PtrSizedInt` identifying a host-side scratch "register" that multi-step calls (e.g. `storage_read` then `read_register`) write into and read back from |
| `Slice<'a, T>` / `Buffer<'a>` / `BufferMut<'a>` | `types/buffer.rs` (+`guest.rs`/`host.rs`) | `#[repr(C)] struct { ptr: Pointer<T>, len: u64, .. }`; move-only borrowed-slice descriptor. `Buffer<'a> = Slice<'a, u8>`, `BufferMut<'a> = Buffer<'a>` (same type, mutability is by convention only) |
| `ValueReturn<'a>` | `types.rs` | `#[repr(C, u64)] enum { Ok(Buffer<'a>), Err(Buffer<'a>) }`; how app-logic function results cross into `value_return` |
| `Event<'a>` | `types/event.rs` (+`guest.rs`/`host.rs`) | `{ kind: Buffer<'a>, data: Buffer<'a> }`; payload for `emit`/`emit_with_handler` |
| `Location<'a>` | `types/location.rs` (+`guest.rs`/`host.rs`) | `{ file: Buffer<'a>, line: u32, column: u32 }`; guest panic-site info passed to `panic`/`panic_utf8`; `Location::caller()` wraps `core::panic::Location::caller()` |
| `XCall<'a>` | `types/xcall.rs` (+`guest.rs`/`host.rs`) | `{ context_id: Buffer<'a>, function: Buffer<'a>, params: Buffer<'a> }`; payload for the `xcall` cross-context call import |

## Host Import Surface (`src/lib.rs`)

Grouped by concern, as declared in the `wasm_imports!` block:

- **Panics**: `panic`, `panic_utf8`
- **Registers / context**: `register_len`, `read_register`, `context_id`, `executor_id`, `xcall_origin`
- **Execution I/O**: `input`, `value_return`, `emit_migration_witness`, `log_utf8`, `emit`, `emit_with_handler`
- **Cross-context calls**: `xcall`
- **Commit**: `commit`
- **Synchronized storage**: `storage_read`, `storage_remove`, `storage_write`
- **Node-local secondary index** (NOT synchronized): `storage_index_set`, `storage_index_remove`, `storage_index_remove_prefix`, `storage_index_scan`, `storage_index_last`
- **Node-local private storage** (NOT synchronized): `private_storage_read`, `private_storage_remove`, `private_storage_write`
- **Network/misc host services**: `fetch`, `random_bytes`, `time_now`, `ed25519_verify`
- **Streaming blobs**: `blob_create`, `blob_open`, `blob_read`, `blob_write`, `blob_close`
- **Network blobs**: `blob_announce_to_context`

Every import is called `unsafe` from the guest side (raw FFI). `calimero-sdk`'s `env` module is the sole intended caller - app code should never call `calimero_sys::*` directly.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `wasm_imports!` macro + the full host import list |
| `src/types.rs` | Module wiring, `ValueReturn`, the 64-bit/wasm32 `compile_error!` guard |
| `src/types/buffer.rs` | `Slice<'a, T>` / `Buffer` / `BufferMut` - the move-only descriptor everything else builds on |
| `src/types/pointer.rs` | `PtrSizedInt`, `Pointer<T>` |
| `src/types/ref.rs` | `Ref<T>` (address-of wrapper for extern "C" args) |
| `src/types/register.rs` | `RegisterId` |
| `src/types/{event,location,xcall}.rs` | ABI payload structs for `emit`, panics, and `xcall` |

## Invariants and Gotchas

- **This crate is not the safe API.** `calimero-sdk::env` is; `calimero-sys` is raw FFI declarations and layout structs only. Never call `sys::*` from app logic - go through `calimero_sdk::env`.
- **`Slice`/`Buffer` is deliberately not `Copy`/`Clone`.** It owns a raw pointer + lifetime and hands out `&mut [T]` via `as_mut_slice`; a duplicable descriptor would let safe code mint two aliasing `&mut [T]` over the same memory. Being move-only plus tying borrows to `&self` (not `'a`) is what makes the borrow checker enforce single access. Do not add `Clone`/`Copy` to `Slice`.
- **`ValueReturn` is not `Copy`/`Clone` for the same reason** - it wraps a move-only `Buffer`.
- **`as_slice`/`as_mut_slice` on an empty `Slice` never call `from_raw_parts`** - the empty descriptor carries a null `ptr`, and `from_raw_parts` on a null pointer is UB even at `len == 0`. Always check `is_empty()` before adding new slice-materializing code paths.
- **The host-native build path is intentionally broken at runtime, not compile time.** Every `host.rs` implements the same signatures as `guest.rs` but panics/`unimplemented!()`s on call. This lets host-side crates (`calimero-runtime`) type-check against `sys::Buffer`, `sys::Event`, etc. without pulling in a second copy of the type - do not "fix" a host stub to actually work; host code is meant to read/write these layouts directly out of guest memory, not construct them via guest constructors.
- **`#[repr(C)]` on every type is load-bearing.** These structs are read by the runtime directly out of WASM linear memory at a raw byte offset (see `calimero-runtime`'s `read_guest_memory_typed::<sys::Buffer<'_>>`); reordering fields or dropping `#[repr(C)]` silently breaks the host/guest contract with no compiler error on either side.
- **`storage_index_*` and `private_storage_*` are explicitly node-local and NOT synchronized** across peers - unlike `storage_read`/`storage_write`, which participate in the CRDT sync protocol. Don't assume symmetry between these families.
- **The crate hard-fails to compile off 64-bit/wasm32 targets** via the `compile_error!` in `types.rs` - `PtrSizedInt` assumes a 64-bit address space (or wasm32, which is 32-bit but has its own dedicated path).

Part of [crates/](../AGENTS.md).
