use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Error(#[serde(serialize_with = "error_string")] Box<dyn std::error::Error>);

fn error_string<S>(error: &Box<dyn std::error::Error>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&error.to_string())
}

impl Error {
    pub fn msg(s: &str) -> Self {
        Self(s.to_owned().into())
    }
}

impl<T> From<T> for Error
where
    T: std::error::Error + 'static,
{
    fn from(error: T) -> Self {
        Error(Box::new(error))
    }
}
