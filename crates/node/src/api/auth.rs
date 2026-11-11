//! The API's routes for passkey sign-up and sign-in, sessions, the account, aliases and API keys.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// A sign-in's session: its refresh token logged with the sign-in's
/// assertion, then the session token.
async fn new_session(
    node: &Node,
    account: &str,
    passkey: &str,
    from: Option<AppOrigin>,
    new: NewSession,
    consent: Value,
) -> ApiResult {
    let origin = from.as_ref().and_then(AppOrigin::registered);
    let app = from.map(|a| a.package);
    if let (Some(package), Some(origin)) = (&app, &origin) {
        info!(%package, %origin, "a session opened on a registered origin");
    }
    let token = session_token_for(node, account, passkey, app.clone(), origin.clone(), &new)?;
    let record = RefreshRecord {
        opened_at: now_secs(),
        ..new.record(account, passkey, app, origin)
    };
    let _ok = node
        .sys_write(&SystemCmd::CreateRefresh {
            hash: new.hash(),
            record,
            consent,
        })
        .await?;
    Ok(session_answer(account, &token, &new))
}

#[derive(Deserialize)]
pub(super) struct RegisterBegin {
    #[serde(default)]
    name: String,
    /// The page's origin, for a registered origin's rp-id.
    #[serde(default)]
    origin: Option<String>,
    /// The invite the person signs up with (an organization invite token or
    /// a cluster sign-up invite token), checked here so a cluster whose
    /// sign-up is not open refuses before the passkey ceremony; the finish
    /// carries it again.
    #[serde(default)]
    invite: Option<String>,
}

/// What a sign-in begins with: the page's origin, when it is a registered
/// one (its rp-id differs from the cluster's).
// Open to other fields: clients sent arbitrary bodies here before it took any.
#[derive(Deserialize, Default)]
pub(super) struct LoginBegin {
    #[serde(default)]
    origin: Option<String>,
}

/// The rp-id a ceremony begun on `origin` uses: the cluster's for its own
/// origins and the frontends it serves (and when no origin is named), a
/// registered origin's own; any other origin is refused, `403
/// ORIGIN_NOT_REGISTERED`.
async fn begin_rp_id(node: &Node, origin: Option<&str>) -> Result<String, ApiError> {
    let cluster = node.config.rp_id.clone();
    let Some(origin) = origin else {
        return Ok(cluster);
    };
    // The cluster's own origins, and the nodes' own (a page a node serves).
    if node
        .config
        .origins
        .iter()
        .any(|o| o.trim_end_matches('/') == origin)
        || node.api_origins().iter().any(|o| o == origin)
    {
        return Ok(cluster);
    }
    let mut found = app_origin(node, origin);
    if found.is_none() {
        // Registered through another node a moment ago, perhaps.
        let _caught_up = node.sys_fresh().await;
        found = app_origin(node, origin);
    }
    found.map(|a| a.rp_id.unwrap_or(cluster)).ok_or_else(|| {
        ApiError::new(
            403,
            "ORIGIN_NOT_REGISTERED",
            "this origin is not one of the cluster's, nor registered for an app",
        )
    })
}

/// Refuse a sign-up the cluster's policy would refuse (`403
/// SIGN_UP_REFUSED`), before a challenge is made or spent. A refusal is
/// judged again on fresh state, so a replica behind a policy change or a new
/// invite does not refuse what the log would take; `apply` decides in the end.
async fn refuse_sign_up(node: &Node, invite: Option<&str>) -> Result<(), ApiError> {
    let key = invite.map(crate::proof::invite_key);
    let now = now_secs();
    let judge = |s: &SystemState| s.admits_key(key.as_deref().map(|k| (k, now))).map(|_| ());
    if node.sys(judge).is_ok() {
        return Ok(());
    }
    let _caught_up = node.sys_fresh().await;
    node.sys(judge).map_err(crate::node::rejected)
}

async fn register_begin(
    State(node): State<Arc<Node>>,
    Json(body): Json<RegisterBegin>,
) -> ApiResult {
    refuse_sign_up(&node, body.invite.as_deref()).await?;
    let rp_id = begin_rp_id(&node, body.origin.as_deref()).await?;
    let challenge = random::<32>();
    let user_id = random::<16>();
    let name = if body.name.is_empty() {
        "Calimero user".to_owned()
    } else {
        body.name
    };
    // `begin_rp_id` admitted the origin, on fresh state when it had to.
    let app = body
        .origin
        .as_deref()
        .and_then(|o| app_origin(&node, o))
        .map(|a| passkeys::app_name(&node, &a.package));
    let shown = passkeys::passkey_user_name(
        &name,
        &node.config.cluster,
        &passkeys::cluster_tag(&node),
        app.as_deref(),
    );
    let id = put_pending(
        &node,
        Pending::Register {
            challenge,
            name: name.clone(),
            expires: now_secs() + CHALLENGE_TTL,
        },
    )?;
    Ok(Json(json!({
        "challengeId": id,
        "options": {
            "challenge": b64url(&challenge),
            "rp": { "id": rp_id, "name": "Calimero" },
            "user": { "id": b64url(&user_id), "name": shown, "displayName": shown },
            "pubKeyCredParams": [ { "type": "public-key", "alg": -7 }, { "type": "public-key", "alg": -8 } ],
            "timeout": 60000,
            "attestation": "none",
            "authenticatorSelection": { "residentKey": "required", "userVerification": "preferred" },
        }
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RegisterFinish {
    challenge_id: String,
    registration: Registration,
    /// The invite the account is created with ([`RegisterBegin::invite`]),
    /// sent on the first finish and again on its `412` retry: the policy is
    /// judged before either spends its challenge, and the retry's node signs
    /// the redemption with the token's key.
    #[serde(default)]
    invite: Option<String>,
    /// The retry after `412 CONFIRM_REQUIRED`: the new passkey's assertion
    /// over the account it creates.
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

async fn register_finish(
    State(node): State<Arc<Node>>,
    Json(body): Json<RegisterFinish>,
) -> ApiResult {
    // A new account approves this node's release, so only a release the
    // operator did not approve (where it holds release approvals) refuses it:
    // decided before the challenge is spent, so the client can finish on
    // another node. An account no one holds yet stands for the new one.
    if !serves(&node, "") {
        return Err(wrong_node());
    }
    // Before a challenge is spent, on both halves: a person refused for a
    // missing or bad invite starts again with one.
    refuse_sign_up(&node, body.invite.as_deref()).await?;
    if let Some(confirm) = body.confirm {
        return sign_up_confirmed(&node, confirm, body.invite.as_deref()).await;
    }
    let Pending::Register {
        challenge, name, ..
    } = take_pending(&node, &body.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a registration challenge",
        ));
    };
    // Signing up on a package's frontend opens a session bound to it.
    let app = claimed_app(&node, &body.registration.client_data_json);
    let cred = verify_registration(
        &rp_with(rp(&node), app.as_ref()),
        &challenge,
        &body.registration,
    )
    .map_err(|e| ApiError::bad_request("BAD_REGISTRATION", &e.to_string()))?;
    let account = Account {
        account_id: typed_id::<32>(),
        name,
        created_at: now_secs(),
        passkeys: vec![cred.clone()],
        // The release this account began on holds its apps until the
        // owner approves another.
        approved_releases: [node.own_release()].into_iter().collect(),
        outbound_approvals: std::collections::BTreeMap::new(),
        passkey_labels: std::collections::BTreeMap::new(),
        aliases: std::collections::BTreeMap::new(),
        // Set at apply, from the sign-up's time (`CreateAccount::at`).
        passkey_times: std::collections::BTreeMap::new(),
    };
    // A registration carries no signature by the new key (attestation
    // `none`): the new passkey signs the account it creates, and the session
    // it opens, before the log takes it (412, then the retry).
    let session = NewSession::new();
    // The new passkey belongs to the rp-id of the origin it was made on.
    let rp_id = app
        .as_ref()
        .and_then(|a| a.rp_id.clone())
        .unwrap_or_else(|| node.config.rp_id.clone());
    let origin = app.as_ref().and_then(AppOrigin::registered);
    let app = app.map(|a| a.package);
    let grant = RefreshGrant {
        hash: session.hash(),
        record: session.record(&account.account_id, &cred.id, app.clone(), origin.clone()),
    };
    let statement = crate::proof::Statement::new(
        &format!("account:{}", account.account_id),
        "__create_account",
        &crate::system::account_args(&account, Some(&grant)),
    );
    let signed = statement.challenge().map_err(ApiError::internal)?;
    let id = put_pending(
        &node,
        Pending::SignUp {
            challenge: signed,
            expires: now_secs() + CHALLENGE_TTL,
            statement,
            account: Box::new(account),
            session,
            app,
            origin,
        },
    )?;
    Err(confirm_required(
        "sign in with the new passkey to create the account",
        &id,
        &signed,
        &rp_id,
        std::slice::from_ref(&cred.id),
    ))
}

/// The sign-up's retry: the new passkey's assertion over the account, then
/// the account and its first session. `invite` (the same token the first
/// half carried) is redeemed for the account here, signed with its key.
async fn sign_up_confirmed(node: &Node, confirm: ConfirmBody, invite: Option<&str>) -> ApiResult {
    let Pending::SignUp {
        challenge,
        statement,
        account,
        session,
        app,
        origin,
        ..
    } = take_pending(node, &confirm.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a sign-up confirmation",
        ));
    };
    let cred = account
        .passkeys
        .first()
        .ok_or_else(|| ApiError::internal("a sign-up without its passkey"))?;
    // From the origin the sign-up came from: the package's (the registered
    // origin itself, for one), or the cluster's.
    let from = claimed_app(node, &confirm.assertion.client_data_json)
        .filter(|a| app.as_deref() == Some(a.package.as_str()) && a.registered() == origin);
    if origin.is_some() && from.is_none() {
        return Err(ApiError::unauthenticated(
            "confirm the sign-up on the origin it began on",
        ));
    }
    let _count = verify_assertion(
        &rp_with(rp(node), from.as_ref()),
        &challenge,
        cred,
        &confirm.assertion,
    )
    .map_err(|e| ApiError::unauthenticated(&e.to_string()))?;
    let account_id = account.account_id.clone();
    let passkey = cred.id.clone();
    let token = session_token_for(
        node,
        &account_id,
        &passkey,
        app.clone(),
        origin.clone(),
        &session,
    )?;
    let consent = crate::proof::Consent {
        statement,
        assertion: confirm.assertion,
        // A sign-in: its activity entry comes from its session's opening.
        at: 0,
    };
    let now = now_secs();
    let invite = invite.map(|token| crate::system::SignUpRedemption {
        invite_key: crate::proof::invite_key(token),
        now,
        signature: crate::proof::sign_invite_redeem(token, &account_id, now),
    });
    let _ok = node
        .sys_write(&SystemCmd::CreateAccount {
            account: *account,
            invite,
            refresh: Some(RefreshGrant {
                hash: session.hash(),
                record: session.record(&account_id, &passkey, app, origin),
            }),
            consent: serde_json::to_value(&consent).unwrap_or_default(),
            at: now,
        })
        .await?;
    Ok(session_answer(&account_id, &token, &session))
}

async fn login_begin(State(node): State<Arc<Node>>, body: Option<Json<LoginBegin>>) -> ApiResult {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let rp_id = begin_rp_id(&node, body.origin.as_deref()).await?;
    // The challenge states the session the sign-in opens, so the log takes
    // its refresh token only with this assertion (`CreateRefresh`).
    let session = NewSession::new();
    let statement = session.sign_in_statement();
    let challenge = statement.challenge().map_err(ApiError::internal)?;
    let id = put_pending(
        &node,
        Pending::Login {
            challenge,
            expires: now_secs() + CHALLENGE_TTL,
            statement,
            session,
        },
    )?;
    Ok(Json(json!({
        "challengeId": id,
        "options": { "challenge": b64url(&challenge), "rpId": rp_id, "userVerification": "preferred", "timeout": 60000 }
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AssertionBody {
    challenge_id: String,
    assertion: Assertion,
}

/// Verify an assertion for `challenge` and move the credential's counter.
/// `bound`: the ceremony may come from that package's frontend origin only;
/// `None` with `any_app`, from any package's (a sign-in there opens a session
/// bound to it). Answers the account and the package whose origin it came from.
async fn check_assertion(
    node: &Node,
    challenge: &[u8; 32],
    assertion: &Assertion,
    account: Option<&str>,
    step_up: bool,
    bound: Option<&str>,
    any_app: bool,
) -> Result<(String, Option<AppOrigin>), ApiError> {
    let base = if step_up { rp_step_up(node) } else { rp(node) };
    check_assertion_for(node, base, challenge, assertion, account, bound, any_app).await
}

async fn login_finish(State(node): State<Arc<Node>>, Json(body): Json<AssertionBody>) -> ApiResult {
    // The account the passkey names, if this replica knows it: refused before
    // the challenge is spent when this node may not serve it, so the client
    // finishes the same ceremony on another node. (A replica that does not
    // know the passkey yet decides after the assertion is checked.)
    let named = node.sys(|s| {
        s.account_of_credential(&body.assertion.credential_id)
            .map(|(a, _)| a.account_id.clone())
    });
    if named.is_some_and(|a| !serves(&node, &a)) {
        return Err(wrong_node());
    }
    let Pending::Login {
        challenge,
        statement,
        session,
        ..
    } = take_pending(&node, &body.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a login challenge",
        ));
    };
    let (account, app) =
        check_assertion(&node, &challenge, &body.assertion, None, false, None, true).await?;
    let consent = crate::proof::Consent {
        statement,
        assertion: body.assertion.clone(),
        // A sign-in: its activity entry comes from its session's opening.
        at: 0,
    };
    new_session(
        &node,
        &account,
        &body.assertion.credential_id,
        app,
        session,
        serde_json::to_value(&consent).unwrap_or_default(),
    )
    .await
}

async fn passkey_add_begin(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let challenge = random::<32>();
    let id = put_pending(
        &node,
        Pending::AddPasskey {
            challenge,
            account: account.clone(),
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
            "attestation": "none",
        }
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AddPasskeyFinish {
    challenge_id: String,
    registration: Registration,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// What a person approves when adding a passkey (`__add_passkey`).
fn add_passkey_args(cred: &calimero_identity::Credential) -> Value {
    json!({ "credentialId": cred.id, "publicKey": cred.public_key })
}

/// Add a passkey: the registration is checked first, then a passkey the
/// account already has approves exactly that credential (412 first). The
/// retry carries the approval; the registration it repeats was spent by the
/// first request, and the credential comes back with the challenge.
async fn passkey_add_finish(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<AddPasskeyFinish>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let made = if body.confirm.is_some() {
        None
    } else {
        let Pending::AddPasskey {
            challenge,
            account: issued,
            ..
        } = take_pending(&node, &body.challenge_id).await?
        else {
            return Err(ApiError::bad_request(
                "WRONG_CHALLENGE",
                "not an add-passkey challenge",
            ));
        };
        if issued != account {
            return Err(ApiError::forbidden("challenge issued to another account"));
        }
        Some(
            verify_registration(&rp(&node), &challenge, &body.registration)
                .map_err(|e| ApiError::bad_request("BAD_REGISTRATION", &e.to_string()))?,
        )
    };
    let args = made.as_ref().map_or(Value::Null, add_passkey_args);
    let carry = made.and_then(|c| serde_json::to_value(c).ok());
    let c = consent_minted(
        &node,
        &p,
        &format!("account:{account}"),
        "__add_passkey",
        &args,
        carry,
        body.confirm,
    )
    .await?;
    let cred: calimero_identity::Credential = c
        .carry
        .and_then(|v| serde_json::from_value(v).ok())
        .filter(|cred| add_passkey_args(cred) == c.args)
        .ok_or_else(|| ApiError::bad_request("WRONG_CHALLENGE", "not an add-passkey approval"))?;
    let id = cred.id.clone();
    let _ok = node
        .sys_write(&SystemCmd::AddPasskey {
            account,
            credential: cred,
            consent: c.consent,
            at: now_secs(),
        })
        .await?;
    Ok(Json(json!({ "credentialId": id })))
}

/// Remove a passkey: the account's, approved with a passkey (412 first; the
/// retry's JSON body carries `confirm`).
async fn passkey_remove(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(cred): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let consent = consent(
        &node,
        &p,
        &format!("account:{account}"),
        "__remove_passkey",
        &json!({ "credentialId": cred }),
        body.and_then(|Json(b)| b.confirm),
    )
    .await?;
    let _ok = node
        .sys_write(&SystemCmd::RemovePasskey {
            account,
            credential_id: cred,
            now: now_secs(),
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RefreshBody {
    refresh_token: String,
}

async fn refresh(State(node): State<Arc<Node>>, Json(body): Json<RefreshBody>) -> ApiResult {
    let old_hash = b64url(&sha256(body.refresh_token.as_bytes()));
    let mut found = node.sys_until(|s| s.refresh.get(&old_hash).cloned()).await;
    let mut caught_up = true;
    if found.is_none() {
        // Not here yet, or exchanged already: this replica may be behind, so
        // ask the leader how far the log is before telling a reused token
        // (revoke the session) from an unknown one.
        caught_up = node.sys_fresh().await;
        found = node.sys(|s| s.refresh.get(&old_hash).cloned());
    }
    let Some(rec) = found else {
        // A token already exchanged, presented again: someone else holds a
        // copy, so the session it belongs to ends here. `RefreshReused` is
        // refused at apply for a token never exchanged, so a replica that
        // could not catch up lets the system log decide rather than answer
        // "unknown" to a thief from a stale view.
        let known_here = node.sys(|s| s.rotated_refresh.contains_key(&old_hash));
        if known_here || !caught_up {
            let revoked = node
                .sys_write(&SystemCmd::RefreshReused {
                    hash: old_hash,
                    now: now_secs(),
                })
                .await;
            if known_here || revoked.is_ok() {
                return Err(ApiError::unauthenticated(
                    "refresh token already used; the session is revoked",
                ));
            }
            // Not exchanged at the leader either: it may be a token issued
            // after this replica fell behind, so do not call it unknown.
            return Err(ApiError::unavailable(
                "this node is behind the cluster; retry the refresh",
            ));
        }
        return Err(ApiError::unauthenticated("unknown refresh token"));
    };
    // Decided before the token is spent: a `421 WRONG_NODE` leaves it valid
    // for another node, where presenting it again would otherwise look like a
    // stolen copy and revoke the session.
    if !serves(&node, &rec.account) {
        return Err(wrong_node());
    }
    if let Some(origin) = &rec.origin {
        // Judged on the cluster's latest state: a removal made through
        // another node holds here at once.
        let _caught_up = node.sys_fresh().await;
        if !origin_holds(&node, rec.app.as_deref(), origin) {
            return Err(origin_gone());
        }
    }
    if let Some(kept) = kept_refresh(&node, &body.refresh_token, &old_hash, &rec).await? {
        return Ok(kept);
    }
    let new_refresh = format!("rt_{}", b64url(&random::<32>()));
    let record = RefreshRecord {
        account: rec.account.clone(),
        session: rec.session.clone(),
        expires_at: now_secs() + REFRESH_TTL,
        passkey: rec.passkey.clone(),
        app: rec.app.clone(),
        origin: rec.origin.clone(),
        key: crate::proof::refresh_key(&new_refresh),
        opened_at: rec.opened_at,
    };
    let new_hash = b64url(&sha256(new_refresh.as_bytes()));
    let now = now_secs();
    // Signed with the presented token's key: the log exchanges a token only
    // for whoever holds it, never for a node that only saw its hash.
    let signature = crate::proof::sign_refresh_use(
        &body.refresh_token,
        &crate::proof::refresh_use_message(
            &old_hash,
            &new_hash,
            &record.key,
            &record.session,
            record.expires_at,
            now,
        ),
    );
    let exchanged = node
        .sys_write(&SystemCmd::UseRefresh {
            old_hash: old_hash.clone(),
            new_hash,
            record,
            now,
            signature,
        })
        .await;
    if let Err(e) = exchanged {
        // This replica still saw the token as live, but the log had exchanged
        // it already: a copy, decided by the log itself (`RefreshReused` is
        // refused at apply for a token never exchanged).
        let revoked = node
            .sys_write(&SystemCmd::RefreshReused {
                hash: old_hash,
                now: now_secs(),
            })
            .await;
        if revoked.is_ok() {
            return Err(ApiError::unauthenticated(
                "refresh token already used; the session is revoked",
            ));
        }
        return Err(ApiError::unauthenticated(&e.message));
    }
    let expires_at = now_secs() + SESSION_TTL;
    let token = session_token(
        &node,
        SessionClaims {
            account: rec.account.clone(),
            session: rec.session,
            expires_at,
            passkey: rec.passkey,
            app: rec.app,
            origin: rec.origin,
            node: 0,
            key: String::new(),
        },
    )?;
    Ok(Json(
        json!({ "accountId": rec.account, "sessionToken": token, "expiresAt": expires_at, "refreshToken": new_refresh }),
    ))
}

/// A refresh that keeps its token and writes nothing to the system log: the
/// token is younger than `Config::refresh_rotate_after`, and the cluster's
/// latest system state (a read index the system leader confirmed) still holds
/// it, live, for a session that is not revoked, of an account that still has
/// the passkey it was opened with. So a sign-out, a sign-out everywhere, a
/// recovery or a passkey removal committed through any node before this
/// refresh ends it here, and no node mints a session for a token the log does
/// not hold. `None`: the log decides, by exchanging the token (`UseRefresh`):
/// it is old enough to rotate, it uses no key (`RefreshRecord::key`), this
/// replica could not catch up, or the fresh state no longer holds it (revoked,
/// or exchanged elsewhere, which the log then tells from a stolen copy).
async fn kept_refresh(
    node: &Node,
    token: &str,
    hash: &str,
    rec: &RefreshRecord,
) -> Result<Option<Json<Value>>, ApiError> {
    let now = now_secs();
    let issued = rec.expires_at.saturating_sub(REFRESH_TTL);
    if rec.key.is_empty()
        || now.saturating_sub(issued) >= node.config.refresh_rotate_after.as_secs()
    {
        return Ok(None);
    }
    if !node.sys_fresh().await {
        return Ok(None);
    }
    let live = node.sys(|s| {
        s.refresh.get(hash) == Some(rec)
            && rec.expires_at > now
            && !s.revoked_sessions.contains(&rec.session)
            && rec
                .passkey
                .as_ref()
                .is_none_or(|p| s.credentials.get(p) == Some(&rec.account))
    });
    if !live {
        return Ok(None);
    }
    let expires_at = now_secs() + SESSION_TTL;
    let session = session_token(
        node,
        SessionClaims {
            account: rec.account.clone(),
            session: rec.session.clone(),
            expires_at,
            passkey: rec.passkey.clone(),
            app: rec.app.clone(),
            origin: rec.origin.clone(),
            node: 0,
            key: String::new(),
        },
    )?;
    Ok(Some(Json(json!({
        "accountId": rec.account,
        "sessionToken": session,
        "expiresAt": expires_at,
        "refreshToken": token,
    }))))
}

async fn logout(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = person(&node, &headers).await?;
    let session = p.session.map(|s| s.session).unwrap_or_default();
    let _ok = node
        .sys_write(&SystemCmd::RevokeSession {
            session,
            actor: p.caller.account().unwrap_or_default().to_owned(),
            now: now_secs(),
        })
        .await?;
    Ok(Json(json!({})))
}

/// Sign out everywhere: revoke every session of the caller's account.
async fn logout_all(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let out = node
        .sys_write(&SystemCmd::RevokeAllSessions {
            actor: p.account()?,
            now: now_secs(),
        })
        .await?;
    Ok(Json(out))
}

/// The most sessions `GET /auth/sessions` lists: the latest active.
const MAX_SESSIONS_LISTED: usize = 256;

/// The longest session id `DELETE /auth/sessions/{session}` looks up (ids
/// are 22 characters, `NewSession`); a longer one is no session.
const MAX_SESSION_ID_CHARS: usize = 64;

/// The account's live sessions, latest active first: each one's passkey, the
/// package and registered origin it is bound to, when it opened (`null`:
/// before the node kept it) and when its refresh token was last exchanged.
/// A refresh token younger than `Config::refresh_rotate_after` (an hour) is
/// kept without a write, and a session token lives [`SESSION_TTL`], so a
/// session may have been used up to that much after `lastActiveAt`.
fn account_sessions(s: &SystemState, account: &str, current: Option<&str>, now: u64) -> Vec<Value> {
    let mut latest: BTreeMap<&str, &RefreshRecord> = BTreeMap::new();
    for r in s.refresh.values() {
        if r.account != account || r.expires_at <= now || s.revoked_sessions.contains(&r.session) {
            continue;
        }
        let _prev = latest
            .entry(r.session.as_str())
            .and_modify(|seen| {
                if r.expires_at > seen.expires_at {
                    *seen = r;
                }
            })
            .or_insert(r);
    }
    let mut sessions: Vec<&RefreshRecord> = latest.into_values().collect();
    sessions.sort_by_key(|r| std::cmp::Reverse(r.expires_at));
    sessions
        .into_iter()
        .take(MAX_SESSIONS_LISTED)
        .map(|r| {
            json!({
                "session": r.session,
                "passkey": r.passkey,
                "app": r.app,
                "origin": r.origin,
                "openedAt": (r.opened_at > 0).then_some(r.opened_at),
                "lastActiveAt": r.expires_at.saturating_sub(REFRESH_TTL),
                "current": current == Some(r.session.as_str()),
            })
        })
        .collect()
}

/// `GET /auth/sessions`: the caller's own sessions ([`account_sessions`]).
/// The account holder's alone, on the cluster's own origin: never an API key
/// or a package-bound session, which would learn the other apps and origins
/// the person signs in to.
async fn sessions_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let current = p.session.as_ref().map(|c| c.session.clone());
    let now = now_secs();
    let sessions = node.sys(|s| account_sessions(s, &account, current.as_deref(), now));
    Ok(Json(json!({ "sessions": sessions })))
}

/// `DELETE /auth/sessions/{session}`: end one of the caller's own sessions,
/// gated as sign-out everywhere is (the account holder's session, no
/// passkey asked: it ends less than that does). A session the account does
/// not hold live, another account's included, is `404`, so the answer tells
/// nothing about anyone else's; `RevokeSession` judges it again at apply.
async fn session_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(session): Path<String>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    if session.len() > MAX_SESSION_ID_CHARS {
        return Err(ApiError::not_found("no such session"));
    }
    // A session opened through another node a moment ago is the caller's too.
    let _caught_up = node.sys_fresh().await;
    let now = now_secs();
    let theirs = node.sys(|s| {
        !s.revoked_sessions.contains(&session)
            && s.refresh
                .values()
                .any(|r| r.session == session && r.account == account && r.expires_at > now)
    });
    if !theirs {
        return Err(ApiError::not_found("no such session"));
    }
    let _ok = node
        .sys_write(&SystemCmd::RevokeSession {
            session,
            actor: account,
            now,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub(super) struct ActivityQuery {
    /// Only entries after this time (Unix seconds).
    #[serde(default)]
    after: u64,
}

/// `GET /auth/activity`: what the caller's passkeys did, newest first
/// (`SystemState::activity`): each sign-in, with the package and origin
/// its session is bound to, and each change a passkey approved, with the
/// method and subject its consent states. The account holder's alone, as
/// `GET /auth/sessions` is: never an API key or a package-bound session,
/// which would learn the other apps the person signs in to and what they
/// approve; no one else (operator, organization admin, deployment manager)
/// has a route to it.
async fn activity_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<ActivityQuery>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let activity = node.sys(|s| {
        s.activity.get(&account).map_or_else(Vec::new, |a| {
            a.iter()
                .rev()
                .filter(|e| e.at > q.after)
                .map(activity_entry)
                .collect::<Vec<_>>()
        })
    });
    Ok(Json(json!({ "activity": activity })))
}

/// One activity entry as `GET /auth/activity` lists it: every key present,
/// the ones its kind does not have `null`.
fn activity_entry(e: &crate::types::ActivityEntry) -> Value {
    use crate::types::Activity;
    let (kind, app, origin, method, subject) = match &e.what {
        Activity::SignIn { app, origin } => {
            ("signIn", app.as_deref(), origin.as_deref(), None, None)
        }
        Activity::Approval { method, subject } => (
            "approval",
            None,
            None,
            Some(method.as_str()),
            Some(subject.as_str()),
        ),
    };
    json!({
        "at": e.at,
        "passkey": e.passkey,
        "kind": kind,
        "app": app,
        "origin": origin,
        "method": method,
        "subject": subject,
    })
}

async fn me(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let (acc, orgs, (recovery, pending)) = node.sys(|s| {
        let acc = s.accounts.get(&account).cloned();
        let recovery = (
            s.recovery_keys.values().any(|a| *a == account),
            s.account_recoveries.get(&account).map(|p| p.json(&account)),
        );
        let orgs: Vec<Value> = s
            .orgs
            .values()
            .filter_map(|o| {
                o.role(&account)
                    .map(|r| json!({ "orgId": o.org_id, "name": o.name, "role": r }))
            })
            .collect();
        (acc, orgs, recovery)
    });
    let acc = acc.ok_or_else(|| ApiError::not_found("account"))?;
    Ok(Json(json!({
        "accountId": acc.account_id,
        "name": acc.name,
        "passkeys": acc.passkeys.iter().map(|c| {
            let times = acc.passkey_times.get(&c.id);
            json!({
                "id": c.id,
                "signCount": c.sign_count,
                "label": acc.passkey_labels.get(&c.id),
                // When it was added and last signed in (Unix seconds);
                // `null`: before the node kept it.
                "addedAt": times.and_then(|t| t.added_at),
                "lastSignInAt": times.and_then(|t| t.last_sign_in_at),
            })
        }).collect::<Vec<_>>(),
        "orgs": orgs,
        "via": if p.api_key.is_some() { "apiKey" } else { "session" },
        // The passkey (device) this session was opened with.
        "passkeyId": p.session.as_ref().and_then(|s| s.passkey.clone()),
        // The package a frontend's session is bound to, and the registered
        // origin that opened it.
        "app": p.bound_app(),
        "origin": p.session.as_ref().and_then(|s| s.origin.clone()),
        // Releases approved to hold this account's own apps.
        "approvedReleases": acc.approved_releases,
        // Outbound prefixes approved for this account's own apps, by signer.
        "outboundApprovals": acc.outbound_approvals,
        // Whether a recovery code is set (`/auth/recovery`); never the code.
        "recoveryCode": recovery,
        // A recovery under way (`/auth/recovery/start`), which a passkey of
        // this account cancels (`/auth/recovery/cancel`) before `readyAt`.
        "recoveryPending": pending,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApiKeyBody {
    #[serde(default)]
    label: String,
    #[serde(default)]
    apps: Option<Vec<String>>,
    #[serde(default)]
    methods: Option<Vec<String>>,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    allow_confirm: bool,
    #[serde(default)]
    ttl_secs: Option<u64>,
    /// An upgrade key: upgrades this app, and nothing else.
    #[serde(default)]
    upgrades: Option<UpgradesBody>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// An upgrade key's scope as asked: the app, and the package whose signed
/// releases it may move the app to (default: the package the app runs).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpgradesBody {
    app: String,
    #[serde(default)]
    package: Option<String>,
}

/// The scope an upgrade key asked for `body` gets, its proof key left for
/// the key it is made with: the app (by id or context id), its package by
/// default, and that package's pinned signer as the log holds it now.
async fn upgrade_scope(
    node: &Node,
    account: &str,
    body: &UpgradesBody,
) -> Result<crate::types::UpgradeScope, ApiError> {
    let app = node.app(&node.canonical_app_id(&body.app)).await?;
    let package = body
        .package
        .clone()
        .or_else(|| node.release_of_app(&app).map(|r| r.at.package))
        .ok_or_else(|| {
            ApiError::bad_request(
                "NOT_A_PACKAGE",
                "an upgrade key needs an app that runs a signed package",
            )
        })?;
    let signer = node
        .sys(|s| s.packages.get(&package).map(|p| p.signer_id.clone()))
        .ok_or_else(|| ApiError::not_found("no such package"))?;
    if !node.sys(|s| s.may_upgrade(&app, account)) {
        return Err(ApiError::forbidden(
            "an upgrade key is made by someone who may upgrade the app",
        ));
    }
    Ok(crate::types::UpgradeScope {
        app: app.app_id,
        package,
        signer,
        proof_key: String::new(),
    })
}

/// The longest an API key lives.
const MAX_API_KEY_TTL: u64 = 365 * 24 * 3600;

/// Create an API key: the account's, approved with a passkey over the key's
/// record and hash (412 first). The first request makes the key, its id and
/// expiry and keeps them with the challenge; the retry logs what was
/// approved. A passkey session only: an API key cannot make another.
async fn apikey_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<ApiKeyBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let (key, hash) = calimero_identity::new_api_key();
    let upgrades = match &body.upgrades {
        Some(u) => Some(upgrade_scope(&node, &account, u).await?),
        None => None,
    };
    let with_key = |key: &str| {
        upgrades.clone().map(|u| crate::types::UpgradeScope {
            proof_key: crate::proof::api_key_public(key),
            ..u
        })
    };
    let record = ApiKeyRecord {
        key_id: typed_id::<8>(),
        account: account.clone(),
        label: body.label,
        apps: body.apps,
        methods: body.methods,
        read_only: body.read_only,
        allow_confirm: body.allow_confirm,
        expires_at: now_secs()
            + body
                .ttl_secs
                .unwrap_or(MAX_API_KEY_TTL)
                .min(MAX_API_KEY_TTL),
        upgrades: with_key(&key),
    };
    if let Some(scope) = &record.upgrades {
        node.sys(|s| s.upgrade_key_fits(&record, scope))
            .map_err(|e| ApiError::bad_request("BAD_UPGRADE_KEY", e))?;
    }
    let args = crate::system::api_key_args(&hash, &record);
    let c = consent_minted(
        &node,
        &p,
        &format!("account:{account}"),
        "__create_api_key",
        &args,
        Some(json!(key)),
        body.confirm,
    )
    .await?;
    // The retry logs the key the approval names, made by the first request;
    // its scope must be the one this request asks for.
    let key = c
        .carry
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("WRONG_CHALLENGE", "not an API key approval"))?;
    let hash = calimero_identity::api_key_hash(&key);
    let signed = |f: &str| c.args.get(f).cloned().unwrap_or(Value::Null);
    let record = ApiKeyRecord {
        key_id: signed("keyId").as_str().unwrap_or_default().to_owned(),
        expires_at: signed("expiresAt").as_u64().unwrap_or_default(),
        upgrades: with_key(&key),
        ..record
    };
    if crate::system::api_key_args(&hash, &record) != c.args
        || record.expires_at > now_secs() + MAX_API_KEY_TTL
    {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "the approval was for a different call",
        ));
    }
    let key_id = record.key_id.clone();
    let _ok = node
        .sys_write(&SystemCmd::CreateApiKey {
            hash,
            record,
            consent: c.consent,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?;
    Ok(Json(json!({ "keyId": key_id, "key": key })))
}

async fn apikey_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let keys: Vec<ApiKeyRecord> = node.sys(|s| {
        s.api_keys
            .values()
            .filter(|k| k.account == account)
            .cloned()
            .collect()
    });
    Ok(Json(json!({ "keys": keys })))
}

/// Revoke an API key: the account's, approved with a passkey (412 first;
/// the retry's JSON body carries `confirm`).
async fn apikey_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    // Asked before the passkey: a key the account does not have is not one
    // to approve revoking.
    let theirs = node.sys(|s| {
        s.api_keys
            .values()
            .any(|k| k.key_id == key_id && k.account == account)
    });
    if !theirs {
        return Err(ApiError::not_found("no such key"));
    }
    let consent = consent(
        &node,
        &p,
        &format!("account:{account}"),
        "__revoke_api_key",
        &json!({ "keyId": key_id }),
        body.and_then(|Json(b)| b.confirm),
    )
    .await?;
    let _ok = node
        .sys_write(&SystemCmd::RevokeApiKey {
            key_id,
            actor: account,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub(super) struct AliasBody {
    app: String,
}

/// Name an app for this account (an alias).
async fn alias_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
    Json(body): Json<AliasBody>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let app = node.canonical_app_id(&body.app);
    let rec = node.app(&app).await?;
    p.within(&node, &rec)?;
    if !reaches(&node, &rec, Some(&account)) {
        return Err(ApiError::not_found("no such app"));
    }
    let _ok = node
        .sys_write(&SystemCmd::SetAlias {
            account,
            alias,
            app: Some(app),
        })
        .await?;
    Ok(Json(json!({})))
}

async fn alias_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetAlias {
            account,
            alias,
            app: None,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn alias_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let app = node
        .sys(|s| s.accounts.get(&account)?.aliases.get(&alias).cloned())
        .ok_or_else(|| ApiError::not_found("no such alias"))?;
    Ok(Json(json!({ "alias": alias, "appId": app })))
}

async fn alias_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let aliases = node.sys(|s| {
        s.accounts
            .get(&account)
            .map(|a| a.aliases.clone())
            .unwrap_or_default()
    });
    Ok(Json(json!({ "aliases": aliases })))
}

#[derive(Deserialize)]
pub(super) struct PasskeyLabelBody {
    label: String,
}

/// Name one of the caller's passkeys (a device label).
async fn passkey_label(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(cred): Path<String>,
    Json(body): Json<PasskeyLabelBody>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetPasskeyLabel {
            account,
            credential: cred,
            label: body.label,
        })
        .await?;
    Ok(Json(json!({})))
}

/// What the caller's account uses, against the cluster's quotas, and what today's
/// runs of the apps it is the developer of used (Q20, D76, D78): fuel and writes,
/// summed over the component apps billed to `account:{id}` as their developer
/// (`usage::developer`), on the UTC day of the node's app time. An organization's
/// are the organization's (`GET /orgs/{org}/usage`). Sums of 64-bit counters are
/// decimal strings (D20): `bundleBytes`, `developerFuelToday`, `developerWritesToday`.
async fn usage(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let today = node
        .developer_usage_today(&crate::usage::account_payer(&account))
        .await;
    let mut out = node.sys(|s| {
        json!({
            "apps": s.apps.values().filter(|a| a.owner == account).count(),
            "bundleBytes": s
                .bundles
                .values()
                .filter(|b| b.uploaded_by == account)
                .map(|b| b.size)
                .sum::<u64>()
                .to_string(),
            "apiKeys": s.api_keys.values().filter(|k| k.account == account).count(),
            "orgsOwned": s
                .orgs
                .values()
                .filter(|o| o.role(&account) == Some(OrgRole::Owner))
                .count(),
            "fileBytes": s
                .orgs
                .values()
                .filter(|o| o.role(&account).is_some())
                .map(|o| o.org_id.clone())
                .chain([format!("account:{account}")])
                .map(|payer| {
                    let bytes = s.file_bytes(&payer);
                    (payer, bytes)
                })
                .collect::<std::collections::BTreeMap<_, _>>(),
            "quotas": s.quotas,
        })
    });
    out["developerFuelToday"] = json!(today.fuel.to_string());
    out["developerWritesToday"] = json!(today.writes.to_string());
    Ok(Json(out))
}

/// The signed-in account's own apps (outside any organization), whose
/// release approvals and upgrades are its own: as [`org_apps`].
async fn account_apps(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let limit = q.limit()?;
    Ok(Json(
        scope_report(&node, &p, &account, None, &q, limit).await,
    ))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/auth/passkey/register/begin", post(register_begin))
        .route("/auth/passkey/register/finish", post(register_finish))
        .route("/auth/passkey/login/begin", post(login_begin))
        .route("/auth/passkey/login/finish", post(login_finish))
        .route("/auth/passkeys/add/begin", post(passkey_add_begin))
        .route("/auth/passkeys/add/finish", post(passkey_add_finish))
        .route(
            "/auth/passkeys/{cred}",
            post(passkey_label).delete(passkey_remove),
        )
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/logout-all", post(logout_all))
        .route("/auth/sessions", get(sessions_list))
        .route("/auth/sessions/{session}", delete(session_revoke))
        .route("/auth/activity", get(activity_list))
        .route("/auth/me", get(me))
        .route("/auth/apps", get(account_apps))
        .route("/auth/usage", get(usage))
        .route("/auth/aliases", get(alias_list))
        .route(
            "/auth/aliases/{alias}",
            get(alias_get).put(alias_set).delete(alias_delete),
        )
        .route("/auth/apikeys", post(apikey_create).get(apikey_list))
        .route("/auth/apikeys/{key}", delete(apikey_revoke))
}
