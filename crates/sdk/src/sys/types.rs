mod bool;
mod buffer;
mod pointer;
mod register;

pub use bool::*;
pub use buffer::*;
pub use pointer::*;
pub use register::*;

#[repr(C, u64)]
#[derive(Eq, Copy, Clone, Debug, PartialEq)]
pub enum ValueReturn<'a> {
    Ok(Buffer<'a>),
    Err(Buffer<'a>),
}

impl<T, E> From<Result<T, E>> for ValueReturn<'_>
where
    T: AsRef<[u8]>,
    E: AsRef<[u8]>,
{
    fn from(result: Result<T, E>) -> Self {
        match result {
            Ok(value) => ValueReturn::Ok(Buffer::from(value.as_ref())),
            Err(value) => ValueReturn::Err(Buffer::from(value.as_ref())),
        }
    }
}
