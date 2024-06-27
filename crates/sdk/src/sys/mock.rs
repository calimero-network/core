use super::types::*;

#[no_mangle]
extern "C" fn panic(_loc: Location) -> ! {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn panic_utf8(_msg: Buffer, _loc: Location) -> ! {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn register_len(_register_id: RegisterId) -> PtrSizedInt {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn read_register(_register_id: RegisterId, _buf: BufferMut) -> Bool {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn input(_register_id: RegisterId) {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn value_return(_value: ValueReturn) {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn log_utf8(_msg: Buffer) {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn emit(_event: Event) {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn storage_read(_key: Buffer, _register_id: RegisterId) -> Bool {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn storage_write(_key: Buffer, _value: Buffer, _register_id: RegisterId) -> Bool {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}

#[no_mangle]
extern "C" fn fetch(
    _url: Buffer,
    _method: Buffer,
    _headers: Buffer,
    _body: Buffer,
    _register_id: RegisterId,
) -> Bool {
    panic!("Although your build was successful, it doesn't mean you're allowed to call this function in a non WASM target!")
}
