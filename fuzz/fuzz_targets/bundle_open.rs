#![no_main]
//! The `.mpk` read path. The input is a raw tar stream; the target signs a manifest
//! naming its first entry as the wasm, so mutations reach extraction, not just the signature.

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::Arc;

use calimero_node_primitives::bundle::sign_manifest_json;
use calimero_node_primitives::client::application::bundle::{
    extract_bundle_manifest, is_bundle_blob, VerifiedBundle,
};
use calimero_node_primitives::fuzz_api::extract_bundle_files;
use flate2::write::GzEncoder;
use flate2::Compression;
use libfuzzer_sys::fuzz_target;
use mero_sign::{dev_signer_id, dev_signing_key};
use sha2::{Digest, Sha256};
use tar::Header;

const BLOCK: usize = 512;

fuzz_target!(|data: &[u8]| {
    let mut entries = data.to_vec();
    fix_checksums(&mut entries);

    // Unsigned, as a peer serves it: a manifest the read accepts marks a bundle.
    let unsigned = gzip(&entries);
    if extract_bundle_manifest(&unsigned).is_ok() {
        assert!(
            is_bundle_blob(&unsigned),
            "manifest read but blob not recognised"
        );
    }

    let Some((path, wasm)) = first_entry(&entries) else {
        return;
    };
    let hash = hex::encode(Sha256::digest(wasm));
    let mut archive = manifest_entry(&path, &hash, wasm.len());
    archive.extend_from_slice(&entries);
    let compressed: Arc<[u8]> = gzip(&archive).into();
    let Ok(bundle) = VerifiedBundle::open(Arc::clone(&compressed), true) else {
        return;
    };
    assert_eq!(bundle.signer_id(), dev_signer_id());
    let declared = bundle
        .manifest()
        .wasm
        .as_ref()
        .expect("signed manifest names a wasm");
    assert_eq!(
        (declared.path.as_str(), declared.hash.as_str()),
        (path.as_str(), hash.as_str())
    );

    // An artifact is a regular file's bytes, so it must sit verbatim in the archive.
    let wanted = BTreeSet::from([path.as_str()]);
    if let Ok(found) = extract_bundle_files(&compressed, &wanted) {
        for bytes in found.values() {
            assert!(
                contains(&archive, bytes),
                "{}-byte artifact is not in the {}-byte archive",
                bytes.len(),
                archive.len()
            );
        }
    }
    if let Ok(all) = bundle.all_wasm() {
        for artifact in all {
            assert_eq!(hex::encode(Sha256::digest(&artifact.bytes)), hash);
        }
    }
});

/// Recompute every header checksum, so mutated headers reach the entry logic.
fn fix_checksums(tar: &mut [u8]) {
    let mut at = 0_usize;
    while let Some(block) = at.checked_add(BLOCK).and_then(|end| tar.get_mut(at..end)) {
        // An all-zero block marks the end of the archive, and a checksum would hide it.
        if block.iter().all(|&b| b == 0) {
            at += BLOCK;
            continue;
        }
        let mut header = Header::new_old();
        header.as_mut_bytes().copy_from_slice(block);
        header.set_cksum();
        block.copy_from_slice(header.as_bytes());
        let Some(next) = header
            .entry_size()
            .ok()
            .and_then(|size| usize::try_from(size.div_ceil(BLOCK as u64)).ok())
            .and_then(|blocks| blocks.checked_add(1)?.checked_mul(BLOCK)?.checked_add(at))
        else {
            return;
        };
        at = next;
    }
}

/// The first header's path and the data bytes its size field covers.
fn first_entry(tar: &[u8]) -> Option<(String, &[u8])> {
    let header = Header::from_byte_slice(tar.get(..BLOCK)?);
    let path = header.path().ok()?.to_str()?.to_owned();
    let size = usize::try_from(header.entry_size().ok()?).ok()?;
    Some((path, tar.get(BLOCK..BLOCK.checked_add(size)?)?))
}

fn manifest_entry(path: &str, hash: &str, size: usize) -> Vec<u8> {
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "network.calimero.fuzz",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "wasm": { "path": path, "hash": hash, "size": size },
    });
    sign_manifest_json(&mut manifest, &dev_signing_key()).expect("manifest signs");
    let bytes = serde_json::to_vec(&manifest).expect("manifest serialises");

    let mut header = Header::new_gnu();
    header.set_path("manifest.json").expect("fixed path");
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    let mut entry = header.as_bytes().to_vec();
    entry.extend_from_slice(&bytes);
    entry.resize(entry.len().next_multiple_of(BLOCK), 0);
    entry
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes).expect("in-memory write");
    encoder.finish().expect("in-memory write")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}
