//! The API's routes for account and organization recovery.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// Create (or replace) the account's recovery code: approved with a passkey
/// over the code's public key (412 first), as an API key is over its hash.
/// The first request makes the code and keeps it with the challenge; the
/// retry logs only its public key and answers the code, once. The
/// account holder's, on the cluster's own origin: never a package-bound
/// session or an API key.
async fn recovery_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let code = calimero_identity::new_recovery_code();
    let key =
        recovery_key_of(&code).ok_or_else(|| ApiError::internal("a malformed recovery code"))?;
    let c = consent_minted(
        &node,
        &p,
        &format!("account:{account}"),
        "__set_recovery",
        &json!({ "recoveryKey": key }),
        Some(json!(code)),
        body.and_then(|Json(b)| b.confirm),
    )
    .await?;
    // The retry logs the code the approval names, made by the first request.
    let code = c
        .carry
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("WRONG_CHALLENGE", "not a recovery approval"))?;
    let recovery_key = recovery_key_of(&code)
        .filter(|k| c.args == json!({ "recoveryKey": k }))
        .ok_or_else(|| {
            ApiError::bad_request("WRONG_CHALLENGE", "the approval was for a different call")
        })?;
    let _ok = node
        .sys_write(&SystemCmd::SetAccountRecoveryKey {
            account,
            recovery_key,
            consent: c.consent,
        })
        .await?;
    Ok(Json(json!({ "code": code })))
}

/// Remove the account's recovery code, approved with a passkey (412 first).
async fn recovery_remove(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let consent = consent(
        &node,
        &p,
        &format!("account:{account}"),
        "__remove_recovery",
        &json!({}),
        body.and_then(|Json(b)| b.confirm),
    )
    .await?;
    let _ok = node
        .sys_write(&SystemCmd::RemoveAccountRecoveryKey { account, consent })
        .await?;
    Ok(Json(json!({})))
}

/// A recovery code's public key, from the code as typed; `None` for what
/// cannot be a code.
fn recovery_key_of(code: &str) -> Option<String> {
    calimero_identity::normalize_recovery_code(code).map(|c| crate::proof::recovery_key(&c))
}

fn wrong_recovery_code() -> ApiError {
    ApiError::unauthenticated(crate::system::UNKNOWN_RECOVERY_CODE)
}

fn recovery_pending(node: &Node, account: &str) -> Option<crate::types::PendingAccountRecovery> {
    node.sys(|s| s.account_recoveries.get(account).cloned())
}

fn account_recovery_pending() -> ApiError {
    ApiError::new(
        409,
        "RECOVERY_PENDING",
        crate::system::ACCOUNT_RECOVERY_PENDING,
    )
}

fn no_account_recovery() -> ApiError {
    ApiError::not_found(crate::system::NO_ACCOUNT_RECOVERY_PENDING)
}

fn recovery_not_ready(ready_at: u64) -> ApiError {
    ApiError::new(409, "RECOVERY_NOT_READY", crate::system::RECOVERY_NOT_READY)
        .with_details(json!({ "readyAt": ready_at }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoveryBegin {
    code: String,
}

/// Begin a recovery: the code names the account (no account id needed), and
/// the answer is a passkey registration for it. Signed out; a wrong or spent
/// code is `401`, and an account with a recovery under way `409`, both
/// before a challenge is made.
async fn account_recovery_begin(
    State(node): State<Arc<Node>>,
    Json(body): Json<RecoveryBegin>,
) -> ApiResult {
    let key = recovery_key_of(&body.code).ok_or_else(wrong_recovery_code)?;
    node.sys_fresh().await;
    let account = node
        .sys(|s| s.recovery_keys.get(&key).cloned())
        .ok_or_else(wrong_recovery_code)?;
    if !serves(&node, &account) {
        return Err(wrong_node());
    }
    if recovery_pending(&node, &account).is_some() {
        return Err(account_recovery_pending());
    }
    let challenge = random::<32>();
    let id = put_pending(
        &node,
        Pending::RecoverStart {
            challenge,
            account: account.clone(),
            recovery_key: key,
            expires: now_secs() + CHALLENGE_TTL,
        },
    )?;
    let (name, exclude): (String, Vec<Value>) = node.sys(|s| {
        s.accounts.get(&account).map_or_else(Default::default, |a| {
            (
                a.name.clone(),
                a.passkeys
                    .iter()
                    .map(|c| json!({ "type": "public-key", "id": c.id }))
                    .collect(),
            )
        })
    });
    let shown = passkeys::passkey_user_name(
        &name,
        &node.config.cluster,
        &passkeys::cluster_tag(&node),
        None,
    );
    Ok(Json(json!({
        "challengeId": id,
        "options": {
            "challenge": b64url(&challenge),
            "rp": { "id": node.config.rp_id, "name": "Calimero" },
            "user": { "id": b64url(&random::<16>()), "name": shown, "displayName": shown },
            "pubKeyCredParams": [ { "type": "public-key", "alg": -7 }, { "type": "public-key", "alg": -8 } ],
            "excludeCredentials": exclude,
            "timeout": 60000,
            "attestation": "none",
            "authenticatorSelection": { "residentKey": "required", "userVerification": "required" },
        }
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoveryStart {
    challenge_id: String,
    code: String,
    registration: Registration,
}

/// Start a recovery: the challenge is spent first, so a wrong code ends the
/// ceremony (begin again), as a failed passkey assertion does. The code must
/// be the one begin named (its public key, compared in constant time); the
/// new passkey must be registered on the cluster's own origins, never an
/// app's frontend, with the user verified. This node signs the start with
/// the code's key; the log spends the code and keeps the new passkey as
/// pending. No passkey is added and no session opens: the answer is the
/// pending recovery (`accountId`, `credentialId`, `startedAt`, `readyAt`).
async fn account_recovery_start(
    State(node): State<Arc<Node>>,
    Json(body): Json<RecoveryStart>,
) -> ApiResult {
    let Pending::RecoverStart {
        challenge,
        account,
        recovery_key,
        ..
    } = take_pending(&node, &body.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a recovery challenge",
        ));
    };
    let code = calimero_identity::normalize_recovery_code(&body.code)
        .filter(|c| {
            ct_eq(
                crate::proof::recovery_key(c).as_bytes(),
                recovery_key.as_bytes(),
            )
        })
        .ok_or_else(wrong_recovery_code)?;
    let cred = verify_registration(
        &RelyingParty {
            require_uv: true,
            ..rp(&node)
        },
        &challenge,
        &body.registration,
    )
    .map_err(|e| ApiError::bad_request("BAD_REGISTRATION", &e.to_string()))?;
    let at = recovery_now(&node);
    node.sys_write(&SystemCmd::StartAccountRecovery {
        signature: crate::proof::sign_recover(&code, &account, &cred, at),
        recovery_key,
        account,
        credential: cred,
        at,
    })
    .await
    .map(Json)
    .map_err(|e| {
        if e.message.contains(crate::system::UNKNOWN_RECOVERY_CODE) {
            wrong_recovery_code()
        } else {
            e
        }
    })
}

/// Cancel the recovery under way for the caller's account: a passkey the
/// account already has, approved over the passkey the recovery would add
/// (412 first). Whoever started it learns nothing; the code stays spent.
async fn account_recovery_cancel(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh().await;
    let pending = recovery_pending(&node, &account).ok_or_else(no_account_recovery)?;
    let credential_id = pending.credential.id;
    let consent = consent(
        &node,
        &p,
        &format!("account:{account}"),
        "__cancel_recovery",
        &json!({ "credentialId": credential_id }),
        body.and_then(|Json(b)| b.confirm),
    )
    .await?;
    let _ok = node
        .sys_write(&SystemCmd::CancelAccountRecovery {
            account,
            credential_id,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoveryFinish {
    /// The account the start answered: lets the first request refuse early
    /// (no recovery, too early) and name the passkey to use. Without it, the
    /// passkey that signs names the recovery.
    #[serde(default)]
    account_id: Option<String>,
    /// The retry after `412 CONFIRM_REQUIRED`: the new passkey's assertion.
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Finish a recovery once its delay passed: signed out, with the passkey the
/// start registered. The first request answers `412` with a sign-in
/// challenge stating the session the finish opens (`session / __sign_in`,
/// as every sign-in's: [`NewSession`]); the retry carries that passkey's
/// assertion, and the log adds the passkey, ends every session the account
/// had and opens this one, in one entry judged at this request's time. The
/// answer is the session.
async fn account_recovery_finish(
    State(node): State<Arc<Node>>,
    Json(body): Json<RecoveryFinish>,
) -> ApiResult {
    let Some(confirm) = body.confirm else {
        let allow = match &body.account_id {
            Some(account) => {
                node.sys_fresh().await;
                let p = recovery_pending(&node, account).ok_or_else(no_account_recovery)?;
                if !serves(&node, account) {
                    return Err(wrong_node());
                }
                if recovery_now(&node) < p.ready_at {
                    return Err(recovery_not_ready(p.ready_at));
                }
                vec![p.credential.id]
            }
            None => Vec::new(),
        };
        let session = NewSession::new();
        let statement = session.sign_in_statement();
        let challenge = statement.challenge().map_err(ApiError::internal)?;
        let id = put_pending(
            &node,
            Pending::RecoverFinish {
                challenge,
                expires: now_secs() + CHALLENGE_TTL,
                statement,
                session,
                account: body.account_id,
            },
        )?;
        return Err(ApiError::new(
            412,
            "CONFIRM_REQUIRED",
            "sign in with the passkey the recovery adds",
        )
        .with_details(json!({
            "challengeId": id,
            "challenge": b64url(&challenge),
            "rpId": node.config.rp_id,
            "allowCredentials": allow,
        })));
    };
    let Pending::RecoverFinish {
        challenge,
        statement,
        session,
        account: named,
        ..
    } = take_pending(&node, &confirm.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a recovery confirmation",
        ));
    };
    node.sys_fresh().await;
    let credential_id = &confirm.assertion.credential_id;
    let (account, pending) = node
        .sys(|s| {
            s.account_recoveries
                .iter()
                .find(|(a, p)| {
                    p.credential.id == *credential_id && named.as_ref().is_none_or(|n| n == *a)
                })
                .map(|(a, p)| (a.clone(), p.clone()))
        })
        .ok_or_else(no_account_recovery)?;
    if !serves(&node, &account) {
        return Err(wrong_node());
    }
    let at = recovery_now(&node);
    if at < pending.ready_at {
        return Err(recovery_not_ready(pending.ready_at));
    }
    let _count = verify_assertion(
        &RelyingParty {
            require_uv: true,
            ..rp(&node)
        },
        &challenge,
        &pending.credential,
        &confirm.assertion,
    )
    .map_err(|e| ApiError::unauthenticated(&e.to_string()))?;
    let passkey = pending.credential.id;
    let token = session_token_for(&node, &account, &passkey, None, None, &session)?;
    let consent = crate::proof::Consent {
        statement,
        assertion: confirm.assertion,
        // A sign-in: its activity entry comes from its session's opening.
        at: 0,
    };
    let _ok = node
        .sys_write(&SystemCmd::FinishAccountRecovery {
            account: account.clone(),
            at,
            refresh: RefreshGrant {
                hash: session.hash(),
                record: RefreshRecord {
                    opened_at: at,
                    ..session.record(&account, &passkey, None, None)
                },
            },
            consent: serde_json::to_value(&consent).unwrap_or_default(),
            revoke_api_keys: true,
        })
        .await?;
    Ok(session_answer(&account, &token, &session))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoveryKeyBody {
    /// The new key (base64url Ed25519 public key); `null` removes it.
    public_key: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Set, rotate or remove an organization's recovery key: the owner's, with a
/// passkey assertion (412 first). A recovery under way is cancelled.
async fn recovery_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<RecoveryKeyBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh().await;
    if node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&actor))) != Some(OrgRole::Owner) {
        return Err(ApiError::forbidden(crate::system::OWNER_ONLY_RECOVERY));
    }
    if let Some(k) = &body.public_key {
        let _key = calimero_identity::recovery::parse_public(k).map_err(|_| {
            ApiError::bad_request(
                "BAD_RECOVERY_KEY",
                "a recovery key is a base64url Ed25519 public key",
            )
        })?;
    }
    let args = json!({ "publicKey": body.public_key });
    let consent = org_consent(&node, &p, &org, "__set_recovery_key", &args, body.confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::SetRecoveryKey {
            org,
            public_key: body.public_key,
            at: recovery_now(&node),
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoveryStartBody {
    nonce: String,
    signature: String,
}

/// Start handing an organization to the signed-in account, with the recovery
/// key's signature over the organization, this account and `nonce`, made
/// where the seed is (the seed never reaches a node). Any account may try;
/// no organization, no key and a wrong signature answer the same `403`.
async fn recovery_start(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<RecoveryStartBody>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    node.sys_fresh().await;
    let key = node.sys(|s| {
        s.orgs
            .get(&org)
            .and_then(|o| o.recovery.as_ref())
            .map(|r| r.public_key.clone())
    });
    let signed = key.is_some_and(|k| {
        crate::proof::verify_recovery(&k, &org, &account, &body.nonce, &body.signature).is_ok()
    });
    if !signed {
        return Err(ApiError::new(
            403,
            "WRONG_RECOVERY_KEY",
            "the recovery is not signed with this organization's recovery key",
        ));
    }
    let v = node
        .sys_write(&SystemCmd::StartRecovery {
            org,
            account,
            nonce: body.nonce,
            signature: body.signature,
            at: recovery_now(&node),
        })
        .await?;
    Ok(Json(v))
}

/// Cancel the recovery under way: the owner's, with a passkey assertion.
async fn recovery_cancel(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh().await;
    let (role, pending) = node.sys(|s| {
        let o = s.orgs.get(&org);
        (
            o.and_then(|o| o.role(&actor)),
            o.and_then(|o| o.recovery.as_ref())
                .and_then(|r| r.pending.as_ref())
                .map(|p| p.account.clone()),
        )
    });
    if role != Some(OrgRole::Owner) {
        return Err(ApiError::forbidden(crate::system::OWNER_ONLY_RECOVERY));
    }
    let Some(account) = pending else {
        return Err(ApiError::not_found(crate::system::NO_RECOVERY_PENDING));
    };
    let confirm = body.and_then(|Json(b)| b.confirm);
    let args = json!({ "account": account });
    let consent = org_consent(&node, &p, &org, "__cancel_recovery", &args, confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::CancelRecovery {
            org,
            account,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Finish a recovery once its delay passed: the account it names, with that
/// account's passkey. The account becomes the organization's owner.
async fn recovery_finish(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh().await;
    let pending = node.sys(|s| {
        s.orgs
            .get(&org)
            .and_then(|o| o.recovery.as_ref())
            .and_then(|r| r.pending.clone())
            .filter(|p| p.account == actor)
    });
    let Some(pending) = pending else {
        return Err(ApiError::not_found(crate::system::NO_RECOVERY_PENDING));
    };
    let at = recovery_now(&node);
    if at < pending.ready_at {
        return Err(
            ApiError::new(409, "RECOVERY_NOT_READY", crate::system::RECOVERY_NOT_READY)
                .with_details(json!({ "readyAt": pending.ready_at })),
        );
    }
    let confirm = body.and_then(|Json(b)| b.confirm);
    let args = json!({ "org": org });
    let consent = org_consent(&node, &p, &org, "__finish_recovery", &args, confirm).await?;
    let v = node
        .sys_write(&SystemCmd::FinishRecovery {
            org,
            at,
            actor,
            consent,
        })
        .await?;
    Ok(Json(v))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route(
            "/auth/recovery",
            post(recovery_create).delete(recovery_remove),
        )
        .route("/auth/recovery/begin", post(account_recovery_begin))
        .route("/auth/recovery/start", post(account_recovery_start))
        .route("/auth/recovery/cancel", post(account_recovery_cancel))
        .route("/auth/recovery/finish", post(account_recovery_finish))
        .route("/orgs/{org}/recovery", post(recovery_set))
        .route("/orgs/{org}/recovery/start", post(recovery_start))
        .route("/orgs/{org}/recovery/cancel", post(recovery_cancel))
        .route("/orgs/{org}/recovery/finish", post(recovery_finish))
}
