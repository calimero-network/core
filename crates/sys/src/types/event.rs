use super::Buffer;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
mod host;

#[repr(C)]
#[derive(Debug)]
pub struct Event<'a> {
    kind: Buffer<'a>,
    data: Buffer<'a>,
}
