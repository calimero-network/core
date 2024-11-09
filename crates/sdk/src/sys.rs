#![allow(dead_code, reason = "Will be used in the future")]

mod types;

pub use types::*;

wasm_imports! {
    "env" => {
        fn panic(loc: Location<'_>) -> !;
        fn panic_utf8(msg: Buffer<'_>, loc: Location<'_>) -> !;
        // --
        fn register_len(register_id: RegisterId) -> PtrSizedInt;
        fn read_register(register_id: RegisterId, buf: BufferMut<'_>) -> Bool;
        // --
        fn context_id(register_id: RegisterId);
        fn executor_id(register_id: RegisterId);
        // --
        fn input(register_id: RegisterId);
        fn value_return(value: ValueReturn<'_>);
        fn log_utf8(msg: Buffer<'_>);
        fn emit(event: Event<'_>);
        // --
        fn commit(root: Buffer<'_>, artifact: Buffer<'_>);
        // --
        fn storage_read(key: Buffer<'_>, register_id: RegisterId) -> Bool;
        fn storage_remove(key: Buffer<'_>, register_id: RegisterId) -> Bool;
        fn storage_write(key: Buffer<'_>, value: Buffer<'_>, register_id: RegisterId) -> Bool;
        // --
        fn fetch(
            url: Buffer<'_>,
            method: Buffer<'_>,
            headers: Buffer<'_>,
            body: Buffer<'_>,
            register_id: RegisterId
        ) -> Bool;
        // --
        fn random_bytes(buf: BufferMut<'_>);
        fn time_now(buf: BufferMut<'_>);
        // --
        fn send_proposal(value: Buffer<'_>, buf: BufferMut<'_>);
        fn approve_proposal(value: Buffer<'_>);
    }
}

macro_rules! wasm_imports {
    ($module:literal => { $(fn $func_name:ident($($arg:ident: $arg_ty:ty),*) $(-> $returns:ty)?;)* }) => {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "wasm32")] {
                #[link(wasm_import_module = $module)]
                extern "C" {
                    $(
                        pub fn $func_name($($arg: $arg_ty),*) $(-> $returns)?;
                    )*
                }
            } else {
                $(
                    #[expect(unused_variables, reason = "Needed due to macro expansion")]
                    pub unsafe fn $func_name($($arg: $arg_ty),*) $(-> $returns)? {
                        panic!("host function `{}` is only available when compiled for wasm32", stringify!($func_name));
                    }
                )*
            }
        }
    };
}

use wasm_imports;
