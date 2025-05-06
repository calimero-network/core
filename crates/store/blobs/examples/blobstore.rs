#![allow(unused_crate_dependencies)]

use std::env::args;
use std::process::exit;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use eyre::Result as EyreResult;
use futures_util::TryStreamExt;
use tokio::io::{stdin, stdout, AsyncWriteExt};
use tokio_util::compat::TokioAsyncReadCompatExt;

const DATA_DIR: &'static str = "blob-tests/data";
const BLOB_DIR: &'static str = "blob-tests/blob";

#[tokio::main]
async fn main() -> EyreResult<()> {
    let config = StoreConfig::new(DATA_DIR.into());

    let data_store = Store::open::<RocksDB>(&config)?;

    let blob_store = FileSystem::new(&BlobStoreConfig::new(BLOB_DIR.into())).await?;

    let blob_mgr = BlobManager::new(data_store, blob_store);

    let mut args = args().skip(1);

    match args.next() {
        Some(hash) => match blob_mgr.get(hash.parse()?)? {
            Some(mut blob) => {
                let mut stdout = stdout();

                while let Some(chunk) = blob.try_next().await? {
                    stdout.write_all(&chunk).await?;
                }
            }
            None => {
                eprintln!("Blob does not exist");
                exit(1);
            }
        },
        None => {
            let stdin = stdin().compat();

            let (blob_id, hash, size) = blob_mgr.put_sized(None, stdin).await?;

            println!("Blob ID: {}", blob_id);
            println!("Hash: {}", hash);
            println!("Size: {}", size);
        }
    }

    Ok(())
}
