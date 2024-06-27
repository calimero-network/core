use super::types::*;

extern "C" fn panic(loc: Location) -> ! {
    todo!()
}

extern "C" fn panic_utf8(msg: Buffer, loc: Location) -> ! {
    todo!()
}

extern "C" fn register_len(register_id: RegisterId) -> PtrSizedInt {
    todo!()
}

extern "C" fn read_register(register_id: RegisterId, buf: BufferMut) -> Bool {
    todo!()
}
extern "C" fn input(register_id: RegisterId) {
    todo!()
}
extern "C" fn value_return(value: ValueReturn) {
    todo!()
}
extern "C" fn log_utf8(msg: Buffer) {
    todo!()
}
extern "C" fn emit(event: Event) {
    todo!()
}
extern "C" fn storage_read(key: Buffer, register_id: RegisterId) -> Bool {
    todo!()
}
extern "C" fn storage_write(key: Buffer, value: Buffer, register_id: RegisterId) -> Bool {
    todo!()
}

extern "C" fn fetch(
    url: Buffer,
    method: Buffer,
    headers: Buffer,
    body: Buffer,
    register_id: RegisterId,
) -> Bool {
    todo!()
}
