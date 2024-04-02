use std::ptr::NonNull;

use crate::logic::VMLimits;

pub struct WasmerTunables {
    base: wasmer::sys::BaseTunables,
    vmconfig: wasmer::sys::VMConfig,
}

impl WasmerTunables {
    pub fn new(limits: &VMLimits) -> Self {
        let base = wasmer::sys::BaseTunables {
            static_memory_bound: wasmer_types::Pages(limits.max_memory_pages),
            static_memory_offset_guard_size: wasmer_types::WASM_MAX_PAGES as _,
            dynamic_memory_offset_guard_size: wasmer_types::WASM_MAX_PAGES as _,
        };

        let vmconfig = wasmer::sys::VMConfig {
            wasm_stack_size: Some(limits.max_stack_size),
        };

        Self { base, vmconfig }
    }
}

impl wasmer::Tunables for WasmerTunables {
    fn vmconfig(&self) -> &wasmer::sys::VMConfig {
        &self.vmconfig
    }

    fn memory_style(&self, memory: &wasmer_types::MemoryType) -> wasmer_types::MemoryStyle {
        self.base.memory_style(memory)
    }

    fn table_style(&self, table: &wasmer_types::TableType) -> wasmer_types::TableStyle {
        self.base.table_style(table)
    }

    fn create_host_memory(
        &self,
        ty: &wasmer_types::MemoryType,
        style: &wasmer_types::MemoryStyle,
    ) -> Result<wasmer::vm::VMMemory, wasmer_types::MemoryError> {
        self.base.create_host_memory(ty, style)
    }

    unsafe fn create_vm_memory(
        &self,
        ty: &wasmer_types::MemoryType,
        style: &wasmer_types::MemoryStyle,
        vm_definition_location: NonNull<wasmer::vm::VMMemoryDefinition>,
    ) -> Result<wasmer::vm::VMMemory, wasmer_types::MemoryError> {
        self.base
            .create_vm_memory(ty, style, vm_definition_location)
    }

    fn create_host_table(
        &self,
        ty: &wasmer_types::TableType,
        style: &wasmer_types::TableStyle,
    ) -> Result<wasmer::vm::VMTable, String> {
        self.base.create_host_table(ty, style)
    }

    unsafe fn create_vm_table(
        &self,
        ty: &wasmer_types::TableType,
        style: &wasmer_types::TableStyle,
        vm_definition_location: NonNull<wasmer::vm::VMTableDefinition>,
    ) -> Result<wasmer::vm::VMTable, String> {
        self.base.create_vm_table(ty, style, vm_definition_location)
    }
}
