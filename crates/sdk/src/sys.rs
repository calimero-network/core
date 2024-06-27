#![allow(dead_code)]

#[cfg(not(target_arch = "wasm32"))]
mod types;

pub use types::*;

macro_rules! wasm_imports {
    ($module:expr => { $(fn $func_name:ident($($arg:ident: $arg_ty:ty),*) $(-> $returns:ty)?;)* }) => {
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
                    #[no_mangle]
                    pub extern "C" fn $func_name($($arg: $arg_ty),*) $(-> $returns)? {
                        panic!("host function `{}` is only available when compiled for wasm32", stringify!($func_name));
                    }
                )*
            }
        }
    };
}

wasm_imports! {
    "env" => {
        fn panic(_loc: Location) -> !;
        fn panic_utf8(_msg: Buffer, _loc: Location) -> !;
        fn register_len(_register_id: RegisterId) -> PtrSizedInt;
        fn read_register(_register_id: RegisterId, _buf: BufferMut) -> Bool;
        fn input(_register_id: RegisterId);
        fn value_return(_value: ValueReturn);
        fn log_utf8(_msg: Buffer);
        fn emit(_event: Event);
        fn storage_read(_key: Buffer, _register_id: RegisterId) -> Bool;
        fn storage_write(_key: Buffer, _value: Buffer, _register_id: RegisterId) -> Bool;

        fn fetch(
            _url: Buffer,
            _method: Buffer,
            _headers: Buffer,
            _body: Buffer,
            _register_id: RegisterId
        ) -> Bool;
    }
}
