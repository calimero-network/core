use core::error::Error as CoreError;

use serde::{Serialize, Serializer};

#[derive(Debug, Serialize)]
pub struct Error(#[serde(serialize_with = "error_string")] Box<dyn CoreError>);

fn error_string<S>(error: &Box<dyn CoreError>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&error.to_string())
}

impl Error {
    #[must_use]
    pub fn msg(s: &str) -> Self {
        Self(s.to_owned().into())
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
