//! The API's routes for blobs, files and resumable uploads.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

fn blob_json(id: &[u8; 32], size: u64) -> Value {
    json!({
        "blobId": calimero_primitives::blobs::BlobId::from(*id).to_string(),
        "id": hex::encode(id),
        "size": size,
    })
}

/// A sealed file as the routes show it: its plaintext size and SHA-256, never its key.
fn file_json(id: &[u8; 32], size: u64, sha256: &[u8; 32]) -> Value {
    let mut out = blob_json(id, size);
    out["sha256"] = json!(hex::encode(sha256));
    out
}

/// Refuses `raw` when it is one of the component `app`'s sealed files: only its code reads
/// those, so no route serves, replaces or deletes one.
async fn refuse_sealed_file(
    node: &Node,
    app: &crate::types::AppRecord,
    raw: &[u8; 32],
) -> Result<(), ApiError> {
    if app.executor != crate::types::Executor::Component {
        return Ok(());
    }
    match node.file_held_strong(app, raw).await {
        Some(false) => Ok(()),
        Some(true) => Err(ApiError::new(
            409,
            "NOT_SUPPORTED",
            "a sealed file is read only by the app's code",
        )),
        None => Err(ApiError::unavailable("the app's leader did not answer")),
    }
}

#[derive(Deserialize)]
pub(super) struct BlobUploadQuery {
    /// Files have their own route; an old client's `?sealed=` is refused, not ignored.
    sealed: Option<bool>,
}

/// Upload a blob for an app: the body is its bytes. Anyone who may call the
/// app may add one; its runs then open it by id.
async fn blob_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<BlobUploadQuery>,
    body: axum::body::Bytes,
) -> ApiResult {
    if q.sealed.is_some() {
        return Err(ApiError::bad_request(
            "USE_FILES_API",
            &format!("upload files to /apps/{app_id}/files"),
        ));
    }
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "__blob", true).await?;
    if p.caller.account().is_none() {
        return Err(ApiError::unauthenticated("sign in to upload"));
    }
    if app.executor == crate::types::Executor::Component {
        refuse_full_app(&node, &app).await?;
    }
    // A component records the upload's content type.
    let mime = blob_mime(&headers)?;
    // The bytes of a sealed file would claim its id.
    refuse_sealed_file(&node, &app, &sha256(&body)).await?;
    let size = body.len() as u64;
    let pin = node
        .pin_on_leader(&app, vec![hex::encode(sha256(&body))])
        .await?;
    let stored = async {
        let id = node.store_blob(&body, Some(&app_id)).await?;
        let a = node
            .call(&CallRequest {
                // The gate's round covers the leader's checks (`Node::sys_fresh_from`).
                sys_index: p.sys_index,
                ..CallRequest::host(
                    app_id.clone(),
                    "__blob",
                    json!({ "id": id, "size": size, "mime": mime }),
                    p.caller,
                )
            })
            .await?;
        Ok::<_, ApiError>((id, a))
    }
    .await;
    node.unpin_on_leader(pin).await;
    let (id, a) = stored?;
    let raw = parse_blob_id(&id)?;
    let mut out = blob_json(&raw, size);
    out["index"] = json!(a.index);
    Ok(Json(out))
}

/// A component holds a bounded number of blobs and files (R8).
async fn refuse_full_app(node: &Node, app: &crate::types::AppRecord) -> Result<(), ApiError> {
    let stats = node.storage_stats(app).await?;
    if stats.blobs.len() + stats.files.len() >= crate::component::blobs::MAX_COMPONENT_BLOBS {
        return Err(ApiError::new(
            429,
            "QUOTA_EXCEEDED",
            "the app holds as many blobs as it may",
        ));
    }
    Ok(())
}

/// A blob's content type as the uploader gave it (`Content-Type`, default
/// `application/octet-stream`): printable ASCII, at most `MAX_BLOB_MIME_BYTES`.
fn blob_mime(headers: &HeaderMap) -> Result<String, ApiError> {
    let Some(v) = headers.get(axum::http::header::CONTENT_TYPE) else {
        return Ok("application/octet-stream".to_owned());
    };
    let mime = v.to_str().unwrap_or_default();
    if mime.is_empty()
        || mime.len() > crate::component::blobs::MAX_BLOB_MIME_BYTES
        || !mime.bytes().all(|b| b == b' ' || b.is_ascii_graphic())
    {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "unusable content type",
        ));
    }
    Ok(mime.to_owned())
}

/// `?grant=` on a download: a component's run handed it out (R8).
#[derive(Deserialize)]
pub(super) struct BlobGrant {
    grant: Option<String>,
}

/// Download one of an app's blobs: as one who reaches the app, or, for a component, with a
/// grant one of its runs handed out (no session needed).
async fn blob_download(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
    Query(q): Query<BlobGrant>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let raw = parse_blob_id(&id)?;
    let component = app.executor == crate::types::Executor::Component;
    // A bad or expired grant answers as a missing blob: it proves nothing.
    let granted = match q.grant.as_deref() {
        Some(grant) => Some(
            component
                .then(|| {
                    crate::component::blobs::grant_epoch(
                        &node.blob_grant_key(),
                        &app.app_id,
                        &raw,
                        grant,
                        node.app_clock().now_ms(),
                    )
                })
                .flatten()
                .ok_or_else(|| ApiError::not_found("no such blob in this app"))?,
        ),
        None => {
            let _p = gate(&node, &headers, &app, "__blob", false).await?;
            None
        }
    };
    refuse_sealed_file(&node, &app, &raw).await?;
    // The app's leader decides: a replica that has not applied a delete or a revoke yet
    // still serves the blob and its old grants.
    let (published, epoch) = match node.blob_standing_strong(&app, &raw).await {
        Some(Some(standing)) => standing,
        Some(None) => return Err(ApiError::not_found("no such blob in this app")),
        None => return Err(ApiError::unavailable("the app's leader did not answer")),
    };
    // A published blob is served only through a grant made since its last revoke.
    if granted.is_some_and(|made| made != epoch) || (published && granted.is_none()) {
        return Err(ApiError::not_found("no such blob in this app"));
    }
    let hex_id = hex::encode(raw);
    let bytes = match node
        .engine
        .get_blob(&hex_id)
        .map_err(|e| ApiError::internal(&e.to_string()))?
    {
        Some(b) => b,
        None => node.fetch_blob(&hex_id).await?,
    };
    let mime = if component {
        node.blob_mime(&app, &raw)
            .unwrap_or_else(|| "application/octet-stream".to_owned())
    } else {
        "application/octet-stream".to_owned()
    };
    Ok(([(axum::http::header::CONTENT_TYPE, mime)], bytes).into_response())
}

/// The app's blobs.
async fn blob_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "__blob", false).await?;
    let stats = node.storage_stats_strong(&app).await?;
    let blobs: Vec<Value> = (stats.blobs.iter())
        .map(|(id, size)| blob_json(id, *size))
        .collect();
    Ok(Json(json!({ "blobs": blobs })))
}

/// One blob's size; 404 if the app has no such blob.
async fn blob_info(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "__blob", false).await?;
    let raw = parse_blob_id(&id)?;
    let stats = node.storage_stats_strong(&app).await?;
    if stats.files.iter().any(|(f, ..)| *f == raw) {
        return Err(ApiError::new(
            409,
            "NOT_SUPPORTED",
            "a sealed file is read only by the app's code",
        ));
    }
    let size = stats
        .blobs
        .iter()
        .find(|(b, _)| *b == raw)
        .map(|(_, s)| *s)
        .ok_or_else(|| ApiError::not_found("no such blob in this app"))?;
    Ok(Json(blob_json(&raw, size)))
}

/// The app and signed-in account of a files route: a component app, and an account that may
/// handle its files (its owner or an organization member).
async fn files_gate(
    node: &Node,
    headers: &HeaderMap,
    app_id: &str,
    write: bool,
) -> Result<(crate::types::AppRecord, String), ApiError> {
    let app_id = node.canonical_app_id(app_id);
    let app = node.app(&app_id).await?;
    let p = gate(node, headers, &app, "__blob", write).await?;
    let Some(account) = p.caller.account().map(str::to_owned) else {
        return Err(ApiError::unauthenticated("sign in to use files"));
    };
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::new(
            409,
            "NOT_SUPPORTED",
            "only a component app holds files",
        ));
    }
    if !node.sys(|s| may_handle_files(s, &app, Some(&account))) {
        return Err(ApiError::forbidden(
            "only the app's owner and its organization's members use its files",
        ));
    }
    Ok((app, account))
}

/// A file a node committed, as the routes answer it.
fn uploaded_json(up: &crate::node::Uploaded) -> Result<Value, ApiError> {
    let sha256 = <[u8; 32]>::try_from(up.mark.sha256.0.as_slice())
        .map_err(|_| ApiError::internal("bad file mark"))?;
    let mut out = file_json(&parse_blob_id(&up.mark.id)?, up.mark.size, &sha256);
    out["index"] = json!(up.index);
    Ok(out)
}

/// Upload a file of up to one part (16 MiB): the body is its bytes, sealed under a key of its own.
async fn files_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, true).await?;
    refuse_full_app(&node, &app).await?;
    let up = node.upload_file(&app, body, &account).await?;
    Ok(Json(uploaded_json(&up)?))
}

/// The app's files: size and plaintext SHA-256, never a key.
async fn files_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let (app, _account) = files_gate(&node, &headers, &app_id, false).await?;
    let stats = node.storage_stats_strong(&app).await?;
    let files: Vec<Value> = (stats.files.iter())
        .map(|(id, size, h)| file_json(id, *size, h))
        .collect();
    Ok(Json(json!({ "files": files })))
}

/// One file's size and plaintext SHA-256; 404 if the app has no such file.
async fn file_info(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> ApiResult {
    let (app, _account) = files_gate(&node, &headers, &app_id, false).await?;
    let raw = parse_blob_id(&id)?;
    let stats = node.storage_stats_strong(&app).await?;
    let (_, size, sha256) = (stats.files.iter().find(|(f, ..)| *f == raw))
        .ok_or_else(|| ApiError::not_found("no such file in this app"))?;
    Ok(Json(file_json(&raw, *size, sha256)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BeginUpload {
    size: u64,
}

/// Begin an upload session for a file of `size` bytes.
async fn upload_begin(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<BeginUpload>,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, true).await?;
    refuse_full_app(&node, &app).await?;
    let op = crate::node::UploadOp::Begin { size: body.size };
    Ok(Json(node.upload_op(&app, &account, op).await?))
}

/// Where an upload session is: the next part it takes.
async fn upload_status(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, upload)): Path<(String, String)>,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, false).await?;
    let op = crate::node::UploadOp::Status { upload };
    Ok(Json(node.upload_op(&app, &account, op).await?))
}

/// Part `k` of an upload session: the raw bytes, exactly `partBytes` but for the last part.
async fn upload_part(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, upload, k)): Path<(String, String, String)>,
    body: axum::body::Bytes,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, true).await?;
    let k = k
        .parse()
        .map_err(|_| ApiError::bad_request("BAD_REQUEST", "a part number is a whole number"))?;
    let op = crate::node::UploadOp::Part {
        upload,
        k,
        bytes: body,
    };
    Ok(Json(node.upload_op(&app, &account, op).await?))
}

/// Charge the file and commit it: every part must be in.
async fn upload_complete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, upload)): Path<(String, String)>,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, true).await?;
    let op = crate::node::UploadOp::Complete { upload };
    let up = node.upload_op(&app, &account, op).await?;
    let up = serde_json::from_value(up).map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(uploaded_json(&up)?))
}

/// End an upload session and drop its bytes.
async fn upload_abort(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, upload)): Path<(String, String)>,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, true).await?;
    let op = crate::node::UploadOp::Abort { upload };
    Ok(Json(node.upload_op(&app, &account, op).await?))
}

/// Delete one of the app's blobs: its owner's or an org admin's. The app
/// forgets it at once; the bytes leave every node unless another app marks
/// the same content.
async fn blob_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "__blob", true).await?;
    if !manages_app(&node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner or an org admin deletes its blobs",
        ));
    }
    let raw = parse_blob_id(&id)?;
    refuse_sealed_file(&node, &app, &raw).await?;
    if !node.blob_held(&app, &raw).await? {
        return Err(ApiError::not_found("no such blob in this app"));
    }
    let a = node
        .call(&CallRequest {
            // The gate's round covers the leader's checks (`Node::sys_fresh_from`).
            sys_index: p.sys_index,
            ..CallRequest::host(
                app_id,
                "__blob_forget",
                json!({ "id": hex::encode(raw) }),
                p.caller,
            )
        })
        .await?;
    node.collect_blob(&raw).await;
    Ok(Json(json!({ "index": a.index })))
}

/// Revoke the grants made so far for one of the app's published blobs: its owner's or an org
/// admin's. A grant made after this one holds.
async fn blob_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, id)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "__blob", true).await?;
    if !manages_app(&node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner or an org admin revokes grants",
        ));
    }
    let raw = parse_blob_id(&id)?;
    let a = node
        .call(&CallRequest {
            sys_index: p.sys_index,
            ..CallRequest::host(
                app_id,
                "__blob_revoke",
                json!({ "id": hex::encode(raw) }),
                p.caller,
            )
        })
        .await?;
    if a.status != Status::Ok {
        return Err(ApiError::not_found("no such published blob in this app"));
    }
    Ok(Json(json!({ "index": a.index })))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/apps/{app}/blobs", post(blob_upload).get(blob_list))
        .route(
            "/apps/{app}/blobs/{id}",
            get(blob_download).delete(blob_delete),
        )
        .route("/apps/{app}/blobs/{id}/info", get(blob_info))
        .route("/apps/{app}/blobs/{id}/revoke", post(blob_revoke))
        .route("/apps/{app}/files", post(files_upload).get(files_list))
        .route("/apps/{app}/files/uploads", post(upload_begin))
        .route(
            "/apps/{app}/files/uploads/{upload}",
            get(upload_status).delete(upload_abort),
        )
        .route(
            "/apps/{app}/files/uploads/{upload}/parts/{k}",
            put(upload_part),
        )
        .route(
            "/apps/{app}/files/uploads/{upload}/complete",
            post(upload_complete),
        )
        .route("/apps/{app}/files/{id}", get(file_info))
}
