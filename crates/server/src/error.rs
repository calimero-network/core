//! Error types

use std::{error, fmt};

#[derive(Debug, Default)]
pub struct ErrorUnauthorized {
    reason: String,
}

impl ErrorUnauthorized {
    pub fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

impl fmt::Display for ErrorUnauthorized {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("request unauthenticated")
    }
}

impl error::Error for ErrorUnauthorized {}
