#![allow(dead_code)]

mod types;

pub use types::*;

wasm_imports! {
    "env" => {
        fn panic(loc: Location) -> !;
        fn panic_utf8(msg: Buffer, loc: Location) -> !;
        // --
        fn register_len(register_id: RegisterId) -> PtrSizedInt;
        fn read_register(register_id: RegisterId, buf: BufferMut) -> Bool;
        // --
        fn input(register_id: RegisterId);
        fn value_return(value: ValueReturn);
        fn log_utf8(msg: Buffer);
        fn emit(event: Event);
        // --
        fn storage_read(key: Buffer, register_id: RegisterId) -> Bool;
        fn storage_write(key: Buffer, value: Buffer, register_id: RegisterId) -> Bool;
        // --
        fn fetch(
            url: Buffer,
            method: Buffer,
            headers: Buffer,
            body: Buffer,
            register_id: RegisterId
        ) -> Bool;
        fn get_executor_identity(register_id: RegisterId);
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
                    #[allow(unused_variables)]
                    pub unsafe fn $func_name($($arg: $arg_ty),*) $(-> $returns)? {
                        panic!("host function `{}` is only available when compiled for wasm32", stringify!($func_name));
                    }
                )*
            }
        }
    };
}

use wasm_imports;
