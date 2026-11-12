//! The API's routes for backup keys: the X25519 public keys the recovery disk
//! seals each app's backups to (`crate::recovery_disk`). Whoever upgrades an
//! app holds the key ([`crate::system::SystemState::backup_holder`]): the
//! operator, with its signature (`/ops/backup-key`), where it upgrades apps;
//! else an organization's org-wide `MANAGE_APPLICATION` holders
//! (`/orgs/{org}/backup-key`) and an account for its personal apps
//! (`/auth/backup-key`), each with a passkey. The private half never reaches
//! a node: a node only seals to it.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers read `api.rs`'s items, as every area does"
)]
use super::*;
use crate::system::cluster_version::Feature;
use crate::system::{backup_key_fits, BackupHolder, BAD_BACKUP_KEY, NOT_BACKUP_HOLDER, OPERATOR};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BackupKeyBody {
    /// The X25519 public key, 64 lowercase hex.
    public_key: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OpsBackupKeyBody {
    /// The X25519 public key, 64 lowercase hex.
    public_key: String,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
}

fn checked(public_key: &str) -> Result<(), ApiError> {
    if backup_key_fits(public_key) {
        Ok(())
    } else {
        Err(ApiError::bad_request("BAD_BACKUP_KEY", BAD_BACKUP_KEY))
    }
}

/// `{ holder, key: { publicKey, setAt, setBy } | null, apps }`: `apps` is how
/// many apps' backups open with it.
fn answer(node: &Node, holder: &BackupHolder) -> Value {
    node.sys(|s| {
        let key = s
            .backup_keys
            .get(&holder.key())
            .map(|k| json!({ "publicKey": k.public_key, "setAt": k.set_at, "setBy": k.set_by }));
        json!({
            "holder": holder,
            "key": key,
            "apps": s.apps_of_holder(holder).count(),
        })
    })
}

/// Write `holder`'s key, its proof already made.
async fn set(
    node: &Node,
    holder: BackupHolder,
    public_key: String,
    actor: String,
    proof: Value,
) -> ApiResult {
    let _ok = node
        .sys_write(&SystemCmd::SetBackupKey {
            holder: holder.clone(),
            public_key,
            actor,
            at: recovery_now(node),
            proof,
        })
        .await?;
    Ok(Json(answer(node, &holder)))
}

/// The operator's backup key, where it upgrades apps.
async fn ops_backup_key(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(answer(&node, &BackupHolder::Operator)))
}

/// Set the operator's backup key: signed over `operator / __set_backup_key /
/// { publicKey }`, where the operator upgrades apps.
async fn ops_set_backup_key(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<OpsBackupKeyBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    checked(&b.public_key)?;
    node.require_features(&[Feature::BackupKeys]).await?;
    let proof = operator_proof(
        &node,
        OPERATOR,
        "__set_backup_key",
        &json!({ "publicKey": b.public_key }),
        b.operator_proof,
    )?;
    set(
        &node,
        BackupHolder::Operator,
        b.public_key,
        OPERATOR.to_owned(),
        proof,
    )
    .await
}

/// An organization's backup key: any member reads it.
async fn org_backup_key(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh().await;
    if node
        .sys(|s| s.orgs.get(&org).and_then(|o| o.role(&actor)))
        .is_none()
    {
        return Err(ApiError::not_found("no such organization"));
    }
    Ok(Json(answer(&node, &BackupHolder::Org(org))))
}

/// Set an organization's backup key: those who upgrade all its apps (its
/// org-wide `MANAGE_APPLICATION` holders), with a passkey over `org:<org> /
/// __set_backup_key / { publicKey }` (`412` first).
async fn org_set_backup_key(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(b): Json<BackupKeyBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh().await;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    let holder = BackupHolder::Org(org.clone());
    if !node.sys(|s| s.may_set_backup_key(&holder, &actor)) {
        return Err(ApiError::forbidden(NOT_BACKUP_HOLDER));
    }
    checked(&b.public_key)?;
    node.require_features(&[Feature::BackupKeys]).await?;
    let args = json!({ "publicKey": b.public_key });
    let proof = org_consent(&node, &p, &org, "__set_backup_key", &args, b.confirm).await?;
    set(&node, holder, b.public_key, actor, proof).await
}

/// The signed-in account's backup key, for its personal apps.
async fn account_backup_key(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    Ok(Json(answer(&node, &BackupHolder::Account(actor))))
}

/// Set the account's backup key, with its passkey over `account:<a> /
/// __set_backup_key / { publicKey }` (`412` first).
async fn account_set_backup_key(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(b): Json<BackupKeyBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    checked(&b.public_key)?;
    node.require_features(&[Feature::BackupKeys]).await?;
    let args = json!({ "publicKey": b.public_key });
    let proof = consent(
        &node,
        &p,
        &format!("account:{actor}"),
        "__set_backup_key",
        &args,
        b.confirm,
    )
    .await?;
    set(
        &node,
        BackupHolder::Account(actor.clone()),
        b.public_key,
        actor,
        proof,
    )
    .await
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route(
            "/ops/backup-key",
            get(ops_backup_key).post(ops_set_backup_key),
        )
        .route(
            "/orgs/{org}/backup-key",
            get(org_backup_key).post(org_set_backup_key),
        )
        .route(
            "/auth/backup-key",
            get(account_backup_key).post(account_set_backup_key),
        )
}
