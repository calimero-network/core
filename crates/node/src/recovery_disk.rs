//! The recovery disk (`--recovery-dir`): backups that open with the key of
//! whoever upgrades the app ([`crate::system::SystemState::backup_holder`]),
//! never with K_c, so they outlive every mero-kms.
//!
//! A file is `MAGIC`, the header's length (u32, big-endian), the header
//! (JSON, [`Header`]), the node's 64-byte Ed25519 signature over
//! [`SIGNING_LABEL`] and the header bytes, then the body. The body is the
//! plaintext sealed as a file is ([`crate::files::FileSealer`]: 64 KiB
//! AES-256-GCM chunks) under a fresh data key; that key is sealed to the
//! holder's X25519 public key with HPKE (RFC 9180, base mode,
//! DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM; [`HPKE_INFO`],
//! [`HPKE_AAD`]) into `enc` and `wrappedKey`. The header names the
//! ciphertext's SHA-256 and the plaintext's size and SHA-256, so the
//! signature covers every byte.
//!
//! The signing key is the node's receipt key, and the header carries its
//! attestation (`purpose=receipts`, nonce [`attestation_nonce`]): whoever
//! opens a file checks, against Intel's root and the mero-tee release's
//! signed measurements, that attested hardware wrote it. The node holds
//! only public keys: nothing it keeps opens a file it wrote.
//!
//! Bodies by [`Kind`]: `app`, an [`AppMeta`] then the group's state as
//! [`calimero_log::PortableRow`]s ([`write_row`]); `holder`, the holder's
//! slice of the system state (JSON); `system`, the system group's whole
//! state (JSON); `code`, one bundle or release `.mpk`, named by its hash.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use calimero_log::PortableRow;
use ed25519_dalek::{Signer, SigningKey};
use eyre::Result as EyreResult;
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use calimero_client::recovery::signing_message;
pub use calimero_client::recovery::{
    attestation_nonce, new_keypair, open_key, public_of, seal_key, verify_header, BackupHolder,
    Header, Kind, FORMAT, HPKE_AAD, HPKE_INFO, MAGIC, MAX_HEADER_BYTES, SIGNING_LABEL, SUFFIX,
};

use crate::files::{FileOpener, FileSealer};

/// The largest single key or value in an `app` body.
pub const MAX_ROW_BYTES: usize = 64 * 1024 * 1024;
/// The largest `holder`, `system` or `code` body opened whole, and an
/// [`AppMeta`].
pub const MAX_WHOLE_BODY_BYTES: u64 = 512 * 1024 * 1024;
/// How many recovery files of one app (or org slice, or system) a node keeps.
pub const KEEP: usize = 3;

/// What a header names before the node seals and signs it.
#[derive(Clone, Debug)]
pub struct Subject {
    pub kind: Kind,
    pub holder: BackupHolder,
    pub cluster: String,
    pub app: Option<String>,
    pub org: Option<String>,
    pub hash: Option<String>,
    pub group: u64,
    pub index: u64,
    pub written_at: u64,
    pub merod: String,
    pub release: String,
    pub node: u64,
}

/// What an `app` body starts with: the app's directory record and the key
/// its sealed files' data keys are wrapped under, so its files open again
/// once they are on the disk too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppMeta {
    pub record: Value,
    /// `backup::file_key(K_c, app)` (hex).
    pub file_key: String,
}

/// The node's signer: its receipt key and that key's attestation.
pub struct NodeSigner<'a> {
    pub key: &'a SigningKey,
    pub attestation: &'a Value,
}

/// A file's name: what it holds, then the index and time, so names sort
/// oldest first within one subject.
#[must_use]
pub fn file_name(subject: &Subject) -> String {
    let what = match (subject.kind, &subject.app, &subject.org, &subject.hash) {
        (Kind::App, Some(app), _, _) => format!("app-{app}"),
        (Kind::Code, _, _, Some(hash)) => return format!("code-{hash}{SUFFIX}"),
        (kind, ..) => kind.name().to_owned(),
    };
    format!(
        "{what}.{:020}.{:020}{SUFFIX}",
        subject.index, subject.written_at
    )
}

/// The directory a holder's files go in: [`BackupHolder::key`] with `:`
/// made `-`, which ids never hold.
#[must_use]
pub fn holder_dir(dir: &Path, holder: &BackupHolder) -> PathBuf {
    dir.join(holder.key().replace(':', "-"))
}

fn hex_sha256(ctx: Context) -> String {
    hex::encode(ctx.finish())
}

/// Write a recovery file for `subject` in `dir`, its body what `body`
/// writes, sealed to `recipient` and signed by `signer`. Answers its path,
/// or `None` when that file is there already.
pub fn write(
    dir: &Path,
    subject: Subject,
    recipient: &[u8; 32],
    signer: &NodeSigner<'_>,
    body: impl FnOnce(&mut dyn FnMut(&[u8]) -> EyreResult<()>) -> EyreResult<()>,
) -> EyreResult<Option<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let name = file_name(&subject);
    let out = dir.join(&name);
    if out.exists() {
        return Ok(None);
    }
    let body_tmp = dir.join(format!(".{name}.body"));
    let file_tmp = dir.join(format!(".{name}.tmp"));
    let written = write_parts(&body_tmp, &file_tmp, subject, recipient, signer, body)
        .and_then(|()| Ok(std::fs::rename(&file_tmp, &out)?));
    let _gone = std::fs::remove_file(&body_tmp);
    if let Err(e) = written {
        let _gone = std::fs::remove_file(&file_tmp);
        return Err(e);
    }
    Ok(Some(out))
}

fn write_parts(
    body_tmp: &Path,
    file_tmp: &Path,
    subject: Subject,
    recipient: &[u8; 32],
    signer: &NodeSigner<'_>,
    body: impl FnOnce(&mut dyn FnMut(&[u8]) -> EyreResult<()>) -> EyreResult<()>,
) -> EyreResult<()> {
    // The body first, to a file of its own: the header names its hash.
    let mut sealer = FileSealer::new();
    let data_key = Zeroizing::new(*sealer.data_key());
    let mut cipher = BufWriter::new(File::create(body_tmp)?);
    let mut cipher_hash = Context::new(&SHA256);
    let mut cipher_bytes = 0_u64;
    {
        let mut emit = |chunk: &[u8]| -> EyreResult<()> {
            cipher.write_all(chunk)?;
            cipher_hash.update(chunk);
            cipher_bytes += chunk.len() as u64;
            Ok(())
        };
        let mut feed = |plain: &[u8]| -> EyreResult<()> {
            let sealed = sealer.write(plain)?;
            emit(&sealed)
        };
        body(&mut feed)?;
        let done = sealer.finish()?;
        emit(&done.tail)?;
        cipher.flush()?;
        drop(cipher);
        let (enc, wrapped) = seal_key(recipient, &data_key)?;
        let header = Header {
            format: FORMAT,
            kind: subject.kind,
            holder: subject.holder,
            cluster: subject.cluster,
            app: subject.app,
            org: subject.org,
            hash: subject.hash,
            group: subject.group,
            index: subject.index,
            written_at: subject.written_at,
            merod: subject.merod,
            release: subject.release,
            node: subject.node,
            recipient: hex::encode(recipient),
            enc: hex::encode(enc),
            wrapped_key: hex::encode(wrapped),
            size: done.size,
            sha256: hex::encode(done.sha256),
            body_bytes: cipher_bytes,
            body_sha256: hex_sha256(cipher_hash),
            signing_key: hex::encode(signer.key.verifying_key().to_bytes()),
            attestation: signer.attestation.clone(),
        };
        let header_bytes = serde_json::to_vec(&header)?;
        let signature = signer.key.sign(&signing_message(&header_bytes));
        let mut file = BufWriter::new(File::create(file_tmp)?);
        file.write_all(MAGIC)?;
        file.write_all(&u32::try_from(header_bytes.len())?.to_be_bytes())?;
        file.write_all(&header_bytes)?;
        file.write_all(&signature.to_bytes())?;
        let _copied = std::io::copy(&mut File::open(body_tmp)?, &mut file)?;
        let file = file.into_inner().map_err(|e| eyre::eyre!("{e}"))?;
        file.sync_all()?;
    }
    Ok(())
}

/// A recovery file read up to its body: the header, checked against the
/// signature the header's own key made (the attestation that key belongs
/// to attested hardware is the reader's to check, [`Header::attestation`]).
pub struct Opened {
    pub header: Header,
    /// The header as signed, and the node's signature over it, for a reader
    /// that checks it again.
    pub header_bytes: Vec<u8>,
    pub signature: [u8; 64],
    pub path: PathBuf,
    body_offset: u64,
}

/// Read and check a recovery file's header and signature.
pub fn read(path: &Path) -> EyreResult<Opened> {
    let mut f = BufReader::new(File::open(path)?);
    let mut magic = vec![0; MAGIC.len()];
    f.read_exact(&mut magic)?;
    if magic != MAGIC {
        eyre::bail!("{} is not a recovery file", path.display());
    }
    let mut len = [0; 4];
    f.read_exact(&mut len)?;
    let len = usize::try_from(u32::from_be_bytes(len))?;
    if len > MAX_HEADER_BYTES {
        eyre::bail!("a recovery file's header is at most {MAX_HEADER_BYTES} bytes");
    }
    let mut header_bytes = vec![0; len];
    f.read_exact(&mut header_bytes)?;
    let mut signature = [0; 64];
    f.read_exact(&mut signature)?;
    let header = verify_header(&header_bytes, &signature)
        .map_err(|e| eyre::eyre!("{}: {e}", path.display()))?;
    let body_offset = (MAGIC.len() + 4 + len + 64) as u64;
    let file_bytes = std::fs::metadata(path)?.len();
    if file_bytes != body_offset + header.body_bytes {
        eyre::bail!(
            "{}: the body is {} bytes, the header says {}",
            path.display(),
            file_bytes.saturating_sub(body_offset),
            header.body_bytes
        );
    }
    Ok(Opened {
        header,
        header_bytes,
        signature,
        path: path.to_owned(),
        body_offset,
    })
}

impl Opened {
    /// Stream the plaintext with the file's data key: `f` gets each piece,
    /// in order; the ciphertext's hash, then the plaintext's size and hash,
    /// are proved before the last piece.
    pub fn plaintext(
        &self,
        data_key: &[u8; 32],
        mut f: impl FnMut(&[u8]) -> EyreResult<()>,
    ) -> EyreResult<()> {
        // The ciphertext's hash first: nothing opens from a body the
        // signature does not cover.
        let mut r = BufReader::new(File::open(&self.path)?);
        std::io::copy(&mut (&mut r).take(self.body_offset), &mut std::io::sink())?;
        let mut hash = Context::new(&SHA256);
        let mut buf = vec![0; 64 * 1024];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if hex_sha256(hash) != self.header.body_sha256 {
            eyre::bail!("{}: the body is not the one signed", self.path.display());
        }
        let sha256: [u8; 32] = hex::decode(&self.header.sha256)?
            .try_into()
            .map_err(|_| eyre::eyre!("the plaintext hash is not 32 bytes"))?;
        let mut r = BufReader::new(File::open(&self.path)?);
        std::io::copy(&mut (&mut r).take(self.body_offset), &mut std::io::sink())?;
        let mut opener = FileOpener::new(data_key);
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let plain = Zeroizing::new(opener.push(&buf[..n])?);
            if !plain.is_empty() {
                f(&plain)?;
            }
        }
        let last = Zeroizing::new(opener.finish_expecting(self.header.size, &sha256)?);
        if !last.is_empty() {
            f(&last)?;
        }
        Ok(())
    }

    /// The whole plaintext of a `holder`, `system` or `code` file.
    pub fn whole(&self, data_key: &[u8; 32]) -> EyreResult<Zeroizing<Vec<u8>>> {
        if self.header.size > MAX_WHOLE_BODY_BYTES {
            eyre::bail!("a body opened whole is at most {MAX_WHOLE_BODY_BYTES} bytes");
        }
        let mut out = Zeroizing::new(Vec::with_capacity(usize::try_from(self.header.size)?));
        self.plaintext(data_key, |p| {
            out.extend_from_slice(p);
            Ok(())
        })?;
        Ok(out)
    }

    /// An `app` file: its [`AppMeta`], then each row to `f`.
    pub fn app(
        &self,
        data_key: &[u8; 32],
        mut f: impl FnMut(PortableRow) -> EyreResult<()>,
    ) -> EyreResult<AppMeta> {
        if self.header.kind != Kind::App {
            eyre::bail!("not an app's recovery file");
        }
        let mut parser = BodyParser::default();
        self.plaintext(data_key, |p| parser.push(p, &mut f))?;
        parser.finish()
    }
}

/// The start of an `app` body.
pub fn write_app_meta(
    out: &mut dyn FnMut(&[u8]) -> EyreResult<()>,
    meta: &AppMeta,
) -> EyreResult<()> {
    let bytes = Zeroizing::new(serde_json::to_vec(meta)?);
    out(&u32::try_from(bytes.len())?.to_be_bytes())?;
    out(&bytes)
}

/// One row of an `app` body: `0` (plain) or `1` (hidden), the tag (0 for a
/// plain row), then the key and the value, each after its u32 length.
pub fn write_row(
    out: &mut dyn FnMut(&[u8]) -> EyreResult<()>,
    row: &PortableRow,
) -> EyreResult<()> {
    let (kind, tag, key, value) = match row {
        PortableRow::Plain { key, value } => (0_u8, 0_u8, key, value),
        PortableRow::Hidden { tag, key, value } => (1, *tag, key, value),
    };
    out(&[kind, tag])?;
    out(&u32::try_from(key.len())?.to_be_bytes())?;
    out(key)?;
    out(&u32::try_from(value.len())?.to_be_bytes())?;
    out(value)
}

/// Parses an `app` body as it streams in.
#[derive(Default)]
struct BodyParser {
    buf: Zeroizing<Vec<u8>>,
    meta: Option<AppMeta>,
}

impl BodyParser {
    fn push(
        &mut self,
        data: &[u8],
        f: &mut impl FnMut(PortableRow) -> EyreResult<()>,
    ) -> EyreResult<()> {
        self.buf.extend_from_slice(data);
        let mut at = 0;
        loop {
            let rest = &self.buf[at..];
            if self.meta.is_none() {
                let Some(len) = read_len(rest)? else { break };
                if (len as u64) > MAX_WHOLE_BODY_BYTES {
                    eyre::bail!("an app's recovery header is too large");
                }
                let Some(bytes) = rest.get(4..4 + len) else {
                    break;
                };
                self.meta = Some(serde_json::from_slice(bytes)?);
                at += 4 + len;
                continue;
            }
            let Some((row, used)) = parse_row(rest)? else {
                break;
            };
            f(row)?;
            at += used;
        }
        let _consumed = self.buf.drain(..at);
        Ok(())
    }

    fn finish(self) -> EyreResult<AppMeta> {
        if !self.buf.is_empty() {
            eyre::bail!("an app's recovery body ends inside a row");
        }
        self.meta
            .ok_or_else(|| eyre::eyre!("an app's recovery body holds no header"))
    }
}

fn read_len(b: &[u8]) -> EyreResult<Option<usize>> {
    let Some(len) = b.get(..4) else {
        return Ok(None);
    };
    let len = usize::try_from(u32::from_be_bytes(len.try_into()?))?;
    if len > MAX_ROW_BYTES {
        eyre::bail!("a row of a recovery body is at most {MAX_ROW_BYTES} bytes");
    }
    Ok(Some(len))
}

/// One row from the front of `b`, and how many bytes it took; `None` until
/// it is all there.
fn parse_row(b: &[u8]) -> EyreResult<Option<(PortableRow, usize)>> {
    let (Some(&kind), Some(&tag)) = (b.first(), b.get(1)) else {
        return Ok(None);
    };
    let Some(klen) = read_len(&b[2..])? else {
        return Ok(None);
    };
    let k0 = 6;
    let Some(key) = b.get(k0..k0 + klen) else {
        return Ok(None);
    };
    let Some(vlen) = read_len(&b[k0 + klen..])? else {
        return Ok(None);
    };
    let v0 = k0 + klen + 4;
    let Some(value) = b.get(v0..v0 + vlen) else {
        return Ok(None);
    };
    let row = match (kind, tag) {
        (0, 0) => PortableRow::Plain {
            key: key.to_vec(),
            value: value.to_vec(),
        },
        (1, tag) => PortableRow::Hidden {
            tag,
            key: key.to_vec(),
            value: value.to_vec(),
        },
        _ => eyre::bail!("a recovery body holds a row of unknown kind {kind}"),
    };
    Ok(Some((row, v0 + vlen)))
}

/// `dir/name` for a file name a request gives: one plain name, never a path.
pub fn path_in(dir: &Path, name: &str) -> EyreResult<PathBuf> {
    let plain = !name.is_empty()
        && name.len() <= 512
        && !name.starts_with('.')
        && name.ends_with(SUFFIX)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if !plain {
        eyre::bail!("{name:?} is not the name of a recovery file");
    }
    Ok(dir.join(name))
}

/// The recovery files in `dir` of one subject (`prefix`, a [`file_name`]
/// up to its index), oldest first.
pub fn files_of(dir: &Path, prefix: &str) -> EyreResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(out);
    };
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.starts_with(prefix) && name.ends_with(SUFFIX) {
            out.push(dir.join(name));
        }
    }
    out.sort();
    Ok(out)
}

/// Keep the newest [`KEEP`] files of one subject; answers how many went.
pub fn prune(dir: &Path, prefix: &str) -> EyreResult<usize> {
    let files = files_of(dir, prefix)?;
    let excess = files.len().saturating_sub(KEEP);
    for old in &files[..excess] {
        std::fs::remove_file(old)?;
    }
    Ok(excess)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(kind: Kind) -> Subject {
        Subject {
            kind,
            holder: BackupHolder::Org("org".into()),
            cluster: "c".into(),
            app: (kind == Kind::App).then(|| "app".into()),
            org: (kind == Kind::Holder).then(|| "org".into()),
            hash: None,
            group: 7,
            index: 42,
            written_at: 1_700_000_000,
            merod: "1.0.0".into(),
            release: "2.0.0".into(),
            node: 1,
        }
    }

    fn signer_key() -> SigningKey {
        SigningKey::from_bytes(&[3; 32])
    }

    fn rows() -> Vec<PortableRow> {
        let mut rows: Vec<PortableRow> = (0..2000)
            .map(|i| PortableRow::Hidden {
                tag: b'k',
                key: format!("key-{i}").into_bytes(),
                value: vec![u8::try_from(i % 251).unwrap(); i % 300],
            })
            .collect();
        rows.push(PortableRow::Plain {
            key: b"\0head".to_vec(),
            value: b"{}".to_vec(),
        });
        rows
    }

    /// An app's rows written to the holder's key read back only with that
    /// key, after the node's signature, every byte as written.
    #[test]
    fn an_app_file_opens_only_with_its_holders_key() {
        let dir = tempfile::tempdir().unwrap();
        let (secret, public) = new_keypair();
        let key = signer_key();
        let attestation = serde_json::json!({ "mock": true });
        let signer = NodeSigner {
            key: &key,
            attestation: &attestation,
        };
        let meta = AppMeta {
            record: serde_json::json!({ "appId": "app" }),
            file_key: hex::encode([5; 32]),
        };
        let path = write(dir.path(), subject(Kind::App), &public, &signer, |out| {
            write_app_meta(out, &meta)?;
            rows().iter().try_for_each(|r| write_row(out, r))
        })
        .unwrap()
        .unwrap();
        // The same subject again is not written twice.
        assert!(
            write(dir.path(), subject(Kind::App), &public, &signer, |_| Ok(()))
                .unwrap()
                .is_none()
        );
        let opened = read(&path).unwrap();
        assert_eq!(opened.header.app.as_deref(), Some("app"));
        assert_eq!(opened.header.recipient, hex::encode(public));
        let data_key = open_key(&opened.header, &secret).unwrap();
        let mut back = Vec::new();
        let got = opened
            .app(&data_key, |r| {
                back.push(r);
                Ok(())
            })
            .unwrap();
        assert_eq!(got, meta);
        assert_eq!(back, rows());
        // Another holder's key opens nothing.
        let (other, _) = new_keypair();
        assert!(open_key(&opened.header, &other).is_err());
    }

    /// A changed header or body is refused before anything opens.
    #[test]
    fn a_tampered_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (secret, public) = new_keypair();
        let key = signer_key();
        let attestation = Value::Null;
        let signer = NodeSigner {
            key: &key,
            attestation: &attestation,
        };
        let path = write(dir.path(), subject(Kind::Holder), &public, &signer, |out| {
            out(b"{\"org\":\"org\"}")
        })
        .unwrap()
        .unwrap();
        let good = std::fs::read(&path).unwrap();
        let data_key = open_key(&read(&path).unwrap().header, &secret).unwrap();
        assert_eq!(
            read(&path).unwrap().whole(&data_key).unwrap().as_slice(),
            b"{\"org\":\"org\"}"
        );
        // A header byte: the signature fails.
        let mut bad = good.clone();
        let at = MAGIC.len() + 4 + 20;
        bad[at] ^= 1;
        std::fs::write(&path, &bad).unwrap();
        assert!(read(&path).is_err());
        // A body byte: the signed hash fails.
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        std::fs::write(&path, &bad).unwrap();
        assert!(read(&path).unwrap().whole(&data_key).is_err());
        // A file cut short.
        std::fs::write(&path, &good[..good.len() - 1]).unwrap();
        assert!(read(&path).is_err());
    }

    /// Only the newest files of a subject are kept.
    #[test]
    fn the_newest_files_of_a_subject_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (_secret, public) = new_keypair();
        let key = signer_key();
        let attestation = Value::Null;
        let signer = NodeSigner {
            key: &key,
            attestation: &attestation,
        };
        for index in 1..=5 {
            let mut s = subject(Kind::App);
            s.index = index;
            let _path = write(dir.path(), s, &public, &signer, |out| out(b"")).unwrap();
        }
        assert_eq!(prune(dir.path(), "app-app.").unwrap(), 2);
        let left = files_of(dir.path(), "app-app.").unwrap();
        assert_eq!(left.len(), KEEP);
        assert!(left[0].to_string_lossy().contains(&format!("{:020}", 3)));
    }

    #[test]
    fn a_holders_public_key_follows_from_its_secret() {
        let (secret, public) = new_keypair();
        assert_eq!(public_of(&secret).unwrap(), public);
    }
}
