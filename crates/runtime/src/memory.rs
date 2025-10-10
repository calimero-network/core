use core::ptr::NonNull;

use wasmer::sys::{BaseTunables, Tunables};
use wasmer_types::{
    MemoryError, MemoryStyle, MemoryType, Pages, TableStyle, TableType, WASM_MAX_PAGES,
};
use wasmer_vm::{VMConfig, VMMemory, VMMemoryDefinition, VMTable, VMTableDefinition};

use crate::logic::VMLimits;

pub struct WasmerTunables {
    base: BaseTunables,
    vmconfig: VMConfig,
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
