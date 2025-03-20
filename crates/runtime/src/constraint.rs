use core::marker::PhantomData;
use core::ops::Deref;

use thiserror::Error as ThisError;

#[derive(Debug, Clone)]
pub struct Constrained<T, R> {
    value: T,
    _phantom: PhantomData<R>,
}

impl<T, R> Deref for Constrained<T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

pub trait Constrains<T> {
    type Error;

    fn validate(value: T) -> Result<T, Self::Error>;
}

pub trait Constraint: Sized {
    fn validate<R: Constrains<Self>>(self) -> Result<Constrained<Self, R>, R::Error>;
}

impl<T> Constraint for T {
    fn validate<R: Constrains<T>>(self) -> Result<Constrained<T, R>, R::Error> {
        Ok(Constrained {
            value: R::validate(self)?,
            _phantom: PhantomData,
        })
    }
}

#[derive(Debug, Clone)]
pub struct MaxU64<const MAX: u64>;

#[derive(Debug, ThisError)]
#[error("value {0} is greater than the maximum {MAX}")]
pub struct MaxU64Error<const MAX: u64>(u64);

impl<const MAX: u64> Constrains<u64> for MaxU64<MAX> {
    type Error = MaxU64Error<MAX>;

    fn validate(value: u64) -> Result<u64, Self::Error> {
        if value < MAX {
            return Ok(value);
        }

        Err(MaxU64Error::<MAX>(value))
    }
}
