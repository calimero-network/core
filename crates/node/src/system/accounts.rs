//! System commands for accounts, passkeys, sessions and API keys.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`]. Each arm of either
//! is one call to a function of its own, in the order of the arms, and each
//! refusal of this area's own rules is a named [`Rejection`] below.

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// What `RevokeAllSessions` answers: how many sessions ended.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionsRevoked {
    pub revoked: usize,
}

// This area's refusals. Each is `400 REJECTED`, the kind the bare message it
// names was answered with; the message is what a client reads.

/// An exchange of a refresh token the cluster does not hold.
const UNKNOWN_REFRESH_TOKEN: Rejection =
    Rejection::new(RejectionKind::Rejected, "unknown refresh token");
/// An exchange of a refresh token recorded without a key.
const REFRESH_WITHOUT_KEY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this refresh token has no key; sign in again",
);
/// An exchange whose new token continues another session than the old one's.
const OTHER_SESSION: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the exchange is for another session",
);
/// A counter for a passkey the cluster does not hold.
const UNKNOWN_CREDENTIAL: Rejection = Rejection::new(RejectionKind::Rejected, "unknown credential");
/// A counter that carries no assertion.
const NO_ASSERTION: Rejection =
    Rejection::new(RejectionKind::Rejected, "the counter carries no assertion");
/// A counter whose assertion does not verify.
const ASSERTION_FAILS: Rejection =
    Rejection::new(RejectionKind::Rejected, "the assertion does not verify");
/// A counter other than the one its assertion carries.
const OTHER_COUNTER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the assertion carries another counter",
);
/// A sign-up before genesis.
const CLUSTER_STARTING: Rejection =
    Rejection::new(RejectionKind::Rejected, "the cluster is starting");
/// A sign-up for an account id that exists already.
const ACCOUNT_EXISTS: Rejection = Rejection::new(RejectionKind::Rejected, "account exists");
/// A passkey past an account's ten.
const TOO_MANY_PASSKEYS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an account holds at most 10 passkeys",
);
/// Removing an account's only passkey.
const LAST_PASSKEY: Rejection =
    Rejection::new(RejectionKind::Rejected, "cannot remove the last passkey");
/// A passkey the account does not hold.
const NO_SUCH_PASSKEY: Rejection = Rejection::new(RejectionKind::Rejected, "no such passkey");
/// A counter that does not move forward.
const STALE_COUNTER: Rejection = Rejection::new(RejectionKind::Rejected, "stale signature counter");
/// A revocation of an API key the account does not hold.
const NO_SUCH_KEY: Rejection = Rejection::new(RejectionKind::Rejected, "no such key");
/// An exchange of a token past its expiry, or of another account's.
const REFRESH_EXPIRED: Rejection = Rejection::new(RejectionKind::Rejected, "refresh token expired");
/// An exchange in a revoked session.
const SESSION_REVOKED: Rejection = Rejection::new(RejectionKind::Rejected, "session revoked");
/// A passkey label past [`MAX_LABEL_CHARS`].
const LABEL_TOO_LONG: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a passkey label is at most 64 characters",
);

/// Most passkeys an account holds.
const MAX_PASSKEYS: usize = 10;

/// Judge the proof a command of this area carries ([`super::authorize`]).
///
/// Credentials carry the account's own passkey consent; a new passkey is
/// approved by one the account already has (`consent_of` reads the passkeys
/// as they stand, without the new one).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    match cmd {
        SystemCmd::AddPasskey {
            account,
            credential,
            consent,
            ..
        } => account_consent(
            proofs,
            account,
            consent,
            "__add_passkey",
            &json!({
                "credentialId": credential.id,
                "publicKey": credential.public_key,
            }),
        ),
        SystemCmd::RemovePasskey {
            account,
            credential_id,
            consent,
            ..
        } => account_consent(
            proofs,
            account,
            consent,
            "__remove_passkey",
            &json!({ "credentialId": credential_id }),
        ),
        SystemCmd::CreateApiKey {
            hash,
            record,
            consent,
            then,
            ..
        } => account_consent(
            proofs,
            &record.account,
            consent,
            "__create_api_key",
            &api_key_args(hash, record, then),
        ),
        SystemCmd::RevokeApiKey {
            key_id,
            actor,
            consent,
        } => account_consent(
            proofs,
            actor,
            consent,
            "__revoke_api_key",
            &json!({ "keyId": key_id }),
        ),
        SystemCmd::CreateAccount {
            account,
            invite,
            refresh,
            consent,
            ..
        } => authorize_sign_up(proofs, account, invite.as_ref(), refresh.as_ref(), consent),
        SystemCmd::CreateRefresh {
            hash,
            record,
            consent,
        } => authorize_sign_in(proofs, hash, record, consent),
        SystemCmd::UseRefresh {
            old_hash,
            new_hash,
            record,
            now,
            signature,
        } => authorize_exchange(proofs, old_hash, new_hash, record, *now, signature),
        SystemCmd::SetSignCount {
            credential_id,
            count,
            assertion,
        } => authorize_counter(proofs, credential_id, *count, assertion),
        _ => Ok(None),
    }
}

/// `account`'s passkey consent, not used before, stating `method` on the
/// account (`account:<account>`) with exactly the arguments `stated`.
fn account_consent(
    proofs: &Proofs<'_>,
    account: &str,
    consent: &Value,
    method: &str,
    stated: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
    proofs.states(&statement, &format!("account:{account}"), method, &|args| {
        args == stated
    })?;
    Ok(Some(used))
}

/// A sign-up: the new passkey's assertion over the whole account and the
/// session it opens, so a node that saw someone's new credential cannot
/// register it first under an account and a release of its own.
fn authorize_sign_up(
    proofs: &Proofs<'_>,
    account: &Account,
    invite: Option<&SignUpRedemption>,
    refresh: Option<&RefreshGrant>,
    consent: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) =
        proofs.fresh(crate::proof::verify_sign_in(consent, &account.passkeys)?)?;
    if let Some(redemption) = invite {
        check_sign_up_invite(proofs.s, account, redemption)?;
    }
    if refresh.is_some_and(|grant| grant.record.account != account.account_id) {
        return Err(CONSENT_FOR_ANOTHER_CHANGE);
    }
    let stated = account_args(account, refresh);
    proofs.states(
        &statement,
        &format!("account:{}", account.account_id),
        "__create_account",
        &|args| *args == stated,
    )?;
    Ok(Some(used))
}

/// An invite at sign-up: the token's key signs it, as a redemption of the new
/// account, so only the node the person gave it to can. Its mark is spent
/// where the account applies (`run_authorized`).
fn check_sign_up_invite(
    state: &SystemState,
    account: &Account,
    redemption: &SignUpRedemption,
) -> Result<(), Rejection> {
    if !state.invites.contains_key(&redemption.invite_key)
        && !state.sign_up_invites.contains_key(&redemption.invite_key)
    {
        return Err(Rejection::sign_up(
            "this invite was used up, revoked or never existed",
        ));
    }
    let redeemed = crate::proof::verify_invite_redeem(
        &redemption.invite_key,
        &account.account_id,
        redemption.now,
        &redemption.signature,
    )?;
    if state.used_proofs.contains(&redeemed) {
        return Err(REDEMPTION_USED);
    }
    Ok(())
}

/// A sign-in: the assertion it took, by the passkey the session names, over
/// a challenge that states this refresh token.
fn authorize_sign_in(
    proofs: &Proofs<'_>,
    hash: &str,
    record: &RefreshRecord,
    consent: &Value,
) -> Result<Option<String>, Rejection> {
    let passkey: Vec<Credential> = proofs
        .s
        .accounts
        .get(&record.account)
        .ok_or(UNKNOWN_ACCOUNT)?
        .passkeys
        .iter()
        .filter(|credential| record.passkey.as_deref() == Some(credential.id.as_str()))
        .cloned()
        .collect();
    let (statement, used) = proofs.fresh(crate::proof::verify_sign_in(consent, &passkey)?)?;
    let stated = sign_in_args(hash, record);
    proofs.states(&statement, "session", "__sign_in", &|args| *args == stated)?;
    Ok(Some(used))
}

/// An exchange: the old token's key, over the new token and the session it
/// continues, which stays the old one's.
fn authorize_exchange(
    proofs: &Proofs<'_>,
    old_hash: &str,
    new_hash: &str,
    record: &RefreshRecord,
    now: u64,
    signature: &str,
) -> Result<Option<String>, Rejection> {
    let old = proofs
        .s
        .refresh
        .get(old_hash)
        .ok_or(UNKNOWN_REFRESH_TOKEN)?;
    if old.key.is_empty() {
        return Err(REFRESH_WITHOUT_KEY);
    }
    if old.account != record.account
        || old.session != record.session
        || old.passkey != record.passkey
        || old.app != record.app
        || old.origin != record.origin
        // A session bound to a holder key stays bound to it: no node drops the binding
        // (or binds an unbound one) in an exchange.
        || old.holder != record.holder
    {
        return Err(OTHER_SESSION);
    }
    let message = crate::proof::refresh_use_message(
        old_hash,
        new_hash,
        &record.key,
        &record.session,
        record.expires_at,
        now,
    );
    crate::proof::verify_refresh_use(&old.key, &message, signature)?;
    Ok(None)
}

/// A counter: an assertion of that passkey carrying it. Only the
/// authenticator signs a counter, so none is set past what it reached.
fn authorize_counter(
    proofs: &Proofs<'_>,
    credential_id: &str,
    count: u32,
    assertion: &Value,
) -> Result<Option<String>, Rejection> {
    let (_, credential) = proofs
        .s
        .account_of_credential(credential_id)
        .ok_or(UNKNOWN_CREDENTIAL)?;
    let assertion: calimero_identity::Assertion =
        serde_json::from_value(assertion.clone()).map_err(|_| NO_ASSERTION)?;
    let carried = calimero_identity::assertion_counter(credential, &assertion)
        .map_err(|_| ASSERTION_FAILS)?;
    if carried != count {
        return Err(OTHER_COUNTER);
    }
    Ok(None)
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let state = &mut m.state;
    match cmd {
        SystemCmd::CreateAccount {
            account,
            invite,
            refresh,
            at,
            personal_org,
            ..
        } => create_account(state, account, invite, refresh, at, personal_org),
        SystemCmd::AddPasskey {
            account,
            credential,
            at,
            ..
        } => add_passkey(state, account, credential, at),
        SystemCmd::RemovePasskey {
            account,
            credential_id,
            now,
            ..
        } => remove_passkey(state, &account, &credential_id, now),
        SystemCmd::RevokeAllSessions { actor, now } => {
            let revoked = state.revoke_account_sessions(&actor, now);
            Ok(json!(SessionsRevoked { revoked }))
        }
        SystemCmd::RefreshReused { hash, now } => {
            let reused = state
                .rotated_refresh
                .get(&hash)
                .cloned()
                .ok_or(UNKNOWN_REFRESH_TOKEN)?;
            state.revoke_session(&reused.session, now);
            Ok(json!({ "revoked": reused.session }))
        }
        SystemCmd::SetSignCount {
            credential_id,
            count,
            ..
        } => set_sign_count(state, &credential_id, count),
        SystemCmd::CreateApiKey {
            hash,
            record,
            owner_in_org,
            owner_in_team,
            owner_approves,
            ..
        } => create_api_key(
            state,
            hash,
            record,
            owner_in_org,
            owner_in_team,
            owner_approves,
        ),
        SystemCmd::RevokeApiKey { key_id, actor, .. } => {
            let before = state.api_keys.len();
            state
                .api_keys
                .retain(|_, key| !(key.key_id == key_id && key.account == actor));
            if state.api_keys.len() == before {
                return Err(NO_SUCH_KEY);
            }
            Ok(Value::Null)
        }
        SystemCmd::CreateRefresh { hash, record, .. } => create_refresh(state, hash, record),
        SystemCmd::UseRefresh {
            old_hash,
            new_hash,
            record,
            now,
            ..
        } => use_refresh(state, old_hash, new_hash, record, now),
        SystemCmd::RevokeSession {
            session,
            actor,
            now,
        } => {
            // Only the session's own account revokes it.
            let theirs = !state
                .refresh
                .values()
                .any(|token| token.session == session && token.account != actor);
            if theirs {
                state.revoke_session(&session, now);
            }
            state.prune_sessions(now);
            Ok(Value::Null)
        }
        SystemCmd::SetPasskeyLabel {
            account,
            credential,
            label,
        } => set_passkey_label(state, &account, credential, label),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached accounts::apply"),
    }
}

/// Create an account: the rules it must meet, judged before anything
/// changes, then its passkeys, first session, personal organization and
/// invite redemption.
fn create_account(
    state: &mut SystemState,
    mut account: Account,
    invite: Option<SignUpRedemption>,
    mut refresh: Option<RefreshGrant>,
    at: u64,
    personal_org: Option<PersonalOrgSeed>,
) -> Result<Value, Rejection> {
    // The first account waits for genesis: the rules it joins under are
    // fixed before anyone relies on them.
    if state.governance.is_none() && state.accounts.is_empty() {
        return Err(CLUSTER_STARTING);
    }
    if state.accounts.contains_key(&account.account_id) {
        return Err(ACCOUNT_EXISTS);
    }
    // Judged before anything changes: a refusal leaves no trace.
    let admission = state.admits(invite.as_ref())?;
    // Once organizations are ordinary a sign-up makes none, whatever the entry
    // carries (one proposed below that version, applied after it).
    let personal_org = personal_org.filter(|_| !state.ordinary_orgs());
    if state.accounts.len() >= state.quotas.max_accounts as usize {
        return Err(Rejection::quota("accounts"));
    }
    check_new_sign_up(state, &account, refresh.as_ref(), personal_org.as_ref())?;
    for credential in &account.passkeys {
        let _prev = state
            .credentials
            .insert(credential.id.clone(), account.account_id.clone());
    }
    stamp_sign_up_times(&mut account, refresh.as_mut(), at);
    let signed_in = refresh.as_ref().and_then(|grant| sign_in_of(&grant.record));
    if let Some(RefreshGrant { hash, record }) = refresh {
        let _prev = state.refresh.insert(hash, record);
    }
    let id = account.account_id.clone();
    let _prev = state.accounts.insert(id.clone(), account);
    if let Some(seed) = personal_org {
        let org = super::orgs::new_org(
            seed.org_id.clone(),
            PERSONAL_ORG_NAME.to_owned(),
            id.clone(),
            seed.release,
            true,
        );
        let _prev = state.orgs.insert(seed.org_id, org);
    }
    if let Some(entry) = signed_in {
        state.record_activity(&id, entry);
    }
    let org = redeem_sign_up_invite(state, &id, admission, invite)?;
    Ok(match org {
        Some(org) => json!({ "accountId": id, "org": org }),
        None => json!({ "accountId": id }),
    })
}

/// What a sign-up adds is new: its passkeys, its first refresh token and its
/// personal organization.
fn check_new_sign_up(
    state: &SystemState,
    account: &Account,
    refresh: Option<&RefreshGrant>,
    personal_org: Option<&PersonalOrgSeed>,
) -> Result<(), Rejection> {
    for credential in &account.passkeys {
        if state.credentials.contains_key(&credential.id) {
            return Err(CREDENTIAL_REGISTERED);
        }
    }
    if refresh.is_some_and(|grant| state.refresh_known(&grant.hash)) {
        return Err(REFRESH_ISSUED);
    }
    if personal_org.is_some_and(|seed| state.orgs.contains_key(&seed.org_id)) {
        return Err(ORG_EXISTS);
    }
    Ok(())
}

/// The sign-up adds its passkey and signs in with it, at the time the entry
/// carries (none: an earlier release's).
fn stamp_sign_up_times(account: &mut Account, refresh: Option<&mut RefreshGrant>, at: u64) {
    account.passkey_times = BTreeMap::new();
    if at == 0 {
        return;
    }
    let signs_in = refresh.is_some();
    for credential in &account.passkeys {
        let mut times = crate::types::PasskeyTimes {
            added_at: Some(at),
            last_sign_in_at: None,
        };
        if signs_in {
            times.signed_in(at);
        }
        let _prev = account.passkey_times.insert(credential.id.clone(), times);
    }
    if let Some(grant) = refresh {
        grant.record.opened_at = at;
    }
}

/// Spend the invite a sign-up presented: its proof, then an organization's
/// invite (answering the organization) or a sign-up invite.
fn redeem_sign_up_invite(
    state: &mut SystemState,
    account_id: &str,
    admission: Admission,
    invite: Option<SignUpRedemption>,
) -> Result<Option<OrgId>, Rejection> {
    if let Some(redemption) = &invite {
        let _new = state.used_proofs.insert(crate::proof::invite_redeem_used(
            &redemption.invite_key,
            account_id,
            redemption.now,
        ));
    }
    match (admission, invite) {
        // A new account is never a member yet: every use is spent, as before
        // the free redemption existed.
        (Admission::OrgInvite, Some(redemption)) => Ok(Some(state.redeem_org_invite(
            &redemption.invite_key,
            account_id,
            redemption.now,
            false,
            redemption.clamp_grants,
        )?)),
        (Admission::SignUpInvite, Some(redemption)) => {
            state.spend_sign_up_invite(&redemption.invite_key);
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// Add a passkey not registered anywhere, within the account's ten.
fn add_passkey(
    state: &mut SystemState,
    account: AccountId,
    credential: Credential,
    at: u64,
) -> Result<Value, Rejection> {
    if state.credentials.contains_key(&credential.id) {
        return Err(CREDENTIAL_REGISTERED);
    }
    let record = state.accounts.get_mut(&account).ok_or(UNKNOWN_ACCOUNT)?;
    if record.passkeys.len() >= MAX_PASSKEYS {
        return Err(TOO_MANY_PASSKEYS);
    }
    if at > 0 {
        let _prev = record.passkey_times.insert(
            credential.id.clone(),
            crate::types::PasskeyTimes {
                added_at: Some(at),
                last_sign_in_at: None,
            },
        );
    }
    let _prev = state.credentials.insert(credential.id.clone(), account);
    record.passkeys.push(credential);
    Ok(Value::Null)
}

/// Remove a passkey (never the last) with every session it opened: a removed
/// device keeps no session.
fn remove_passkey(
    state: &mut SystemState,
    account: &str,
    credential_id: &str,
    now: u64,
) -> Result<Value, Rejection> {
    let record = state.accounts.get_mut(account).ok_or(UNKNOWN_ACCOUNT)?;
    if record.passkeys.len() <= 1 {
        return Err(LAST_PASSKEY);
    }
    if !record
        .passkeys
        .iter()
        .any(|credential| credential.id == credential_id)
    {
        return Err(NO_SUCH_PASSKEY);
    }
    record
        .passkeys
        .retain(|credential| credential.id != credential_id);
    let _gone = record.passkey_times.remove(credential_id);
    let _prev = state.credentials.remove(credential_id);
    let sessions: BTreeSet<String> = state
        .refresh
        .values()
        .filter(|token| token.account == account && token.passkey.as_deref() == Some(credential_id))
        .map(|token| token.session.clone())
        .collect();
    for session in sessions {
        state.revoke_session(&session, now);
    }
    Ok(Value::Null)
}

/// Record a passkey's signature counter. Counters only move forward,
/// whatever order logins committed in; a passkey that does not count (0) has
/// nothing to record.
fn set_sign_count(
    state: &mut SystemState,
    credential_id: &str,
    count: u32,
) -> Result<Value, Rejection> {
    let account = state
        .credentials
        .get(credential_id)
        .cloned()
        .ok_or(UNKNOWN_CREDENTIAL)?;
    let record = state.accounts.get_mut(&account).ok_or(UNKNOWN_ACCOUNT)?;
    let credential = record
        .passkeys
        .iter_mut()
        .find(|credential| credential.id == credential_id)
        .ok_or(UNKNOWN_CREDENTIAL)?;
    if count <= credential.sign_count {
        return Err(STALE_COUNTER);
    }
    credential.sign_count = count;
    Ok(Value::Null)
}

/// Create an API key within the account's room; an upgrade key also fits
/// the upgrade rules its entry was judged by.
fn create_api_key(
    state: &mut SystemState,
    hash: String,
    record: ApiKeyRecord,
    owner_in_org: bool,
    owner_in_team: bool,
    owner_approves: bool,
) -> Result<Value, Rejection> {
    state.api_key_room(&record.account)?;
    if let Some(scope) = &record.upgrades {
        state.upgrade_key_fits_as_logged(
            &record,
            scope,
            OwnerRule::logged(owner_in_org, owner_in_team),
            state.owner_approves(owner_approves),
        )?;
    }
    let id = record.key_id.clone();
    let _prev = state.api_keys.insert(hash, record);
    Ok(json!({ "keyId": id }))
}

/// Record a sign-in's refresh token. Its passkey's last sign-in is when the
/// session opened (unknown in an earlier release's entry).
fn create_refresh(
    state: &mut SystemState,
    hash: String,
    record: RefreshRecord,
) -> Result<Value, Rejection> {
    if state.refresh_known(&hash) {
        return Err(REFRESH_ISSUED);
    }
    let mut signed_in = false;
    if let (Some(passkey), Some(account), true) = (
        &record.passkey,
        state.accounts.get_mut(&record.account),
        record.opened_at > 0,
    ) {
        if account
            .passkeys
            .iter()
            .any(|credential| &credential.id == passkey)
        {
            account
                .passkey_times
                .entry(passkey.clone())
                .or_default()
                .signed_in(record.opened_at);
            signed_in = true;
        }
    }
    if let Some(entry) = sign_in_of(&record).filter(|_| signed_in) {
        state.record_activity(&record.account, entry);
    }
    let _prev = state.refresh.insert(hash, record);
    Ok(Value::Null)
}

/// Exchange a refresh token for a new one in the same session: an unexpired
/// token of the same account, in a session not revoked.
fn use_refresh(
    state: &mut SystemState,
    old_hash: String,
    new_hash: String,
    mut record: RefreshRecord,
    now: u64,
) -> Result<Value, Rejection> {
    if state.refresh_known(&new_hash) {
        return Err(REFRESH_ISSUED);
    }
    let old = state
        .refresh
        .remove(&old_hash)
        .ok_or(UNKNOWN_REFRESH_TOKEN)?;
    if old.expires_at <= now || old.account != record.account {
        return Err(REFRESH_EXPIRED);
    }
    if state.revoked_sessions.contains(&old.session) {
        return Err(SESSION_REVOKED);
    }
    // The session opened when its first token did, whatever the exchange
    // states (an earlier release's states none).
    record.opened_at = old.opened_at;
    let _prev = state.rotated_refresh.insert(old_hash, old);
    let _prev = state.refresh.insert(new_hash, record);
    state.prune_sessions(now);
    Ok(Value::Null)
}

/// Set (or, empty, clear) a passkey's label, within [`MAX_LABEL_CHARS`].
fn set_passkey_label(
    state: &mut SystemState,
    account: &str,
    credential: String,
    label: String,
) -> Result<Value, Rejection> {
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(LABEL_TOO_LONG);
    }
    let record = state.accounts.get_mut(account).ok_or(UNKNOWN_ACCOUNT)?;
    if !record.passkeys.iter().any(|known| known.id == credential) {
        return Err(NO_SUCH_PASSKEY);
    }
    if label.is_empty() {
        let _gone = record.passkey_labels.remove(&credential);
    } else {
        let _prev = record.passkey_labels.insert(credential, label);
    }
    Ok(Value::Null)
}
