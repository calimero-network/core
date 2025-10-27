use super::Buffer;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
mod host;

#[repr(C)]
#[derive(Debug)]
pub struct XCall<'a> {
    context_id: Buffer<'a>,
    function: Buffer<'a>,
    params: Buffer<'a>,
}
