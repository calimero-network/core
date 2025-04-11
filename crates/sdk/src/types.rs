use core::error::Error as CoreError;
use std::fmt::{Debug, Display};

use serde::{Serialize, Serializer};

#[derive(Debug, Serialize)]
pub struct Error(#[serde(serialize_with = "error_string")] Box<dyn CoreError>);

fn error_string<S>(error: &impl AsRef<dyn CoreError>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&error.as_ref().to_string())
}

impl Error {
    #[must_use]
    pub fn msg<M>(msg: M) -> Self
    where
        M: Display,
    {
        Self(msg.to_string().into())
    }
}

impl<T> From<T> for Error
where
    T: CoreError + 'static,
{
    fn from(error: T) -> Self {
        Self(Box::new(error))
    }
}
