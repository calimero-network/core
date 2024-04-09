#![allow(dead_code)]

mod types;

pub use types::*;

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
    // --
    pub fn storage_read(key: Buffer, register_id: RegisterId) -> Bool;
    pub fn storage_write(key: Buffer, value: Buffer, register_id: RegisterId) -> Bool;
}
