use std::fs::File;
use std::io::{BufReader, BufWriter};

use calimero_primitives::identity::PublicKey;
use eyre::{eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::cli::Environment;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub(crate) public_key: PublicKey,
}

impl Identity {
    pub fn new(public_key: PublicKey) -> Self {
        Self { public_key }
    }
}

pub fn create_identity(
    identity: Identity,
    environment: &Environment,
    identity_name: String,
) -> EyreResult<()> {
    let path = &environment
        .args
        .home
        .join(&environment.args.node_name)
        .join(format!("{}.identity", identity_name));

    let file_writer = BufWriter::new(File::create(path)?);

    serde_json::to_writer(file_writer, &identity)?;

    Ok(())
}

pub fn open_identity(environment: &Environment, identity_name: &str) -> EyreResult<Identity> {
    let path = &environment
        .args
        .home
        .join(&environment.args.node_name)
        .join(format!("{}.identity", identity_name));

    let file_reader = BufReader::new(
        File::open(path).map_err(|_| eyre!("Identity file with this name does not exist"))?,
    );

    Ok(serde_json::from_reader::<BufReader<File>, Identity>(
        file_reader,
    )?)
}
