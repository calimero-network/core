use core::ptr::NonNull;

use wasmer::sys::vm::{VMConfig, VMMemory, VMMemoryDefinition, VMTable, VMTableDefinition};
use wasmer::sys::{BaseTunables, Tunables};
use wasmer_types::{MemoryError, MemoryStyle, MemoryType, TableStyle, TableType};

use crate::logic::VMLimits;

/// Custom tunables for the Wasmer runtime that carry the guest stack limit.
///
/// Memory and table construction delegate to `BaseTunables`; only `vmconfig` is
/// ours. While `WasmerTunables` creates memory through the `Tunables` trait
/// methods, the actual memory ownership is transferred to Wasmer's `Store`.
///
/// # Memory Management
///
/// Memory allocated through `create_host_memory` and `create_vm_memory` is owned
/// by the Wasmer `Store` and `Instance`. Cleanup occurs when:
/// - The `Store` is dropped (cleans up all associated resources)
/// - Individual `Instance` objects are dropped
/// - `VMLogic::drop` is called (explicitly releases memory references)
///
/// This struct does not perform explicit cleanup. Memory management is handled
/// by Wasmer's `Store` and the `VMLogic::finish()` implementation.
pub struct WasmerTunables {
    base: BaseTunables,
    vmconfig: VMConfig,
}

impl WasmerTunables {
    pub fn new(limits: &VMLimits) -> Self {
        let base = BaseTunables::new();

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
