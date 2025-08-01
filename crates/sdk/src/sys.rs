#![allow(dead_code, reason = "Will be used in the future")]

mod types;

pub use types::*;

wasm_imports! {
    "env" => {
        fn panic(loc: Location<'_>) -> !;
        fn panic_utf8(msg: Ref<Buffer<'_>>, loc: Ref<Location<'_>>) -> !;
        // --
        fn register_len(register_id: RegisterId) -> PtrSizedInt;
        fn read_register(register_id: RegisterId, buf: Ref<BufferMut<'_>>) -> Bool;
        // --
        fn context_id(register_id: RegisterId);
        fn executor_id(register_id: RegisterId);
        // --
        fn input(register_id: RegisterId);
        fn value_return(value: Ref<ValueReturn<'_>>);
        fn log_utf8(msg: Ref<Buffer<'_>>);
        fn emit(event: Ref<Event<'_>>);
        // --
        fn commit(root: Ref<Buffer<'_>>, artifact: Ref<Buffer<'_>>);
        // --
        fn storage_read(key: Ref<Buffer<'_>>, register_id: RegisterId) -> Bool;
        fn storage_remove(key: Ref<Buffer<'_>>, register_id: RegisterId) -> Bool;
        fn storage_write(key: Ref<Buffer<'_>>, value: Ref<Buffer<'_>>, register_id: RegisterId) -> Bool;
        // --
        fn fetch(
            url: Ref<Buffer<'_>>,
            method: Ref<Buffer<'_>>,
            headers: Ref<Buffer<'_>>,
            body: Ref<Buffer<'_>>,
            register_id: RegisterId
        ) -> Bool;
        // --
        fn random_bytes(buf: Ref<BufferMut<'_>>);
        fn time_now(buf: Ref<BufferMut<'_>>);
        // --
        fn send_proposal(value: Ref<Buffer<'_>>, buf: Ref<BufferMut<'_>>);
        fn approve_proposal(value: Ref<Buffer<'_>>);
        // --
        // Streaming blob functions
        fn blob_create() -> PtrSizedInt;
        fn blob_open(blob_id: Ref<Buffer<'_>>) -> PtrSizedInt;
        fn blob_read(fd: PtrSizedInt, buf: Ref<BufferMut<'_>>) -> PtrSizedInt;
        fn blob_write(fd: PtrSizedInt, data: Ref<Buffer<'_>>) -> PtrSizedInt;
        fn blob_close(fd: PtrSizedInt, blob_id_buf: Ref<BufferMut<'_>>) -> Bool;
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
