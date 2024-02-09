#![allow(dead_code)]

extern "C" {
    pub fn read_register(register_id: u64, ptr: u64);
    pub fn register_len(register_id: u64) -> u64;
    // --
    pub fn input(register_id: u64);
    // --
    pub fn panic() -> !;
    pub fn panic_utf8(len: u64, ptr: u64) -> !;
    pub fn value_return(value_len: u64, value_ptr: u64);
    pub fn log_utf8(len: u64, ptr: u64);
    // --
    pub fn storage_write(
        key_len: u64,
        key_ptr: u64,
        value_len: u64,
        value_ptr: u64,
        register_id: u64,
    ) -> u64;
    pub fn storage_read(key_len: u64, key_ptr: u64, register_id: u64) -> u64;
}
