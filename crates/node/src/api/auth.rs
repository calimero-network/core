//! The API's routes for passkey sign-up and sign-in, sessions, the account, aliases and API keys.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// The most confirm calls one API key's consent approves with it (`then`).
const MAX_KEY_FOLLOW_UPS: usize = 4;
/// The most bytes those calls take as JSON, so the key's entry and each call's stay small.
const MAX_KEY_FOLLOW_UP_BYTES: usize = 64 * 1024;
/// What a follow-up's arguments write for the key being made: `key:<its id>`.
const KEY_PLACEHOLDER: &str = "key:$key";
/// The longest account name a sign-up takes, in characters: a sign-up's
/// ceremony holds it until its challenge expires, so it is bounded before
/// anything is held (`account_name_fits`).
pub const MAX_ACCOUNT_NAME_CHARS: usize = 64;
/// The largest request body a passkey ceremony's route (`/auth/passkey/*`)
/// takes: a name, an origin, an invite and a registration or assertion fit in
/// a few KiB.
pub const MAX_PASSKEY_CEREMONY_BYTES: usize = 64 * 1024;
/// The bytes of each follow-up's salt.
const FOLLOW_UP_SALT_BYTES: usize = 16;
/// How often a follow-up whose outcome is unknown (a 5xx) is sent again under its key.
const MAX_FOLLOW_UP_TRIES: usize = 3;
/// The wait before sending such a follow-up again.
const FOLLOW_UP_RETRY_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// A sign-in's session: its refresh token logged with the sign-in's
/// assertion, then the session token.
async fn new_session(
    node: &Node,
    account: &str,
    passkey: &str,
    from: Option<AppOrigin>,
    new: NewSession,
    consent: Value,
) -> ApiResult<Json<SessionAnswer>> {
    let origin = from.as_ref().and_then(AppOrigin::registered);
    let registration = from
        .as_ref()
        .map(|a| a.registration.clone())
        .unwrap_or_default();
    let app = from.map(|a| a.package);
    if let (Some(package), Some(origin)) = (&app, &origin) {
        info!(%package, %origin, "a session opened on a registered origin");
    }
    let token = session_token_for(
        node,
        account,
        passkey,
        app.clone(),
        origin.clone(),
        registration.clone(),
        &new,
    )?;
    let record = RefreshRecord {
        opened_at: now_secs(),
        ..new.record(account, passkey, app, origin, registration)
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

/// The holder key a ceremony binds its session to ([`RefreshRecord::holder`]), checked before
/// the ceremony spends anything: `400 BAD_HOLDER_KEY` for a malformed one, `409
/// FEATURE_NOT_ACTIVE` while a node of the cluster would drop it.
async fn holder_for(node: &Node, key: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(key) = key else {
        return Ok(None);
    };
    crate::proof::check_holder_key(&key)
        .map_err(|why| ApiError::bad_request("BAD_HOLDER_KEY", why))?;
    node.require_features(&[crate::system::cluster_version::Feature::SessionHolders])
        .await?;
    Ok(Some(key))
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
    /// The holder key to bind the session to ([`RefreshRecord::holder`]): a browser's
    /// non-extractable P-256 key, base64url of its raw public point. The challenge states
    /// it, so the passkey signs the binding.
    #[serde(default, rename = "holderKey")]
    holder_key: Option<String>,
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
/// SIGN_UP_REFUSED`), or this node's sign-up rate (`429 RATE_LIMITED`,
/// [`super::SignUpLimit`]), before a challenge is made or spent. A refusal is
/// judged again on fresh state, so a replica behind a policy change or a new
/// invite does not refuse what the log would take; `apply` decides in the end.
async fn refuse_sign_up(node: &Node, invite: Option<&str>) -> Result<(), ApiError> {
    super::sign_up::pace(node, false)?;
    let key = invite.map(crate::proof::invite_key);
    let now = now_secs();
    let judge = |s: &SystemState| s.admits_key(key.as_deref().map(|k| (k, now))).map(|_| ());
    if node.sys(judge).is_ok() {
        return Ok(());
    }
    let _caught_up = node.sys_fresh().await;
    node.sys(judge).map_err(crate::node::rejected)
}

/// Refuse an account name past [`MAX_ACCOUNT_NAME_CHARS`], `400
/// NAME_TOO_LONG`: judged before a ceremony holds it or the log is asked to
/// create the account.
fn account_name_fits(name: &str) -> Result<(), ApiError> {
    // A name past the limit in bytes is past it in characters too.
    if name.len() > 4 * MAX_ACCOUNT_NAME_CHARS || name.chars().count() > MAX_ACCOUNT_NAME_CHARS {
        return Err(ApiError::bad_request(
            "NAME_TOO_LONG",
            &format!("an account's name is at most {MAX_ACCOUNT_NAME_CHARS} characters"),
        ));
    }
    Ok(())
}

async fn register_begin(
    State(node): State<Arc<Node>>,
    sealed: Option<Extension<SealedSession>>,
    Json(body): Json<RegisterBegin>,
) -> ApiResult<Json<PasskeyChallenge>> {
    account_name_fits(&body.name)?;
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
        sealed.map(|Extension(s)| s),
    )?;
    Ok(Json(PasskeyChallenge {
        challenge_id: id,
        options: json!({
            "challenge": b64url(&challenge),
            "rp": { "id": rp_id, "name": "Calimero" },
            "user": { "id": b64url(&user_id), "name": shown, "displayName": shown },
            "pubKeyCredParams": [ { "type": "public-key", "alg": -7 }, { "type": "public-key", "alg": -8 } ],
            "timeout": 60000,
            "attestation": "none",
            "authenticatorSelection": { "residentKey": "required", "userVerification": "preferred" },
        }),
    }))
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
    /// The holder key to bind the new account's first session to, on the first finish
    /// ([`LoginBegin::holder_key`]); the new passkey signs it with the account.
    #[serde(default)]
    holder_key: Option<String>,
}

async fn register_finish(
    State(node): State<Arc<Node>>,
    sealed: Option<Extension<SealedSession>>,
    Json(body): Json<RegisterFinish>,
) -> ApiResult<Json<SessionAnswer>> {
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
    let holder = holder_for(&node, body.holder_key).await?;
    let Pending::Register {
        challenge, name, ..
    } = take_pending(&node, &body.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a registration challenge",
        ));
    };
    // Begun on a node that did not bound it, perhaps (`internal/challenge/take`).
    account_name_fits(&name)?;
    // Signing up on a package's frontend opens a session bound to it.
    let app = claimed_app(&node, &body.registration.client_data_json).await;
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
        passkey_labels: std::collections::BTreeMap::new(),
        aliases: std::collections::BTreeMap::new(),
        // Set at apply, from the sign-up's time (`CreateAccount::at`).
        passkey_times: std::collections::BTreeMap::new(),
    };
    // A registration carries no signature by the new key (attestation
    // `none`): the new passkey signs the account it creates, and the session
    // it opens, before the log takes it (412, then the retry).
    let session = NewSession::new(holder);
    // The new passkey belongs to the rp-id of the origin it was made on.
    let rp_id = app
        .as_ref()
        .and_then(|a| a.rp_id.clone())
        .unwrap_or_else(|| node.config.rp_id.clone());
    let origin = app.as_ref().and_then(AppOrigin::registered);
    let registration = app
        .as_ref()
        .map(|a| a.registration.clone())
        .unwrap_or_default();
    let app = app.map(|a| a.package);
    let grant = RefreshGrant {
        hash: session.hash(),
        record: session.record(
            &account.account_id,
            &cred.id,
            app.clone(),
            origin.clone(),
            registration.clone(),
        ),
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
            registration,
        },
        sealed.map(|Extension(s)| s),
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
async fn sign_up_confirmed(
    node: &Node,
    confirm: ConfirmBody,
    invite: Option<&str>,
) -> ApiResult<Json<SessionAnswer>> {
    let Pending::SignUp {
        challenge,
        statement,
        account,
        session,
        app,
        origin,
        registration,
        ..
    } = take_pending(node, &confirm.challenge_id).await?
    else {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "not a sign-up confirmation",
        ));
    };
    // The account the log is asked to create, whichever node held its ceremony.
    account_name_fits(&account.name)?;
    let cred = account
        .passkeys
        .first()
        .ok_or_else(|| ApiError::internal("a sign-up without its passkey"))?;
    // From the origin the sign-up came from: the package's (the registered
    // origin itself, for one), or the cluster's.
    let from = claimed_app(node, &confirm.assertion.client_data_json)
        .await
        .filter(|a| {
            app.as_deref() == Some(a.package.as_str())
                && a.registered() == origin
                && a.registration == registration
        });
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
        registration.clone(),
        &session,
    )?;
    let consent = crate::proof::Consent {
        statement,
        assertion: confirm.assertion,
        // A sign-in: its activity entry comes from its session's opening.
        at: 0,
    };
    let now = now_secs();
    // An earlier release would apply the grant unclamped, so the flag waits
    // for every node to read it.
    let clamp_grants = node
        .require_features(&[crate::system::cluster_version::Feature::InviteCapabilities])
        .await
        .is_ok();
    // Once organizations are ordinary a sign-up makes none: an account's organizations
    // are those it creates (with an instance, or `POST /orgs`) or is invited to. Before,
    // every account has its personal organization from the start, approving the release
    // it signed up through.
    let personal_org = node
        .require_features(&[crate::system::cluster_version::Feature::OrdinaryOrgs])
        .await
        .is_err()
        .then(|| crate::system::PersonalOrgSeed {
            org_id: typed_id::<16>(),
            release: Some(node.own_release()),
        });
    // Spent once the new passkey has signed the account, whatever the log then answers.
    super::sign_up::pace(node, true)?;
    let invite = invite.map(|token| crate::system::SignUpRedemption {
        invite_key: crate::proof::invite_key(token),
        now,
        signature: crate::proof::sign_invite_redeem(token, &account_id, now),
        clamp_grants,
    });
    let _ok = node
        .sys_write(&SystemCmd::CreateAccount {
            account: *account,
            invite,
            refresh: Some(RefreshGrant {
                hash: session.hash(),
                record: session.record(&account_id, &passkey, app, origin, registration),
            }),
            consent: super::consent_value(&consent)?,
            at: now,
            personal_org,
        })
        .await?;
    Ok(session_answer(&account_id, &token, &session))
}

async fn login_begin(
    State(node): State<Arc<Node>>,
    sealed: Option<Extension<SealedSession>>,
    body: Option<Json<LoginBegin>>,
) -> ApiResult<Json<PasskeyChallenge>> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let rp_id = begin_rp_id(&node, body.origin.as_deref()).await?;
    let holder = holder_for(&node, body.holder_key).await?;
    // The challenge states the session the sign-in opens, so the log takes
    // its refresh token only with this assertion (`CreateRefresh`).
    let session = NewSession::new(holder);
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
        sealed.map(|Extension(s)| s),
    )?;
    Ok(Json(PasskeyChallenge {
        challenge_id: id,
        options: json!({ "challenge": b64url(&challenge), "rpId": rp_id, "userVerification": "preferred", "timeout": 60000 }),
    }))
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

async fn login_finish(
    State(node): State<Arc<Node>>,
    Json(body): Json<AssertionBody>,
) -> ApiResult<Json<SessionAnswer>> {
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
        super::consent_value(&consent)?,
    )
    .await
}

async fn passkey_add_begin(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<PasskeyChallenge>> {
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
        None,
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
    Ok(Json(PasskeyChallenge {
        challenge_id: id,
        options: json!({
            "challenge": b64url(&challenge),
            "rp": { "id": node.config.rp_id, "name": "Calimero" },
            "user": { "id": b64url(&random::<16>()), "name": shown, "displayName": shown },
            "pubKeyCredParams": [ { "type": "public-key", "alg": -7 }, { "type": "public-key", "alg": -8 } ],
            "excludeCredentials": exclude,
            "attestation": "none",
        }),
    }))
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
) -> ApiResult<Json<PasskeyAdded>> {
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
    Ok(Json(PasskeyAdded { credential_id: id }))
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
    /// The holder's signature, required for a token bound to a holder key
    /// ([`RefreshRecord::holder`]) and ignored for any other.
    #[serde(default)]
    holder_proof: Option<crate::types::HolderProof>,
}

/// A token bound to a holder key is used only with that key's proof: `401 HOLDER_PROOF_REQUIRED`
/// without one, `401 BAD_HOLDER_PROOF` for one that does not verify. Judged before anything is
/// written, so a copy of the token taken off the device neither refreshes nor spends it.
fn check_holder(
    rec: &RefreshRecord,
    hash: &str,
    proof: Option<&crate::types::HolderProof>,
) -> Result<(), ApiError> {
    let Some(holder) = &rec.holder else {
        return Ok(());
    };
    let Some(proof) = proof else {
        return Err(ApiError::new(
            401,
            "HOLDER_PROOF_REQUIRED",
            "this refresh token is bound to a device key: send its holderProof",
        ));
    };
    crate::proof::verify_refresh_holder(holder, hash, proof.at, &proof.signature, now_secs())
        .map_err(|why| ApiError::new(401, "BAD_HOLDER_PROOF", why))
}

async fn refresh(
    State(node): State<Arc<Node>>,
    Json(body): Json<RefreshBody>,
) -> ApiResult<Json<SessionAnswer>> {
    let old_hash = b64url(&sha256(body.refresh_token.as_bytes()));
    let mut found = node.sys(|s| s.refresh.get(&old_hash).cloned());
    let mut caught_up = true;
    if found.is_none() {
        // Not here yet, or exchanged or revoked already: this replica may be
        // behind, so bring it up to what the cluster has committed (a read
        // index) before telling a token issued elsewhere from a reused one
        // (revoke the session) or an unknown one. The round returns as soon as
        // this replica has caught up, so a revoked token is refused at once
        // rather than after waiting `read_wait` for a record that will not come.
        caught_up = node.sys_fresh().await;
        found = node.sys(|s| s.refresh.get(&old_hash).cloned());
    }
    let Some(rec) = found else {
        // A token already exchanged, presented again: someone else holds a
        // copy, so the session it belongs to ends here. `RefreshReused` is
        // refused at apply for a token never exchanged, so a replica that
        // could not catch up lets the system log decide rather than answer
        // "unknown" to a thief from a stale view.
        let rotated = node.sys(|s| s.rotated_refresh.get(&old_hash).cloned());
        let known_here = rotated.is_some();
        // A token bound to a holder key ends its session only with that key's proof: a copy
        // of an old token taken off the device cannot sign its owner out. Judged here before
        // anything is written, and again at apply (`accounts::authorize_reuse`), which also
        // decides for a replica that could not catch up.
        if let Some(rotated) = &rotated {
            check_holder(rotated, &old_hash, body.holder_proof.as_ref())?;
        }
        if known_here || !caught_up {
            let revoked = node
                .sys_write(&SystemCmd::RefreshReused {
                    hash: old_hash,
                    now: now_secs(),
                    holder_proof: body.holder_proof.clone(),
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
    check_holder(&rec, &old_hash, body.holder_proof.as_ref())?;
    if let Some(origin) = &rec.origin {
        // Judged on the cluster's latest state: a removal made through
        // another node holds here at once.
        let _caught_up = node.sys_fresh().await;
        if !origin_holds(&node, rec.app.as_deref(), origin, &rec.registration) {
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
        expires_at: now_secs() + rec.ttl(),
        passkey: rec.passkey.clone(),
        app: rec.app.clone(),
        origin: rec.origin.clone(),
        registration: rec.registration.clone(),
        key: crate::proof::refresh_key(&new_refresh),
        opened_at: rec.opened_at,
        holder: rec.holder.clone(),
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
                // Checked against this token's live record above (`check_holder`); the log
                // judges it again against the exchanged one.
                holder_proof: body.holder_proof.clone(),
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
            registration: rec.registration,
            node: 0,
            key: String::new(),
        },
    )?;
    Ok(Json(SessionAnswer {
        account_id: rec.account,
        session_token: token,
        expires_at,
        refresh_token: new_refresh,
    }))
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
) -> Result<Option<Json<SessionAnswer>>, ApiError> {
    let now = now_secs();
    let issued = rec.issued_at();
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
            registration: rec.registration.clone(),
            node: 0,
            key: String::new(),
        },
    )?;
    Ok(Some(Json(SessionAnswer {
        account_id: rec.account.clone(),
        session_token: session,
        expires_at,
        refresh_token: token.to_owned(),
    })))
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
fn account_sessions(
    s: &SystemState,
    account: &str,
    current: Option<&str>,
    now: u64,
) -> Vec<SessionView> {
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
        .map(|r| SessionView {
            session: r.session.clone(),
            passkey: r.passkey.clone(),
            app: r.app.clone(),
            origin: r.origin.clone(),
            opened_at: (r.opened_at > 0).then_some(r.opened_at),
            last_active_at: r.issued_at(),
            current: current == Some(r.session.as_str()),
        })
        .collect()
}

/// `GET /auth/sessions`: the caller's own sessions ([`account_sessions`]).
/// The account holder's alone, on the cluster's own origin: never an API key
/// or a package-bound session, which would learn the other apps and origins
/// the person signs in to.
async fn sessions_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<SessionList>> {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let current = p.session.as_ref().map(|c| c.session.clone());
    let now = now_secs();
    let sessions = node.sys(|s| account_sessions(s, &account, current.as_deref(), now));
    Ok(Json(SessionList { sessions }))
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
) -> ApiResult<Json<ActivityList>> {
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
    Ok(Json(ActivityList { activity }))
}

/// One activity entry as `GET /auth/activity` lists it: every key present,
/// the ones its kind does not have `null`.
fn activity_entry(e: &crate::types::ActivityEntry) -> ActivityView {
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
    ActivityView {
        at: e.at,
        passkey: e.passkey.clone(),
        kind: kind.to_owned(),
        app: app.map(str::to_owned),
        origin: origin.map(str::to_owned),
        method: method.map(str::to_owned),
        subject: subject.map(str::to_owned),
    }
}

async fn me(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult<Json<AccountView>> {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    // An instance's own site learns the account's name and that instance's organization alone.
    let (acc, orgs, (recovery, pending), site) = node.sys(|s| {
        let acc = s.accounts.get(&account).cloned();
        let site = p
            .site_instance(s)
            .map(|app| s.apps.get(&app).map(|a| a.org.clone()));
        let recovery = (
            site.is_none() && s.recovery_keys.values().any(|a| *a == account),
            s.account_recoveries
                .get(&account)
                .filter(|_| site.is_none())
                .map(|p| p.view(&account)),
        );
        let orgs: Vec<AccountOrg> = s
            .orgs
            .values()
            .filter(|o| {
                site.as_ref()
                    .is_none_or(|org| org.as_ref() == Some(&o.org_id))
            })
            .filter_map(|o| {
                o.role(&account).map(|role| AccountOrg {
                    org_id: o.org_id.clone(),
                    name: o.name.clone(),
                    role,
                    personal: o.personal,
                })
            })
            .collect();
        (acc, orgs, recovery, site)
    });
    let acc = acc.ok_or_else(|| ApiError::not_found("account"))?;
    Ok(Json(AccountView {
        account_id: acc.account_id.clone(),
        name: acc.name.clone(),
        passkeys: acc
            .passkeys
            .iter()
            .filter(|_| site.is_none())
            .map(|c| {
                let times = acc.passkey_times.get(&c.id);
                AccountPasskey {
                    id: c.id.clone(),
                    sign_count: c.sign_count,
                    label: acc.passkey_labels.get(&c.id).cloned(),
                    // When it was added and last signed in (Unix seconds);
                    // `null`: before the node kept it.
                    added_at: times.and_then(|t| t.added_at),
                    last_sign_in_at: times.and_then(|t| t.last_sign_in_at),
                }
            })
            .collect(),
        orgs,
        via: if p.api_key.is_some() {
            "apiKey"
        } else {
            "session"
        }
        .to_owned(),
        // The passkey (device) this session was opened with.
        passkey_id: p.session.as_ref().and_then(|s| s.passkey.clone()),
        // The package a frontend's session is bound to, and the registered
        // origin that opened it.
        app: p.bound_app().map(str::to_owned),
        origin: p.session.as_ref().and_then(|s| s.origin.clone()),
        // Whether a recovery code is set (`/auth/recovery`); never the code.
        recovery_code: recovery,
        // A recovery under way (`/auth/recovery/start`), which a passkey of
        // this account cancels (`/auth/recovery/cancel`) before `readyAt`.
        recovery_pending: pending,
    }))
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
    /// Confirm calls approved with the key, in one passkey prompt.
    #[serde(default)]
    then: Vec<crate::types::KeyFollowUp>,
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
    owner_approves: bool,
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
    // A package with no release the account may use answers as a missing one.
    let signer = node
        .sys(|s| {
            s.package_used(&package, account, false)
                .map(|p| p.signer_id.clone())
        })
        .ok_or_else(|| ApiError::not_found("no such package"))?;
    if !node.sys(|s| s.may_upgrade(&app, account, owner_approves)) {
        return Err(ApiError::forbidden(if owner_approves {
            "an upgrade key is made by an owner or admin of the app's organization, who approve its upgrades"
        } else {
            "an upgrade key is made by someone who may upgrade the app"
        }));
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

/// `400 BAD_FOLLOW_UP`: a `then` the key cannot be made with.
fn bad_follow_up(message: &str) -> ApiError {
    ApiError::bad_request("BAD_FOLLOW_UP", message)
}

/// The follow-ups `body` asks for, each judged as `POST /apps/{app}/intents`
/// judges its caller, and refused unless it is a confirm method of an app in
/// the key's own `apps`. The app ids are canonical; the arguments as sent.
async fn follow_ups(
    node: &Node,
    headers: &HeaderMap,
    body: &ApiKeyBody,
) -> Result<Vec<crate::types::KeyFollowUp>, ApiError> {
    if body.then.is_empty() {
        return Ok(Vec::new());
    }
    if body.then.len() > MAX_KEY_FOLLOW_UPS {
        return Err(bad_follow_up(&format!(
            "a key takes at most {MAX_KEY_FOLLOW_UPS} follow-ups"
        )));
    }
    follow_ups_fit(&body.then)?;
    if body.upgrades.is_some() {
        return Err(bad_follow_up("an upgrade key takes no follow-ups"));
    }
    let Some(apps) = &body.apps else {
        return Err(bad_follow_up(
            "follow-ups need a key scoped to apps: name them in `apps`",
        ));
    };
    node.require_features(&[crate::system::cluster_version::Feature::KeyFollowUps])
        .await?;
    let mut out = Vec::with_capacity(body.then.len());
    for f in &body.then {
        super::calls::not_reserved(&f.method)?;
        let app_id = node.canonical_app_id(&f.app);
        if !apps.contains(&app_id) {
            return Err(bad_follow_up(&format!(
                "{} is not one of the key's apps",
                f.app
            )));
        }
        let app = node.app(&app_id).await?;
        let p = gate(node, headers, &app, &f.method, true).await?;
        if read_only_in(node, &app, p.caller.account(), p.sys_index).await {
            return Err(ApiError::forbidden("read-only in this team"));
        }
        app.require_app_component()?;
        let confirm = node
            .prepared_for(&node.running_bundle(&app))
            .await?
            .abi
            .methods
            .get(&f.method)
            .is_some_and(takes_follow_up);
        if !confirm {
            return Err(bad_follow_up(&format!(
                "{} is not a confirm method of {}",
                f.method, f.app
            )));
        }
        if !names_key(&f.args) {
            return Err(bad_follow_up(&format!(
                "a follow-up must name the new key: {} on {} has no `{KEY_PLACEHOLDER}`",
                f.method, f.app
            )));
        }
        out.push(crate::types::KeyFollowUp {
            app: app_id,
            ..f.clone()
        });
    }
    Ok(out)
}

/// Whether some string value of `args` is exactly [`KEY_PLACEHOLDER`].
fn names_key(args: &Value) -> bool {
    match args {
        Value::String(s) => s == KEY_PLACEHOLDER,
        Value::Array(items) => items.iter().any(names_key),
        Value::Object(fields) => fields.values().any(names_key),
        _ => false,
    }
}

/// `413 ARGS_TOO_LARGE` for follow-ups past [`MAX_KEY_FOLLOW_UP_BYTES`] as JSON.
fn follow_ups_fit(then: &[crate::types::KeyFollowUp]) -> Result<(), ApiError> {
    if serde_json::to_vec(then).map_or(usize::MAX, |b| b.len()) > MAX_KEY_FOLLOW_UP_BYTES {
        return Err(ApiError::new(
            413,
            "ARGS_TOO_LARGE",
            &format!("follow-ups are limited to {MAX_KEY_FOLLOW_UP_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Whether a follow-up may name `m`: a confirm method a client calls and that
/// applies in its own entry (an action answers before it runs).
fn takes_follow_up(m: &calimero_app_host::MethodAbi) -> bool {
    m.confirm
        && m.kind == calimero_app_host::MethodKind::Call
        && !m.compute_result
        && !m.action_only
}

/// What a follow-up's app entry logs as its consent: which approval it ran under,
/// its place and its salt (so the app's auditors can check its `argsHash`),
/// never the approval itself.
fn follow_up_consent(approval: &Value, index: usize, salt: &str) -> Result<Value, ApiError> {
    let used = serde_json::from_value::<crate::proof::Consent>(approval.clone())
        .ok()
        .and_then(|c| c.used().ok())
        .ok_or_else(|| ApiError::internal("the approval is not a consent"))?;
    Ok(json!({ "followUpOf": used, "index": index, "salt": salt }))
}

/// What a key's challenge keeps for its retry (`Pending::Confirm::carry`): the
/// key, and with follow-ups each one's salt. A plain key's is the key alone, as
/// before follow-ups.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum KeyCarry {
    Key(String),
    WithSalts { key: String, salts: Vec<String> },
}

/// A follow-up as it runs: the call, its salt, and what the consent states of it.
struct Approved {
    call: crate::types::KeyFollowUp,
    salt: String,
    stated: crate::types::SignedFollowUp,
}

/// `then` with the salts the challenge carries, one each; another count, or a
/// salt that is not [`FOLLOW_UP_SALT_BYTES`] of base64url, is a different approval.
fn approved(
    then: Vec<crate::types::KeyFollowUp>,
    salts: &[String],
) -> Result<Vec<Approved>, ApiError> {
    let wrong =
        || ApiError::bad_request("WRONG_CHALLENGE", "the approval was for a different call");
    if then.len() != salts.len() {
        return Err(wrong());
    }
    then.into_iter()
        .zip(salts)
        .map(|(call, salt)| {
            let bytes = calimero_identity::from_b64url(salt)
                .ok()
                .filter(|b| b.len() == FOLLOW_UP_SALT_BYTES)
                .ok_or_else(wrong)?;
            Ok(Approved {
                stated: call.signed(&bytes),
                salt: salt.clone(),
                call,
            })
        })
        .collect()
}

/// What the key's consent states of `approved`.
fn stated(approved: &[Approved]) -> Vec<crate::types::SignedFollowUp> {
    approved.iter().map(|a| a.stated.clone()).collect()
}

/// `follow_ups` as signed for the key `key_id`: every string value that is
/// exactly [`KEY_PLACEHOLDER`] becomes `key:<key_id>`.
fn for_key(
    follow_ups: &[crate::types::KeyFollowUp],
    key_id: &str,
) -> Vec<crate::types::KeyFollowUp> {
    fn substitute(v: &mut Value, principal: &str) {
        match v {
            Value::String(s) if s == KEY_PLACEHOLDER => principal.clone_into(s),
            Value::Array(items) => items.iter_mut().for_each(|i| substitute(i, principal)),
            Value::Object(fields) => fields.values_mut().for_each(|f| substitute(f, principal)),
            _ => {}
        }
    }
    let principal = format!("key:{key_id}");
    follow_ups
        .iter()
        .map(|f| {
            let mut f = f.clone();
            substitute(&mut f.args, &principal);
            f
        })
        .collect()
}

/// Whether a call that answered `e` may still have applied: a node that
/// failed or timed out, a leader race, a forward that got no answer.
fn outcome_unknown(e: &ApiError) -> bool {
    e.status >= 500
}

/// `422 FOLLOW_UP_FAILED`: follow-up `index` answered `e`, so no key was made;
/// the ones before it applied (`applied`), and it may have (`unknown`) when
/// its answer could not say.
pub(super) fn follow_up_failed(
    index: usize,
    f: &crate::types::KeyFollowUp,
    e: &ApiError,
) -> ApiError {
    let unknown: Vec<usize> = if outcome_unknown(e) {
        vec![index]
    } else {
        Vec::new()
    };
    ApiError::new(
        422,
        "FOLLOW_UP_FAILED",
        &format!(
            "follow-up {index} ({} on {}) failed, so the key was not created: {}",
            f.method, f.app, e.message
        ),
    )
    .with_details(json!({
        "index": index,
        "app": f.app,
        "method": f.method,
        "error": e,
        "applied": (0..index).collect::<Vec<_>>(),
        "unknown": unknown,
    }))
}

/// The idempotency key follow-up `index` of `key_id` runs under: its arguments'
/// hash in it, so a key another session took cannot stand for this call.
fn follow_up_key(key_id: &str, index: usize, f: &crate::types::SignedFollowUp) -> String {
    format!(
        "{}{key_id}:{index}:{}",
        crate::idempotency::FOLLOW_UP_KEY_PREFIX,
        f.args_hash
    )
}

/// What a key write that failed after `applied` follow-ups answers: whether the
/// write may still land (`keyWrite: unknown`, a 5xx) or was refused.
fn key_write_failed(e: ApiError, applied: usize) -> ApiError {
    if applied == 0 {
        return e;
    }
    let outcome = if outcome_unknown(&e) {
        "unknown"
    } else {
        "refused"
    };
    let e = super::orgs::with_detail(e, "applied", json!((0..applied).collect::<Vec<_>>()));
    super::orgs::with_detail(e, "keyWrite", json!(outcome))
}

/// What a key made with `follow_ups` answers when its task ended without an
/// answer: any of its follow-ups and the key itself may have applied.
fn interrupted(follow_ups: usize) -> ApiError {
    ApiError::internal(
        "making the key stopped before it answered: list the account's keys, revoke one you do not want, and check the apps",
    )
    .with_details(json!({
        "applied": [],
        "unknown": (0..follow_ups).collect::<Vec<_>>(),
        "keyWrite": "unknown",
    }))
}

/// Run the approved follow-ups in order as `p`, each under an idempotency key
/// of its own, so one sent again after an unknown outcome replays; stop at the
/// first that fails. Each answer carries the call's receipt, as `/intents` does.
async fn run_follow_ups(
    node: &Node,
    p: &Principal,
    then: &[Approved],
    key_id: &str,
    approval: &Value,
) -> Result<Vec<Value>, ApiError> {
    let mut results = Vec::with_capacity(then.len());
    for (index, a) in then.iter().enumerate() {
        let f = &a.call;
        let consent = follow_up_consent(approval, index, &a.salt)?;
        let key = follow_up_key(key_id, index, &a.stated);
        let mut tries = 1;
        let answer = loop {
            match super::calls::call_consented(node, p, f, key.clone(), consent.clone()).await {
                Err(e) if outcome_unknown(&e) && tries < MAX_FOLLOW_UP_TRIES => {
                    tries += 1;
                    tokio::time::sleep(FOLLOW_UP_RETRY_WAIT).await;
                }
                other => break other,
            }
        };
        let (log_index, result) = answer.map_err(|e| follow_up_failed(index, f, &e))?;
        let receipt = node.receipt(&f.app, log_index, &result);
        results.push(json!({ "index": index, "result": result, "receipt": receipt }));
    }
    Ok(results)
}

/// In `record.account`'s turn on this node: check its key quota again, run the
/// follow-ups, then write the key, so no other key of the account lands between.
async fn make_key(
    node: Arc<Node>,
    p: Principal,
    hash: String,
    record: ApiKeyRecord,
    consent: Value,
    owner_approves: bool,
    then: Vec<Approved>,
) -> Result<Vec<Value>, ApiError> {
    let _turn = node.api_key_turn(&record.account).await;
    // The last key this account made may have applied on the leader alone.
    if !node.sys_caught_up().await {
        return Err(ApiError::unavailable(
            "this node could not read the account's keys; the approval is spent: approve it again",
        ));
    }
    key_room(&node, &record.account).map_err(|e| ApiError {
        message: format!("{}; the approval is spent: approve it again", e.message),
        ..e
    })?;
    let follow_ups = stated(&then);
    let results = run_follow_ups(&node, &p, &then, &record.key_id, &consent).await?;
    let _ok = node
        .sys_write(&SystemCmd::CreateApiKey {
            hash,
            record,
            consent,
            owner_in_org: true,
            owner_in_team: true,
            owner_approves,
            then: follow_ups,
        })
        .await
        .map_err(|e| key_write_failed(e, then.len()))?;
    Ok(results)
}

/// `account`'s room for another key, judged as `CreateApiKey` judges it.
fn key_room(node: &Node, account: &str) -> Result<(), ApiError> {
    node.sys(|s| s.api_key_room(account))
        .map_err(crate::node::rejected)
}

/// Create an API key: the account's, approved with a passkey over the key's
/// record and hash (412 first). The first request makes the key, its id and
/// expiry and keeps them with the challenge; the retry logs what was
/// approved. A passkey session only: an API key cannot make another.
/// `then`: confirm calls the same passkey approves, run before the key is made.
async fn apikey_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<ApiKeyBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let follow_ups = follow_ups(&node, &headers, &body).await?;
    let (key, hash) = calimero_identity::new_api_key();
    // An upgrade key is made, and used, only by whom the rule in force lets upgrade
    // the app: from `Feature::OwnerApprovedUpgrades` on, an owner or admin of its
    // organization, judged again where it applies (`owner_approves`).
    let owner_approves = body.upgrades.is_some() && crate::api::owner_approves(&node).await;
    let upgrades = match &body.upgrades {
        Some(u) => Some(upgrade_scope(&node, &account, u, owner_approves).await?),
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
        node.sys(|s| s.upgrade_key_fits(&record, scope, owner_approves))
            .map_err(|e| ApiError::bad_request("BAD_UPGRADE_KEY", e))?;
    }
    let then = for_key(&follow_ups, &record.key_id);
    follow_ups_fit(&then)?;
    // Judged on both requests, so the retry runs no follow-up for a key
    // the log would refuse.
    key_room(&node, &account)?;
    let salts: Vec<String> = then
        .iter()
        .map(|_| b64url(&random::<FOLLOW_UP_SALT_BYTES>()))
        .collect();
    let shown = approved(then.clone(), &salts)?;
    let args = crate::system::api_key_args(&hash, &record, &stated(&shown));
    let carry = if salts.is_empty() {
        KeyCarry::Key(key)
    } else {
        KeyCarry::WithSalts { key, salts }
    };
    let c = consent_minted(
        &node,
        &p,
        &format!("account:{account}"),
        "__create_api_key",
        &args,
        serde_json::to_value(carry).ok(),
        body.confirm,
    )
    .await
    .map_err(|e| match e.code.as_str() {
        // The 412 shows the calls the passkey approves, with the key's id in them.
        "CONFIRM_REQUIRED" if !then.is_empty() => super::orgs::with_detail(e, "then", json!(then)),
        _ => e,
    })?;
    // The retry logs the key the approval names, made by the first request;
    // its scope must be the one this request asks for.
    let (key, salts) = match c.carry.clone().map(serde_json::from_value::<KeyCarry>) {
        Some(Ok(KeyCarry::Key(key))) => (key, Vec::new()),
        Some(Ok(KeyCarry::WithSalts { key, salts })) => (key, salts),
        _ => {
            return Err(ApiError::bad_request(
                "WRONG_CHALLENGE",
                "not an API key approval",
            ))
        }
    };
    let hash = calimero_identity::api_key_hash(&key);
    let signed = |f: &str| c.args.get(f).cloned().unwrap_or(Value::Null);
    let record = ApiKeyRecord {
        key_id: signed("keyId").as_str().unwrap_or_default().to_owned(),
        expires_at: signed("expiresAt").as_u64().unwrap_or_default(),
        upgrades: with_key(&key),
        ..record
    };
    let then = approved(for_key(&follow_ups, &record.key_id), &salts)?;
    if crate::system::api_key_args(&hash, &record, &stated(&then)) != c.args
        || record.expires_at > now_secs() + MAX_API_KEY_TTL
    {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "the approval was for a different call",
        ));
    }
    let key_id = record.key_id.clone();
    // The calls first: one that fails leaves no key, and those before it name
    // a key id that never exists. In a task of its own, so a caller that
    // disconnects does not stop it between a committed call and the key.
    let made = make_key(
        Arc::clone(&node),
        p,
        hash,
        record,
        c.consent,
        owner_approves,
        then,
    );
    let count = follow_ups.len();
    let results = tokio::spawn(made).await.map_err(|_| interrupted(count))??;
    Ok(Json(json!(ApiKeyMade {
        key_id,
        key,
        then: results
    })))
}

async fn apikey_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<ApiKeyList>> {
    let account = holder(&node, &headers).await?.account()?;
    let keys: Vec<ApiKeyRecord> = node.sys(|s| {
        s.api_keys
            .values()
            .filter(|k| k.account == account)
            .cloned()
            .collect()
    });
    Ok(Json(ApiKeyList { keys }))
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
    p.settings_write()?;
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
    let p = signed_in(&node, &headers).await?;
    p.beyond_instance(&node)?;
    p.settings_write()?;
    let account = p.account()?;
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
) -> ApiResult<Json<AliasView>> {
    let p = signed_in(&node, &headers).await?;
    p.beyond_instance(&node)?;
    let account = p.account()?;
    let app = node
        .sys(|s| s.accounts.get(&account)?.aliases.get(&alias).cloned())
        .ok_or_else(|| ApiError::not_found("no such alias"))?;
    Ok(Json(AliasView { alias, app_id: app }))
}

async fn alias_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<AliasList>> {
    let p = signed_in(&node, &headers).await?;
    p.beyond_instance(&node)?;
    let account = p.account()?;
    let aliases = node.sys(|s| {
        s.accounts
            .get(&account)
            .map(|a| a.aliases.clone())
            .unwrap_or_default()
    });
    Ok(Json(AliasList { aliases }))
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
/// runs of the apps it is the developer of used: fuel and writes,
/// summed over the component apps billed to `account:{id}` as their developer
/// (`usage::developer`), on the UTC day of the node's app time. An organization's
/// are the organization's (`GET /orgs/{org}/usage`). Sums of 64-bit counters are
/// decimal strings: `bundleBytes`, `developerFuelToday`, `developerWritesToday`.
async fn usage(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult<Json<AccountUsage>> {
    let p = signed_in(&node, &headers).await?;
    p.beyond_instance(&node)?;
    let account = p.account()?;
    let today = node
        .developer_usage_today(&crate::usage::account_payer(&account))
        .await;
    let out = node.sys(|s| AccountUsage {
        apps: s.apps.values().filter(|a| a.owner == account).count(),
        bundle_bytes: s
            .bundles
            .values()
            .filter(|b| b.uploaded_by == account)
            .map(|b| b.size)
            .sum::<u64>()
            .to_string(),
        api_keys: s.api_keys.values().filter(|k| k.account == account).count(),
        orgs_owned: s
            .orgs
            .values()
            .filter(|o| o.role(&account) == Some(OrgRole::Owner))
            .count(),
        file_bytes: s
            .orgs
            .values()
            .filter(|o| o.role(&account).is_some())
            .map(|o| o.org_id.clone())
            .chain([format!("account:{account}")])
            .map(|payer| {
                let bytes = s.file_bytes(&payer);
                (payer, bytes)
            })
            .collect(),
        quotas: s.quotas.clone(),
        developer_fuel_today: today.fuel.to_string(),
        developer_writes_today: today.writes.to_string(),
    });
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
    Ok(Json(json!(
        scope_report(&node, &p, &account, None, &q, limit).await
    )))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/auth/passkey/register/begin", post(register_begin))
        .route("/auth/passkey/register/finish", post(register_finish))
        .route("/auth/passkey/login/begin", post(login_begin))
        .route("/auth/passkey/login/finish", post(login_finish))
        // Anyone may begin these, so their bodies stay small (the router's
        // own limit, inside which this one applies, is the sealed envelope's).
        .route_layer(axum::extract::DefaultBodyLimit::max(
            MAX_PASSKEY_CEREMONY_BYTES,
        ))
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

#[cfg(test)]
mod tests {
    use calimero_app_host::{MethodAbi, MethodKind};

    use super::*;

    fn method(kind: MethodKind, confirm: bool) -> MethodAbi {
        MethodAbi {
            kind,
            confirm,
            public: false,
            every: None,
            allow_system: false,
            allow_apps: Vec::new(),
            action_only: false,
            compute_result: false,
        }
    }

    /// Only a confirm call that applies in its own entry: an action answers
    /// before it runs, so a later failure could not undo the key.
    #[test]
    fn a_follow_up_is_a_confirm_call_a_client_makes() {
        assert!(takes_follow_up(&method(MethodKind::Call, true)));
        assert!(!takes_follow_up(&method(MethodKind::Call, false)));
        assert!(!takes_follow_up(&method(MethodKind::Action, true)));
        assert!(!takes_follow_up(&method(MethodKind::View, true)));
        let action_only = MethodAbi {
            action_only: true,
            ..method(MethodKind::Call, true)
        };
        assert!(!takes_follow_up(&action_only));
        let compute_result = MethodAbi {
            compute_result: true,
            ..method(MethodKind::Call, true)
        };
        assert!(!takes_follow_up(&compute_result));
    }

    /// A call that failed or timed out on a node may have applied: it is sent
    /// again and, if still unknown, reported as such, never as not applied.
    #[test]
    fn a_follow_up_whose_answer_could_not_say_is_reported_unknown() {
        let f = crate::types::KeyFollowUp {
            app: "a".into(),
            method: "m".into(),
            args: Value::Null,
        };
        let refused = ApiError::new(422, "APP_ERROR", "the app refused the call");
        let failed = follow_up_failed(1, &f, &refused);
        assert_eq!(
            (&failed.details["applied"], &failed.details["unknown"]),
            (&json!([0]), &json!([]))
        );
        let unanswered = ApiError::unavailable("the call raced a leader change");
        let failed = follow_up_failed(1, &f, &unanswered);
        assert_eq!(
            (&failed.details["applied"], &failed.details["unknown"]),
            (&json!([0]), &json!([1]))
        );
        assert!(outcome_unknown(&crate::node::timeout_error()));
        assert!(!outcome_unknown(&ApiError::forbidden("read-only")));
    }

    /// Each follow-up takes one salt of exactly its size, else the approval is
    /// another's.
    #[test]
    fn a_follow_ups_salt_is_its_exact_size() {
        let call = || crate::types::KeyFollowUp {
            app: "a".into(),
            method: "m".into(),
            args: json!({ "p": "key:k" }),
        };
        let salt = |n: usize| b64url(&vec![7; n]);
        assert!(approved(vec![call()], &[salt(FOLLOW_UP_SALT_BYTES)]).is_ok());
        for wrong in [
            vec![salt(FOLLOW_UP_SALT_BYTES - 1)],
            vec![salt(FOLLOW_UP_SALT_BYTES + 1)],
            vec![String::new()],
            vec!["not base64url!".to_owned()],
            vec![],
        ] {
            let e = approved(vec![call()], &wrong).err().map(|e| e.code);
            assert_eq!(e.as_deref(), Some("WRONG_CHALLENGE"), "{wrong:?}");
        }
    }

    /// A key write that failed after follow-ups ran says which applied and
    /// whether the key may still land; a task that ended unanswered says
    /// nothing is known.
    #[test]
    fn a_key_write_after_follow_ups_says_what_is_known() {
        let stalled = follow_up_details(key_write_failed(
            ApiError::unavailable("the system group did not commit this write"),
            2,
        ));
        assert_eq!(stalled, (json!([0, 1]), Value::Null, json!("unknown")));
        let refused = follow_up_details(key_write_failed(
            ApiError::new(429, "QUOTA_EXCEEDED", "quota exceeded: API keys"),
            1,
        ));
        assert_eq!(refused, (json!([0]), Value::Null, json!("refused")));
        let plain = key_write_failed(ApiError::unavailable("x"), 0);
        assert!(plain.details.is_null());
        let gone = interrupted(2);
        assert_eq!(gone.message, interrupted(2).message);
        assert_eq!(
            follow_up_details(gone),
            (json!([]), json!([0, 1]), json!("unknown"))
        );
    }

    fn follow_up_details(e: ApiError) -> (Value, Value, Value) {
        (
            e.details["applied"].clone(),
            e.details["unknown"].clone(),
            e.details["keyWrite"].clone(),
        )
    }

    /// An app's log names the approval a follow-up ran under, never the
    /// assertion, the key's hash or the other calls.
    #[test]
    fn a_follow_ups_app_entry_logs_only_which_approval_it_ran_under() {
        let mut pk =
            calimero_identity::testing::SoftAuthenticator::new("localhost", "https://localhost");
        let statement = crate::proof::Statement::new(
            "account:a",
            "__create_api_key",
            &json!({ "hash": "h", "then": [{ "app": "x", "method": "m", "args": { "s": 1 } }] }),
        );
        let consent = crate::proof::Consent {
            assertion: pk.assert(&statement.challenge().unwrap()),
            statement,
            at: 1,
        };
        let logged =
            follow_up_consent(&serde_json::to_value(&consent).unwrap(), 2, "c2FsdA").unwrap();
        assert_eq!(
            logged,
            json!({ "followUpOf": consent.used().unwrap(), "index": 2, "salt": "c2FsdA" })
        );
    }
}

/// A WebAuthn ceremony begun: its challenge id, and the options to hand the browser as they
/// are (`navigator.credentials.create` or `.get`; WebAuthn's shape, not this API's).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PasskeyChallenge {
    pub(super) challenge_id: String,
    pub(super) options: Value,
}

/// A passkey added to the account.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PasskeyAdded {
    credential_id: String,
}

/// `GET /auth/sessions`: the account's live sessions, latest active first.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SessionList {
    sessions: Vec<SessionView>,
}

/// One live session: its passkey, the package and origin it is bound to, when it opened
/// (`null`: before the node kept it), when it was last active and whether it is the caller's.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionView {
    session: String,
    passkey: Option<String>,
    app: Option<String>,
    origin: Option<String>,
    opened_at: Option<u64>,
    last_active_at: u64,
    current: bool,
}

/// `GET /auth/activity`: what the account's passkeys did, newest first.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ActivityList {
    activity: Vec<ActivityView>,
}

/// One activity entry: a sign-in (its package and origin) or an approval (its method and
/// subject), every key present, those its kind does not have `null`.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ActivityView {
    at: u64,
    passkey: String,
    kind: String,
    app: Option<String>,
    origin: Option<String>,
    method: Option<String>,
    subject: Option<String>,
}

/// `GET /auth/me`: the caller's account, its passkeys and organizations, how the caller came
/// and its recovery state.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountView {
    account_id: String,
    name: String,
    passkeys: Vec<AccountPasskey>,
    orgs: Vec<AccountOrg>,
    via: String,
    passkey_id: Option<String>,
    app: Option<String>,
    origin: Option<String>,
    recovery_code: bool,
    recovery_pending: Option<crate::types::AccountRecoveryPending>,
}

/// One passkey of an [`AccountView`]: its label, and when it was added and last signed in
/// (`null`: before the node kept it).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountPasskey {
    id: String,
    sign_count: u32,
    label: Option<String>,
    added_at: Option<u64>,
    last_sign_in_at: Option<u64>,
}

/// One organization of an [`AccountView`]: the caller's role, and whether it is a personal one.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountOrg {
    org_id: String,
    name: String,
    role: OrgRole,
    personal: bool,
}

/// `GET /auth/usage`: what the account uses against the quotas, and its developer usage today
/// (64-bit sums as decimal strings).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountUsage {
    apps: usize,
    bundle_bytes: String,
    api_keys: usize,
    orgs_owned: usize,
    file_bytes: BTreeMap<String, u64>,
    quotas: crate::system::Quotas,
    developer_fuel_today: String,
    developer_writes_today: String,
}

/// `GET /auth/aliases`: the account's names for apps.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AliasList {
    aliases: BTreeMap<String, String>,
}

/// `GET /auth/aliases/{alias}`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AliasView {
    alias: String,
    app_id: String,
}

/// `POST /auth/apikeys`: the key, answered once, and what the calls made with it answered.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApiKeyMade {
    key_id: String,
    key: String,
    then: Vec<Value>,
}

/// `GET /auth/apikeys`: the account's keys, never a key itself.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ApiKeyList {
    keys: Vec<ApiKeyRecord>,
}
