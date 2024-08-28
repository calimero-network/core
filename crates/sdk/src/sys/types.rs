mod bool;
mod buffer;
mod event;
mod location;
mod pointer;
mod register;

pub use bool::*;
pub use buffer::*;
pub use event::*;
pub use location::*;
pub use pointer::*;
pub use register::*;

#[repr(C, u64)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueReturn<'a> {
    Ok(Buffer<'a>),
    Err(Buffer<'a>),
}

impl<'a, T, E> From<Result<&'a T, &'a E>> for ValueReturn<'a>
where
    T: AsRef<[u8]>,
    E: AsRef<[u8]>,
{
    #[inline]
    fn from(result: Result<&'a T, &'a E>) -> Self {
        match result {
            Ok(value) => ValueReturn::Ok(Buffer::new(value)),
            Err(value) => ValueReturn::Err(Buffer::new(value)),
        }
    }
}
