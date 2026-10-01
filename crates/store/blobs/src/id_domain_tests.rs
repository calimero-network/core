use std::path::Path;
use std::sync::Arc;

use calimero_store::db::InMemoryDB;
use futures_util::TryStreamExt;
use tempfile::tempdir;

use super::*;

async fn manager(root: &Path) -> BlobManager {
    let data_store = DataStore::new(Arc::new(InMemoryDB::owned()));
    let config = BlobStoreConfig::new(Utf8PathBuf::from_path_buf(root.to_path_buf()).unwrap());
    let blob_store = FileSystem::new(&config).await.unwrap();
    BlobManager::new(data_store, blob_store)
}

async fn read_all(mgr: &BlobManager, id: BlobId) -> Vec<u8> {
    let blob = mgr.get(id).unwrap().expect("blob present");
    let chunks: Vec<Box<[u8]>> = blob.try_collect().await.unwrap();
    chunks.concat()
}

/// Store `payload`, and return its root id together with the one chunk whose
/// digest equals that root id: a root id is the digest of its chunk ids, so a
/// chunk holding exactly the chunk id of a single-chunk blob lands on it.
async fn root_and_chunk_hashing_to_it(mgr: &BlobManager, payload: &[u8]) -> (BlobId, Vec<u8>) {
    let (root, _, _) = mgr.put(payload).await.unwrap();
    let root_meta = mgr
        .data_store
        .handle()
        .get(&BlobMetaKey::new(root))
        .unwrap()
        .unwrap();
    assert_eq!(root_meta.links.len(), 1, "one chunk, one link");
    let chunk_id = root_meta.links[0].blob_id();
    (root, chunk_id.as_ref().to_vec())
}

#[tokio::test]
async fn a_chunk_hashing_to_a_root_id_does_not_replace_that_blob() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let payload = b"a blob that is already known".to_vec();
    let (root, colliding) = root_and_chunk_hashing_to_it(&mgr, &payload).await;

    let (other, _, _) = mgr.put(&colliding[..]).await.unwrap();
    assert_ne!(other, root, "different content is a different blob");

    assert_eq!(read_all(&mgr, root).await, payload);
    assert_eq!(read_all(&mgr, other).await, colliding);
}

#[tokio::test]
async fn deleting_the_blob_that_shares_a_digest_keeps_the_other_blob() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let payload = b"a blob that is already known".to_vec();
    let (root, colliding) = root_and_chunk_hashing_to_it(&mgr, &payload).await;
    let (other, _, _) = mgr.put(&colliding[..]).await.unwrap();

    assert!(mgr.delete(other).await.unwrap());

    assert!(mgr.has(root).unwrap());
    assert_eq!(read_all(&mgr, root).await, payload);
}

#[tokio::test]
async fn a_root_whose_chunk_list_does_not_hash_to_its_id_is_refused() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let (first, _, _) = mgr.put(&b"first payload"[..]).await.unwrap();
    let (second, _, _) = mgr.put(&b"second payload"[..]).await.unwrap();

    // Point the first root at the second root's chunks.
    let mut handle = mgr.data_store.handle();
    let second_meta = handle.get(&BlobMetaKey::new(second)).unwrap().unwrap();
    let first_meta = handle.get(&BlobMetaKey::new(first)).unwrap().unwrap();
    handle
        .put(
            &BlobMetaKey::new(first),
            &BlobMetaValue::new(
                first_meta.size,
                first_meta.content_hash,
                second_meta.links,
                first_meta.refs,
            ),
        )
        .unwrap();

    let err = mgr.get(first).expect_err("a root that fails its id check");
    assert!(
        matches!(
            err.downcast_ref::<BlobError>(),
            Some(BlobError::CorruptGraph { .. })
        ),
        "expected CorruptGraph, got {err:?}"
    );
    assert_eq!(read_all(&mgr, second).await, b"second payload");
}

#[tokio::test]
async fn root_ids_are_the_digest_of_the_chunk_digests() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let payload = b"ids handed to clients stay stable";
    let (root, _, _) = mgr.put(&payload[..]).await.unwrap();

    let chunk_digest = Sha256::digest(payload);
    let expected = Sha256::digest(chunk_digest);
    assert_eq!(
        *AsRef::<[u8; 32]>::as_ref(&root),
        *AsRef::<[u8; 32]>::as_ref(&expected)
    );
}

/// Seed a blob the way a version without chunk ids in their own space stored
/// it: the chunk's row and file sit under the chunk id itself.
async fn seed_old_layout_blob(mgr: &BlobManager, payload: &[u8]) -> BlobId {
    let chunk_id = BlobId::from(*AsRef::<[u8; 32]>::as_ref(&Sha256::digest(payload)));
    let links = vec![BlobMetaKey::new(chunk_id)].into_boxed_slice();
    let root = root_id_of(&links);
    let hash = ContentHash::from(*chunk_id);

    mgr.blob_store.put(chunk_id, payload).await.unwrap();
    let mut handle = mgr.data_store.handle();
    handle
        .put(
            &BlobMetaKey::new(chunk_id),
            &BlobMetaValue::new(payload.len() as u64, hash, Box::default(), 1),
        )
        .unwrap();
    handle
        .put(
            &BlobMetaKey::new(root),
            &BlobMetaValue::new(payload.len() as u64, hash, links, 1),
        )
        .unwrap();
    root
}

#[tokio::test]
async fn a_blob_stored_under_the_old_chunk_keys_is_absent() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let root = seed_old_layout_blob(&mgr, b"stored by an earlier version").await;

    assert!(!mgr.has(root).unwrap());
}

#[tokio::test]
async fn a_blob_stored_under_the_old_chunk_keys_is_not_found() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let root = seed_old_layout_blob(&mgr, b"stored by an earlier version").await;

    assert!(mgr.get(root).unwrap().is_none());
}

#[tokio::test]
async fn adding_a_blob_stored_under_the_old_chunk_keys_restarts_its_count() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let payload = b"stored by an earlier version";
    let old_root = seed_old_layout_blob(&mgr, payload).await;

    let (root, _, _) = mgr.put(&payload[..]).await.unwrap();
    assert_eq!(root, old_root, "the id is the same across layouts");
    assert!(mgr.has(root).unwrap());
    assert_eq!(read_all(&mgr, root).await, payload);

    assert!(mgr.delete(root).await.unwrap());
    assert!(!mgr.has(root).unwrap());
    assert!(
        mgr.data_store
            .handle()
            .get(&BlobMetaKey::new(root))
            .unwrap()
            .is_none(),
        "one delete frees the root row too"
    );
}

#[tokio::test]
async fn a_second_add_of_a_current_blob_still_counts_two_references() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let (root, _, _) = mgr.put(&b"payload"[..]).await.unwrap();
    let (again, _, _) = mgr.put(&b"payload"[..]).await.unwrap();
    assert_eq!(root, again);

    assert!(mgr.delete(root).await.unwrap());
    assert!(mgr.has(root).unwrap(), "the other reference keeps it");
    assert!(mgr.delete(root).await.unwrap());
    assert!(!mgr.has(root).unwrap());
}

#[tokio::test]
async fn a_stored_blob_is_present_until_it_is_deleted() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let (root, _, _) = mgr.put(&b"payload"[..]).await.unwrap();
    assert!(mgr.has(root).unwrap());

    assert!(mgr.delete(root).await.unwrap());
    assert!(!mgr.has(root).unwrap());
}

#[tokio::test]
async fn a_chunk_row_id_is_not_a_present_blob() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let (root, _, _) = mgr.put(&b"payload"[..]).await.unwrap();
    let row_id = chunk_key(only_chunk_of(&mgr, root)).blob_id();

    assert!(!mgr.has(row_id).unwrap());
}

fn only_chunk_of(mgr: &BlobManager, root: BlobId) -> BlobId {
    mgr.data_store
        .handle()
        .get(&BlobMetaKey::new(root))
        .unwrap()
        .unwrap()
        .links[0]
        .blob_id()
}

#[tokio::test]
async fn a_chunk_row_is_not_readable_as_a_blob() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let (root, _, _) = mgr.put(&b"payload"[..]).await.unwrap();
    let row_id = chunk_key(only_chunk_of(&mgr, root)).blob_id();

    let err = mgr.get(row_id).expect_err("a chunk row is not a root");
    assert!(
        matches!(
            err.downcast_ref::<BlobError>(),
            Some(BlobError::CorruptGraph { .. })
        ),
        "expected CorruptGraph, got {err:?}"
    );
}

#[tokio::test]
async fn a_chunk_row_is_not_released_as_a_blob() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let (root, _, _) = mgr.put(&b"payload"[..]).await.unwrap();
    let chunk_id = only_chunk_of(&mgr, root);

    assert!(
        mgr.delete(chunk_key(chunk_id).blob_id()).await.is_err(),
        "a chunk row is not a root"
    );

    let chunk_meta = mgr
        .data_store
        .handle()
        .get(&chunk_key(chunk_id))
        .unwrap()
        .unwrap();
    assert_eq!(chunk_meta.refs, 1);
    assert_eq!(read_all(&mgr, root).await, b"payload");
}

#[tokio::test]
async fn a_chunk_row_that_lists_further_chunks_is_refused() {
    let dir = tempdir().unwrap();
    let mgr = manager(dir.path()).await;

    let (root, _, _) = mgr.put(&b"payload"[..]).await.unwrap();
    let chunk_id = only_chunk_of(&mgr, root);
    let mut handle = mgr.data_store.handle();
    let chunk_meta = handle.get(&chunk_key(chunk_id)).unwrap().unwrap();
    handle
        .put(
            &chunk_key(chunk_id),
            &BlobMetaValue::new(
                chunk_meta.size,
                chunk_meta.content_hash,
                vec![BlobMetaKey::new(root)].into_boxed_slice(),
                chunk_meta.refs,
            ),
        )
        .unwrap();

    let blob = mgr.get(root).unwrap().expect("blob present");
    let err = blob
        .try_collect::<Vec<_>>()
        .await
        .expect_err("a chunk must not name further chunks");
    assert!(
        matches!(err, BlobError::CorruptGraph { .. }),
        "expected CorruptGraph, got {err:?}"
    );
}
