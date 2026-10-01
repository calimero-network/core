use core::ptr::NonNull;

use wasmer::sys::vm::{VMConfig, VMMemory, VMMemoryDefinition, VMTable, VMTableDefinition};
use wasmer::sys::{BaseTunables, Tunables};
use wasmer_types::{
    MemoryError, MemoryStyle, MemoryType, Pages, TableStyle, TableType, WASM_MAX_PAGES,
};

use crate::logic::VMLimits;

/// Custom tunables for the Wasmer runtime that configure memory, table and stack limits.
///
/// This struct wraps Wasmer's `BaseTunables` to provide custom memory configuration
/// based on `VMLimits`. Every memory and table is created here, whether the module was
/// compiled or restored from the precompiled cache, so a guest's declared maximum can
/// only lower `max_memory_pages` and `max_table_elements`, never raise them. A memory
/// or table whose minimum is above the cap, and any shared memory, is refused at
/// instantiation. While `WasmerTunables` creates memory through the `Tunables`
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
/// This struct does not perform explicit cleanup. Memory management is handled
/// by Wasmer's `Store` and the `VMLogic::finish()` implementation.
pub struct WasmerTunables {
    base: BaseTunables,
    vmconfig: VMConfig,
    max_memory_pages: Pages,
    max_table_elements: u32,
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

        Self {
            base,
            vmconfig,
            max_memory_pages: Pages(limits.max_memory_pages),
            max_table_elements: limits.max_table_elements,
        }
    }

    /// The memory type the guest actually gets: its declared maximum, capped at
    /// `max_memory_pages`. A minimum above the cap is refused.
    fn bounded_memory(&self, ty: &MemoryType) -> Result<MemoryType, MemoryError> {
        if ty.shared {
            return Err(MemoryError::InvalidMemory {
                reason: "shared memories are not supported".to_owned(),
            });
        }
        if ty.minimum > self.max_memory_pages {
            return Err(MemoryError::MinimumMemoryTooLarge {
                min_requested: ty.minimum,
                max_allowed: self.max_memory_pages,
            });
        }

        let maximum = ty
            .maximum
            .map_or(self.max_memory_pages, |max| max.min(self.max_memory_pages));

        Ok(MemoryType {
            maximum: Some(maximum),
            ..*ty
        })
    }

    /// The table type the guest actually gets: its declared maximum, capped at
    /// `max_table_elements`. A minimum above the cap is refused.
    fn bounded_table(&self, ty: &TableType) -> Result<TableType, String> {
        if ty.minimum > self.max_table_elements {
            return Err(format!(
                "table minimum ({}) exceeds the limit of {} elements",
                ty.minimum, self.max_table_elements
            ));
        }

        let maximum = ty.maximum.map_or(self.max_table_elements, |max| {
            max.min(self.max_table_elements)
        });

        Ok(TableType {
            maximum: Some(maximum),
            ..*ty
        })
    }
}

impl Tunables for WasmerTunables {
    fn vmconfig(&self) -> &VMConfig {
        &self.vmconfig
    }

    fn memory_style(&self, memory: &MemoryType) -> MemoryStyle {
        // The declared type is safe to use here: the memory created from it is
        // never larger than it (see `bounded_memory`).
        self.base.memory_style(memory)
    }

    fn table_style(&self, table: &TableType) -> TableStyle {
        // Safe for the same reason: the table created never exceeds the declared type.
        self.base.table_style(table)
    }

    fn create_host_memory(
        &self,
        ty: &MemoryType,
        style: &MemoryStyle,
    ) -> Result<VMMemory, MemoryError> {
        self.base
            .create_host_memory(&self.bounded_memory(ty)?, style)
    }

    unsafe fn create_vm_memory(
        &self,
        ty: &MemoryType,
        style: &MemoryStyle,
        vm_definition_location: NonNull<VMMemoryDefinition>,
    ) -> Result<VMMemory, MemoryError> {
        self.base
            .create_vm_memory(&self.bounded_memory(ty)?, style, vm_definition_location)
    }

    fn create_host_table(&self, ty: &TableType, style: &TableStyle) -> Result<VMTable, String> {
        self.base.create_host_table(&self.bounded_table(ty)?, style)
    }

    unsafe fn create_vm_table(
        &self,
        ty: &TableType,
        style: &TableStyle,
        vm_definition_location: NonNull<VMTableDefinition>,
    ) -> Result<VMTable, String> {
        self.base
            .create_vm_table(&self.bounded_table(ty)?, style, vm_definition_location)
    }
}
