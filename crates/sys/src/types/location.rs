use crate::Buffer;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
mod host;

#[repr(C)]
#[derive(Debug)]
pub struct Location<'a> {
    file: Buffer<'a>,
    line: u32,
    column: u32,
}
