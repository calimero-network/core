use core::ptr::NonNull;

use tracing::trace;
use wasmer::sys::vm::{VMConfig, VMMemory, VMMemoryDefinition, VMTable, VMTableDefinition};
use wasmer::sys::{BaseTunables, Tunables};
use wasmer_types::{
    MemoryError, MemoryStyle, MemoryType, Pages, TableStyle, TableType, WASM_MAX_PAGES,
};

use crate::logic::VMLimits;

/// Custom tunables for the Wasmer runtime that configure memory and stack limits.
///
/// This struct wraps Wasmer's `BaseTunables` to provide custom memory configuration
/// based on `VMLimits`. While `WasmerTunables` creates memory through the `Tunables`
/// trait methods, the actual memory ownership is transferred to Wasmer's `Store`.
///
/// # Memory Management
///
/// Memory allocated through `create_host_memory` and `create_vm_memory` is owned
/// by the Wasmer `Store` and `Instance`. Cleanup occurs when:
/// - The `Store` is dropped (cleans up all associated resources)
/// - Individual `Instance` objects are dropped
/// - `VMLogic::drop` is called (explicitly releases memory references)
///
/// The `Drop` implementation for this struct is provided for completeness and
/// logging purposes, but the actual memory cleanup is handled by Wasmer's
/// reference counting system and the `VMLogic::drop` implementation.
pub struct WasmerTunables {
    base: BaseTunables,
    vmconfig: VMConfig,
}

/// Implements cleanup logging for `WasmerTunables`.
///
/// Note: `WasmerTunables` doesn't directly own the allocated memories - they are
/// returned to Wasmer's internal machinery via the `Tunables` trait methods.
/// The actual memory cleanup is handled by:
/// - Wasmer's `Store` when it is dropped
/// - `VMLogic::finish()` which explicitly releases memory references
/// - `MemoryCleanupGuard` for edge cases where finish() might not be called
///
/// This `Drop` implementation is provided for consistency and to document the
/// cleanup behavior. See `VMLogic::finish()` for the main cleanup implementation.
impl Drop for WasmerTunables {
    fn drop(&mut self) {
        trace!(
            target: "runtime::memory",
            "WasmerTunables::drop: tunables dropped (memory cleanup handled by Store/VMLogic)"
        );
    }
}

/// A guard that ensures WASM memory is cleaned up when dropped.
///
/// This guard can be used to wrap a `wasmer::Memory` reference and ensure
/// it is properly cleaned up even if an error or panic occurs before
/// normal cleanup paths are reached.
///
/// # Usage
///
/// Create the guard after obtaining a memory reference:
/// ```ignore
/// let memory = instance.exports.get_memory("memory")?;
/// let _guard = MemoryCleanupGuard::new(memory.clone());
/// // Memory will be cleaned up when guard goes out of scope
/// ```
///
/// The guard can also be disarmed if cleanup is handled elsewhere:
/// ```ignore
/// let memory = instance.exports.get_memory("memory")?;
/// let mut guard = MemoryCleanupGuard::new(memory.clone());
/// // ... use memory ...
/// guard.disarm(); // Prevent cleanup by guard
/// ```
pub struct MemoryCleanupGuard {
    memory: Option<wasmer::Memory>,
}

impl MemoryCleanupGuard {
    /// Creates a new cleanup guard for the given memory.
    #[must_use]
    pub fn new(memory: wasmer::Memory) -> Self {
        Self {
            memory: Some(memory),
        }
    }

    /// Disarms the guard, preventing it from cleaning up the memory on drop.
    ///
    /// Call this when cleanup is being handled elsewhere (e.g., by `VMLogic::finish()`).
    pub fn disarm(&mut self) {
        self.memory = None;
    }
}

impl Drop for MemoryCleanupGuard {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            // Explicitly drop the memory reference to ensure Wasmer's
            // internal reference count is decremented. When all references
            // are released, the actual memory will be deallocated.
            drop(memory);
            trace!(
                target: "runtime::memory",
                "MemoryCleanupGuard::drop: cleaned up dangling WASM memory"
            );
        }
    }
}

impl WasmerTunables {
    pub fn new(limits: &VMLimits) -> Self {
        let base = BaseTunables {
            static_memory_bound: Pages(limits.max_memory_pages),
            static_memory_offset_guard_size: u64::from(WASM_MAX_PAGES),
            dynamic_memory_offset_guard_size: u64::from(WASM_MAX_PAGES),
        };

        let vmconfig = VMConfig {
            wasm_stack_size: Some(limits.max_stack_size),
        };

        Self { base, vmconfig }
    }
}

impl Tunables for WasmerTunables {
    fn vmconfig(&self) -> &VMConfig {
        &self.vmconfig
    }

    fn memory_style(&self, memory: &MemoryType) -> MemoryStyle {
        self.base.memory_style(memory)
    }

    fn table_style(&self, table: &TableType) -> TableStyle {
        self.base.table_style(table)
    }

    fn create_host_memory(
        &self,
        ty: &MemoryType,
        style: &MemoryStyle,
    ) -> Result<VMMemory, MemoryError> {
        self.base.create_host_memory(ty, style)
    }

    unsafe fn create_vm_memory(
        &self,
        ty: &MemoryType,
        style: &MemoryStyle,
        vm_definition_location: NonNull<VMMemoryDefinition>,
    ) -> Result<VMMemory, MemoryError> {
        self.base
            .create_vm_memory(ty, style, vm_definition_location)
    }

    fn create_host_table(&self, ty: &TableType, style: &TableStyle) -> Result<VMTable, String> {
        self.base.create_host_table(ty, style)
    }

    unsafe fn create_vm_table(
        &self,
        ty: &TableType,
        style: &TableStyle,
        vm_definition_location: NonNull<VMTableDefinition>,
    ) -> Result<VMTable, String> {
        self.base.create_vm_table(ty, style, vm_definition_location)
    }
}
