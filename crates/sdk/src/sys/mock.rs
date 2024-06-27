use super::types::*;

macro_rules! non_wasm_panic {
    (fn $func:ident($($arg:ident: $arg_ty:ty),*$(,)?) $(-> $returns:ty)?;) => {
        #[no_mangle]
        extern "C" fn $func($($arg: $arg_ty),*) $(-> $returns)? {
            panic!("host function `{}` is only available when compiled for wasm32", stringify!($func_name));
        }
    };
}

non_wasm_panic!(
    fn panic(_loc: Location) -> !;
);
non_wasm_panic!(
    fn panic_utf8(_msg: Buffer, _loc: Location) -> !;
);
non_wasm_panic!(
    fn register_len(_register_id: RegisterId) -> PtrSizedInt;
);
non_wasm_panic!(
    fn read_register(_register_id: RegisterId, _buf: BufferMut) -> Bool;
);
non_wasm_panic!(
    fn input(_register_id: RegisterId);
);
non_wasm_panic!(
    fn value_return(_value: ValueReturn);
);
non_wasm_panic!(
    fn log_utf8(_msg: Buffer);
);
non_wasm_panic!(
    fn emit(_event: Event);
);
non_wasm_panic!(
    fn storage_read(_key: Buffer, _register_id: RegisterId) -> Bool;
);
non_wasm_panic!(
    fn storage_write(_key: Buffer, _value: Buffer, _register_id: RegisterId) -> Bool;
);
non_wasm_panic!(
    fn fetch(
        _url: Buffer,
        _method: Buffer,
        _headers: Buffer,
        _body: Buffer,
        _register_id: RegisterId,
    ) -> Bool;
);
