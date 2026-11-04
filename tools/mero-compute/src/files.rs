//! Plain files for a local run: inputs read from disk by the hash of their content, outputs
//! written to `output-<i>` in the order the run finishes them.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use calimero_app_host::{
    FileError, FileInfo, FileRead, FileWrite, Files, NewFile, ReadError, NO_SUCH_FILE,
};
use sha2::{Digest, Sha256};

const OUTPUT_PREFIX: &str = "output-"; // a finished output: output-{index}
const PART_SUFFIX: &str = ".part"; // a file still being written
const HASH_PIECE: usize = 1 << 20; // bytes hashed per read of an input

/// One input on disk.
#[derive(Clone)]
pub struct Input {
    pub id: String,
    pub size: u64,
    sha256: Vec<u8>,
    path: PathBuf,
}

impl Input {
    /// Hashes the file at `path`; its id is the hex SHA-256 of its content.
    pub fn read(path: &Path) -> eyre::Result<Self> {
        let mut file = File::open(path)
            .map_err(|e| eyre::eyre!("cannot read input {}: {e}", path.display()))?;
        let mut hash = Sha256::new();
        let (mut size, mut buf) = (0, vec![0; HASH_PIECE]);
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            size += n as u64;
        }
        let sha256 = hash.finalize().to_vec();
        Ok(Self {
            id: hex::encode(&sha256),
            size,
            sha256,
            path: path.to_owned(),
        })
    }
}

/// What a run finished writing: path, size and hash, in finishing order.
pub struct Produced {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: Vec<u8>,
}

/// The files of one run: its inputs, and the outputs it writes into `dir`.
pub struct LocalFiles {
    inputs: Vec<Input>,
    dir: PathBuf,
    produced: Arc<Mutex<Vec<Produced>>>,
    writers: AtomicU64,
}

impl LocalFiles {
    pub fn new(inputs: Vec<Input>, dir: PathBuf) -> Self {
        Self {
            inputs,
            dir,
            produced: Arc::default(),
            writers: AtomicU64::new(0),
        }
    }

    /// The outputs finished so far, shared with the writers.
    pub fn produced(&self) -> Arc<Mutex<Vec<Produced>>> {
        Arc::clone(&self.produced)
    }
}

impl Files for LocalFiles {
    fn open(&self, id: &str) -> Result<Box<dyn FileRead>, FileError> {
        let input = self
            .inputs
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| FileError::Failed(NO_SUCH_FILE.to_owned()))?;
        let file = File::open(&input.path).map_err(|_| FileError::Unavailable)?;
        Ok(Box::new(Reader(file)))
    }

    fn info(&self, id: &str) -> Option<FileInfo> {
        let input = self.inputs.iter().find(|i| i.id == id)?;
        Some(FileInfo {
            size: input.size,
            sha256: input.sha256.clone(),
        })
    }

    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        let n = self.writers.fetch_add(1, Ordering::Relaxed);
        let part = self.dir.join(format!("w-{n}{PART_SUFFIX}"));
        let file = File::create(&part).map_err(|_| FileError::Unavailable)?;
        Ok(Box::new(Writer {
            file: BufWriter::new(file),
            hash: Sha256::new(),
            size: 0,
            part,
            dir: self.dir.clone(),
            produced: Arc::clone(&self.produced),
        }))
    }
}

struct Reader(File);

impl FileRead for Reader {
    fn read(&mut self, max: usize) -> Result<Vec<u8>, ReadError> {
        let mut buf = vec![0; max];
        let n = self
            .0
            .read(&mut buf)
            .map_err(|e| ReadError::Failed(e.to_string()))?;
        buf.truncate(n);
        Ok(buf)
    }
}

struct Writer {
    file: BufWriter<File>,
    hash: Sha256,
    size: u64,
    part: PathBuf,
    dir: PathBuf,
    produced: Arc<Mutex<Vec<Produced>>>,
}

impl FileWrite for Writer {
    fn write(&mut self, bytes: &[u8]) -> Result<(), FileError> {
        self.hash.update(bytes);
        self.size += bytes.len() as u64;
        self.file
            .write_all(bytes)
            .map_err(|e| FileError::Failed(e.to_string()))
    }

    fn finish(mut self: Box<Self>) -> Result<NewFile, FileError> {
        let failed = |e: std::io::Error| FileError::Failed(e.to_string());
        self.file.flush().map_err(failed)?;
        let sha256 = self.hash.clone().finalize().to_vec();
        let mut produced = self.produced.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self.dir.join(format!("{OUTPUT_PREFIX}{}", produced.len()));
        std::fs::rename(&self.part, &path).map_err(failed)?;
        produced.push(Produced {
            path,
            size: self.size,
            sha256: sha256.clone(),
        });
        Ok(NewFile {
            id: hex::encode(&sha256),
            size: self.size,
            sha256,
            wrapped_key: Vec::new(),
            ciphertext: Vec::new(),
        })
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.part);
    }
}
