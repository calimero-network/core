#![allow(dead_code)]

#[cfg(not(target_arch = "wasm32"))]
mod mock;
mod types;

pub use types::*;

#[link(wasm_import_module = "env")]
extern "C" {
    pub fn panic(loc: Location) -> !;
    pub fn panic_utf8(msg: Buffer, loc: Location) -> !;
    // --
    pub fn register_len(register_id: RegisterId) -> PtrSizedInt;
    pub fn read_register(register_id: RegisterId, buf: BufferMut) -> Bool;
    // --
    pub fn input(register_id: RegisterId);
    pub fn value_return(value: ValueReturn);
    pub fn log_utf8(msg: Buffer);
    pub fn emit(event: Event);
    // --
    pub fn storage_read(key: Buffer, register_id: RegisterId) -> Bool;
    pub fn storage_write(key: Buffer, value: Buffer, register_id: RegisterId) -> Bool;

    pub fn fetch(
        url: Buffer,
        method: Buffer,
        headers: Buffer,
        body: Buffer,
        register_id: RegisterId,
    ) -> Bool;
}
