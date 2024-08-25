use serde::{Deserialize, Serialize};

#[must_use]
pub const fn bool_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(remote = "Result")]
#[allow(clippy::exhaustive_enums)]
pub enum ResultAlt<T, E> {
    #[serde(rename = "result")]
    Ok(T),
    #[serde(rename = "error")]
    Err(E),
}

impl<T, E> From<ResultAlt<T, E>> for Result<T, E> {
    fn from(result: ResultAlt<T, E>) -> Self {
        match result {
            ResultAlt::Ok(value) => Ok(value),
            ResultAlt::Err(err) => Err(err),
        }
    }
}
