//! The API's routes for the sign-up policy and sign-up invites.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// What a client reads before sign-up (`GET /sign-up`, no session): the
/// cluster's policy and which invites it takes, so a frontend shows "invite
/// only" instead of failing at the end of a passkey ceremony.
async fn sign_up_public(State(node): State<Arc<Node>>) -> ApiResult {
    node.sys_fresh().await;
    let mut out = node.sys(|s| sign_up_json(s.governance().sign_up));
    // What this cluster's passkeys are named after (`passkeys::passkey_user_name`),
    // so a sign-in can say which of the person's passkeys to pick.
    out["cluster"] = json!(node.config.cluster);
    out["passkeyTag"] = json!(passkeys::cluster_tag(&node));
    Ok(Json(out))
}

fn sign_up_json(policy: SignUp) -> Value {
    json!({
        "policy": policy,
        "orgInvites": policy != SignUp::Closed,
        "signUpInvites": true,
        "inviteRequired": policy != SignUp::Open,
    })
}

/// The longest a cluster sign-up invite lives (a year).
pub const MAX_SIGN_UP_INVITE_TTL: u64 = 365 * 24 * 3600;

/// How long a cluster sign-up invite the owner mints lives unless it says.
const SIGN_UP_INVITE_TTL: u64 = 7 * 24 * 3600;

/// The bounds on an operator-chosen sign-up invite token, in bytes.
const SIGN_UP_TOKEN_BYTES: std::ops::RangeInclusive<usize> = 16..=256;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SignUpBody {
    policy: SignUp,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// The operator's sign-up invite: it chooses the token (the node never
/// makes one for it, so the `428` and the signed retry name the same
/// invite) and its expiry.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OpsSignUpInviteBody {
    /// The secret the invitee signs up with (16 to 256 bytes); the log keeps
    /// only its public key ([`crate::proof::invite_key`]).
    token: String,
    /// How many accounts it creates [default: 1].
    #[serde(default)]
    uses: Option<u32>,
    /// RFC 3339; at most [`MAX_SIGN_UP_INVITE_TTL`] ahead.
    expires_at: String,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
}

/// The owner's sign-up invite: the node mints the token (kept with the
/// passkey challenge, as for an organization invite).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SignUpInviteBody {
    #[serde(default)]
    uses: Option<u32>,
    #[serde(default)]
    ttl_secs: Option<u64>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// `403` unless the operator runs the cluster's sign-up (no owner named).
async fn operator_runs_sign_up(node: &Node) -> Result<(), ApiError> {
    let _caught_up = node.sys_fresh().await;
    node.sys(|s| s.runs_sign_up(OPERATOR))
        .map_err(ApiError::forbidden)
}

/// The signed-in account, if it acts for the cluster's owner and the owner
/// runs sign-up; `403` otherwise.
async fn owner_runs_sign_up(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let p = holder(node, headers).await?;
    let account = p.account()?;
    let _caught_up = node.sys_fresh().await;
    node.sys(|s| s.runs_sign_up(&account))
        .map_err(ApiError::forbidden)?;
    Ok(p)
}

/// Set the sign-up policy, signed with the operator key (`cluster /
/// __sign_up / { policy }`), while the operator runs sign-up.
async fn ops_sign_up_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<SignUpBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_runs_sign_up(&node).await?;
    let proof = operator_proof(
        &node,
        "cluster",
        "__sign_up",
        &json!({ "policy": body.policy }),
        body.operator_proof,
    )?;
    let v = node
        .sys_write(&SystemCmd::SetSignUp {
            policy: body.policy,
            actor: OPERATOR.to_owned(),
            proof: Some(proof),
            consent: None,
        })
        .await?;
    Ok(Json(
        json!({ "governance": v["governance"], "signUp": sign_up_json(body.policy) }),
    ))
}

/// The cluster's owner sets the sign-up policy, with a passkey assertion
/// over `cluster / __sign_up / { policy }` (a `412` asks for it).
async fn sign_up_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<SignUpBody>,
) -> ApiResult {
    let p = owner_runs_sign_up(&node, &headers).await?;
    let consent = consent(
        &node,
        &p,
        "cluster",
        "__sign_up",
        &json!({ "policy": body.policy }),
        body.confirm,
    )
    .await?;
    let v = node
        .sys_write(&SystemCmd::SetSignUp {
            policy: body.policy,
            actor: p.account()?,
            proof: None,
            consent: Some(consent),
        })
        .await?;
    Ok(Json(
        json!({ "governance": v["governance"], "signUp": sign_up_json(body.policy) }),
    ))
}

/// A sign-up invite's uses, checked before anything is signed.
fn sign_up_uses(uses: Option<u32>) -> Result<u32, ApiError> {
    let uses = uses.unwrap_or(1);
    if uses == 0 || uses > crate::system::MAX_SIGN_UP_INVITE_USES {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "a sign-up invite creates 1 to 10000 accounts",
        ));
    }
    Ok(uses)
}

/// Issue a cluster sign-up invite, signed with the operator key (`cluster /
/// __create_sign_up_invite / { inviteKey, uses, expiresAt }`), while the
/// operator runs sign-up. Answers the invite's key; the operator hands the
/// token to the person.
async fn ops_sign_up_invite_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OpsSignUpInviteBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    if !SIGN_UP_TOKEN_BYTES.contains(&body.token.len()) {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "a sign-up invite token is 16 to 256 bytes",
        ));
    }
    let uses = sign_up_uses(body.uses)?;
    let now = now_secs();
    let expires_at = unix_secs(Some(&body.expires_at), "expiresAt")?.unwrap_or_default();
    if expires_at <= now || expires_at > now + MAX_SIGN_UP_INVITE_TTL {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "expiresAt is in the future, at most a year ahead",
        ));
    }
    operator_runs_sign_up(&node).await?;
    let invite_key = crate::proof::invite_key(&body.token);
    let invite = SignUpInvite {
        expires_at,
        uses_left: uses,
        created_by: OPERATOR.to_owned(),
        created_at: now,
    };
    let proof = operator_proof(
        &node,
        "cluster",
        "__create_sign_up_invite",
        &crate::system::sign_up_invite_args(&invite_key, &invite),
        body.operator_proof,
    )?;
    let _ok = node
        .sys_write(&SystemCmd::CreateSignUpInvite {
            invite_key: invite_key.clone(),
            invite: invite.clone(),
            actor: OPERATOR.to_owned(),
            proof: Some(proof),
            consent: None,
        })
        .await?;
    Ok(Json(sign_up_invite_json(&invite_key, &invite)))
}

/// The cluster's owner mints a sign-up invite, with a passkey assertion over
/// its key and terms (412 first). Answers the token, once.
async fn sign_up_invite_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<SignUpInviteBody>,
) -> ApiResult {
    let uses = sign_up_uses(body.uses)?;
    let ttl = body.ttl_secs.unwrap_or(SIGN_UP_INVITE_TTL);
    if ttl == 0 || ttl > MAX_SIGN_UP_INVITE_TTL {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "ttlSecs is 1 to a year",
        ));
    }
    let p = owner_runs_sign_up(&node, &headers).await?;
    let actor = p.account()?;
    let token = format!("sup_{}", b64url(&random::<24>()));
    let now = now_secs();
    let invite = SignUpInvite {
        expires_at: now + ttl,
        uses_left: uses,
        created_by: actor.clone(),
        created_at: now,
    };
    let args = crate::system::sign_up_invite_args(&crate::proof::invite_key(&token), &invite);
    let c = consent_minted(
        &node,
        &p,
        "cluster",
        "__create_sign_up_invite",
        &args,
        Some(json!({ "token": token, "createdAt": now })),
        body.confirm,
    )
    .await?;
    // The retry records the invite the approval names, minted by the first
    // request; its uses must be the ones this request asks for.
    let carry = c.carry.unwrap_or_default();
    let (Some(token), Some(created_at)) = (
        carry
            .get("token")
            .and_then(Value::as_str)
            .map(str::to_owned),
        carry.get("createdAt").and_then(Value::as_u64),
    ) else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a sign-up invite approval",
        ));
    };
    let invite_key = crate::proof::invite_key(&token);
    let invite = SignUpInvite {
        expires_at: c
            .args
            .get("expiresAt")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        created_at,
        ..invite
    };
    if crate::system::sign_up_invite_args(&invite_key, &invite) != c.args {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "the approval was for a different call",
        ));
    }
    let _ok = node
        .sys_write(&SystemCmd::CreateSignUpInvite {
            invite_key: invite_key.clone(),
            invite: invite.clone(),
            actor,
            proof: None,
            consent: Some(c.consent),
        })
        .await?;
    let mut out = sign_up_invite_json(&invite_key, &invite);
    out["token"] = json!(token);
    Ok(Json(out))
}

fn sign_up_invite_json(invite_key: &str, i: &SignUpInvite) -> Value {
    json!({
        "inviteKey": invite_key,
        "uses": i.uses_left,
        "expiresAt": crate::system::rfc3339(i.expires_at),
        "expiresAtSecs": i.expires_at,
        "createdBy": i.created_by,
        "createdAt": crate::system::rfc3339(i.created_at),
    })
}

/// Withdraw a sign-up invite, signed with the operator key (`cluster /
/// __revoke_sign_up_invite / { inviteKey }`).
async fn ops_sign_up_invite_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    operator(&node, &headers)?;
    let body = ops_proof_body(&body)?;
    operator_runs_sign_up(&node).await?;
    let proof = operator_proof(
        &node,
        "cluster",
        "__revoke_sign_up_invite",
        &json!({ "inviteKey": key }),
        body.operator_proof,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::RevokeSignUpInvite {
            invite_key: key,
            actor: OPERATOR.to_owned(),
            proof: Some(proof),
            consent: None,
        })
        .await?,
    ))
}

/// The cluster's owner withdraws a sign-up invite, with a passkey assertion.
async fn sign_up_invite_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = owner_runs_sign_up(&node, &headers).await?;
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = consent(
        &node,
        &p,
        "cluster",
        "__revoke_sign_up_invite",
        &json!({ "inviteKey": key }),
        confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::RevokeSignUpInvite {
            invite_key: key,
            actor: p.account()?,
            proof: None,
            consent: Some(consent),
        })
        .await?,
    ))
}

/// The policy and the live sign-up invites (keys and terms, never a token):
/// for the operator (its token) and whoever acts for the cluster's owner.
async fn sign_up_invite_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    if operator(&node, &headers).is_err() {
        let account = signed_in(&node, &headers).await?.account()?;
        let _caught_up = node.sys_fresh().await;
        let owner = node.sys(|s| {
            s.cluster_owner
                .as_ref()
                .is_some_and(|o| s.acts_for(o, &account))
        });
        if !owner {
            return Err(ApiError::forbidden(
                "only the cluster's operator and owner read its sign-up invites",
            ));
        }
    } else {
        let _caught_up = node.sys_fresh().await;
    }
    Ok(Json(node.sys(|s| {
        let invites: Vec<Value> = s
            .sign_up_invites
            .iter()
            .map(|(k, i)| sign_up_invite_json(k, i))
            .collect();
        json!({
            "signUp": sign_up_json(s.governance().sign_up),
            "runBy": if s.deployers_owner().is_some() { "owner" } else { "operator" },
            "invites": invites,
        })
    })))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/sign-up", get(sign_up_public).post(sign_up_set))
        .route("/sign-up/invites", post(sign_up_invite_create).get(sign_up_invite_list))
        .route("/sign-up/invites/{key}", delete(sign_up_invite_revoke))
        .route("/ops/sign-up", post(ops_sign_up_set))
        .route("/ops/sign-up/invites", post(ops_sign_up_invite_create).get(sign_up_invite_list))
        .route("/ops/sign-up/invites/{key}", delete(ops_sign_up_invite_revoke))
        // For an HTTP client whose DELETE carries no body (as for deployers).
        .route("/ops/sign-up/invites/{key}/revoke", post(ops_sign_up_invite_revoke))
}
