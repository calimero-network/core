use std::fs::{self, File};
use std::io::prelude::*;
use std::{error::Error, fmt};

use serde::Serialize;

#[derive(Serialize)]
pub struct Credentials {
    account_id: String,
    private_key: String,
    public_key: String,
}

#[derive(Debug)]
struct StorageError(String);

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Storage Error: {}", self.0)
    }
}

impl Error for StorageError {}

pub fn save_keys_to_storage(
    account_id: &str,
    private_key: &str,
    public_key: &str,
) -> Result<(), Box<dyn Error>> {
    let credentials = Credentials {
        account_id: account_id.to_string(),
        private_key: private_key.to_string(),
        public_key: public_key.to_string(),
    };

    let json_data = serde_json::to_string(&credentials)?;

    let home_dir = dirs::home_dir().ok_or_else(|| StorageError("Failed to get home directory".into()))?;
    let credentials_dir = home_dir.join(".calimero/credentials");

    if !credentials_dir.exists() {
        fs::create_dir_all(&credentials_dir)?;
    }

    let account_id_file = credentials_dir.join(format!("{}.json", account_id));
    let mut file = File::create(account_id_file)?;

    file.write_all(json_data.as_bytes())?;

    Ok(())
}
