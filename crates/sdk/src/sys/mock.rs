use super::types::*;

#[no_mangle]
extern "C" fn panic(loc: Location) -> ! {
    todo!()
}

#[no_mangle]
extern "C" fn panic_utf8(msg: Buffer, loc: Location) -> ! {
    todo!()
}

#[no_mangle]
extern "C" fn register_len(register_id: RegisterId) -> PtrSizedInt {
    todo!()
}

#[no_mangle]
extern "C" fn read_register(register_id: RegisterId, buf: BufferMut) -> Bool {
    todo!()
}

#[no_mangle]
extern "C" fn input(register_id: RegisterId) {
    todo!()
}

#[no_mangle]
extern "C" fn value_return(value: ValueReturn) {
    todo!()
}

#[no_mangle]
extern "C" fn log_utf8(msg: Buffer) {
    todo!()
}

#[no_mangle]
extern "C" fn emit(event: Event) {
    todo!()
}

#[no_mangle]
extern "C" fn storage_read(key: Buffer, register_id: RegisterId) -> Bool {
    todo!()
}

#[no_mangle]
extern "C" fn storage_write(key: Buffer, value: Buffer, register_id: RegisterId) -> Bool {
    todo!()
}

#[no_mangle]
extern "C" fn fetch(
    url: Buffer,
    method: Buffer,
    headers: Buffer,
    body: Buffer,
    register_id: RegisterId,
) -> Bool {
    todo!()
}
