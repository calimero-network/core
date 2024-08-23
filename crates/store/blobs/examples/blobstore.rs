use calimero_blobstore::{BlobManager, FileSystem};
use futures_util::TryStreamExt;
use tokio::io::{self, AsyncWriteExt};
use tokio_util::compat::TokioAsyncReadCompatExt;

const DATA_DIR: &'static str = "blob-tests/data";
const BLOB_DIR: &'static str = "blob-tests/blob";

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let config = calimero_store::config::StoreConfig {
        path: DATA_DIR.into(),
    };

    let data_store = calimero_store::Store::open::<calimero_store::db::RocksDB>(&config)?;

    let blob_store = FileSystem::new(BLOB_DIR.into()).await?;

    let blob_mgr = BlobManager::new(data_store, blob_store);

    let mut args = std::env::args().skip(1);

    match args.next() {
        Some(hash) => match blob_mgr.get(hash.parse()?).await? {
            Some(mut blob) => {
                let mut stdout = io::stdout();

                while let Some(chunk) = blob.try_next().await? {
                    stdout.write_all(&chunk).await?;
                }
            }
            None => {
                eprintln!("Blob does not exist");
                std::process::exit(1);
            }
        },
        None => {
            let stdin = io::stdin().compat();

            println!("{}", blob_mgr.put_sized(None, stdin).await?);
        }
    }

    Ok(())
}
