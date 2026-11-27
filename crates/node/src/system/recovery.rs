//! System commands for account and organization recovery.
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
use crate::proof::verify_consent;

/// What an organization recovery's start answers: when it started and when it may finish
/// (Unix seconds).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryStarted {
    pub started_at: u64,
    pub ready_at: u64,
}

/// What an organization recovery's finish answers: the organization and the owners it replaced.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryFinished {
    pub org: OrgId,
    pub previous_owners: Vec<AccountId>,
}

// This area's refusals. Each is `400 REJECTED`, the kind the bare message it
// names was answered with; the message is what a client reads.

/// An account recovery's start whose code signature was spent before.
const RECOVERY_USED: Rejection =
    Rejection::new(RejectionKind::Rejected, "that recovery was used already");
/// An account recovery's finish whose session is not for the new passkey.
const SESSION_FOR_ANOTHER_PASSKEY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the session is for another passkey",
);
/// An organization recovery's start whose signature was spent before.
const RECOVERY_SIGNATURE_USED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "that recovery signature was used already",
);
/// A recovery code another account holds.
const CODE_IN_USE: Rejection =
    Rejection::new(RejectionKind::Rejected, "that recovery code is in use");
/// Removing a recovery code from an account that has none.
const NO_CODE_SET: Rejection = Rejection::new(RejectionKind::Rejected, "no recovery code is set");
/// An imported app's release that does not ship its bundle as this cluster holds the package.
const RELEASE_NOT_SHIPPED_HERE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an imported app's release does not ship its bundle on this cluster",
);
/// An organization recovery started for its owner.
const OWNS_ALREADY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "that account owns the organization already",
);
/// An import whose slice is not the holder's, or whose key is malformed.
const NOT_THE_HOLDERS_SLICE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the slice is not that holder's, or its key is malformed",
);
/// An imported app outside the slice, here already, or on unknown replicas.
const BAD_IMPORTED_APP: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an imported app is not the slice's, is here already, or names unknown replicas",
);

/// Judge the proof a command of this area carries ([`super::authorize`]).
///
/// [Account recovery](https://calimero-network.github.io/core/protocol/identities/#account-recovery): making and removing a
/// code, and cancelling a recovery, are the account's own passkey consent;
/// starting is the code key's signature over the new passkey; finishing is
/// that new passkey's sign-in assertion over the session it opens.
/// Organization recovery: setting the key and cancelling are the owner's
/// passkey consent, finishing the recovering account's; starting is the
/// recovery key's signature, checked against the key the organization holds
/// now, so a rotated key's signatures are refused.
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    match cmd {
        SystemCmd::SetAccountRecoveryKey {
            account,
            recovery_key,
            consent,
        } => passkey_consent(
            proofs,
            account,
            consent,
            &format!("account:{account}"),
            "__set_recovery",
            &json!({ "recoveryKey": recovery_key }),
        ),
        SystemCmd::RemoveAccountRecoveryKey { account, consent } => passkey_consent(
            proofs,
            account,
            consent,
            &format!("account:{account}"),
            "__remove_recovery",
            &json!({}),
        ),
        SystemCmd::StartAccountRecovery {
            recovery_key,
            account,
            credential,
            at,
            signature,
        } => authorize_account_recovery_start(
            proofs,
            recovery_key,
            account,
            credential,
            *at,
            signature,
        ),
        SystemCmd::CancelAccountRecovery {
            account,
            credential_id,
            consent,
        } => passkey_consent(
            proofs,
            account,
            consent,
            &format!("account:{account}"),
            "__cancel_recovery",
            &json!({ "credentialId": credential_id }),
        ),
        SystemCmd::FinishAccountRecovery {
            account,
            refresh,
            consent,
            ..
        } => authorize_account_recovery_finish(proofs, account, refresh, consent),
        SystemCmd::SetRecoveryKey {
            org,
            public_key,
            actor,
            consent,
            ..
        } => passkey_consent(
            proofs,
            actor,
            consent,
            &format!("org:{org}"),
            "__set_recovery_key",
            &json!({ "publicKey": public_key }),
        ),
        SystemCmd::StartRecovery {
            org,
            account,
            nonce,
            signature,
            ..
        } => authorize_recovery_start(proofs, org, account, nonce, signature),
        SystemCmd::CancelRecovery {
            org,
            account,
            actor,
            consent,
        } => passkey_consent(
            proofs,
            actor,
            consent,
            &format!("org:{org}"),
            "__cancel_recovery",
            &json!({ "account": account }),
        ),
        SystemCmd::FinishRecovery {
            org,
            actor,
            consent,
            ..
        } => passkey_consent(
            proofs,
            actor,
            consent,
            &format!("org:{org}"),
            "__finish_recovery",
            &json!({ "org": org }),
        ),
        SystemCmd::SetBackupKey {
            holder,
            public_key,
            actor,
            proof,
            ..
        } => authorize_backup_key(proofs, holder, public_key, actor, proof),
        // The cluster admits an import with the operator's signature over
        // exactly its files.
        SystemCmd::ImportHolder {
            holder,
            slice_sha256,
            apps,
            proof,
            ..
        } => {
            let (statement, used) = proofs.fresh(proofs.operator(proof)?)?;
            let stated = import_holder_args(holder, slice_sha256, apps);
            proofs.states(&statement, "cluster", "__recover_holder", &|args| {
                *args == stated
            })?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// `actor`'s passkey consent, not used before, stating `method` on `subject`
/// with exactly the arguments `stated`.
fn passkey_consent(
    proofs: &Proofs<'_>,
    actor: &str,
    consent: &Value,
    subject: &str,
    method: &str,
    stated: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
    proofs.states(&statement, subject, method, &|args| args == stated)?;
    Ok(Some(used))
}

/// The code's key signs an account recovery's start: only a node the person
/// typed the code into can make it, never one that only saw the log.
fn authorize_account_recovery_start(
    proofs: &Proofs<'_>,
    recovery_key: &str,
    account: &str,
    credential: &Credential,
    at: u64,
    signature: &str,
) -> Result<Option<String>, Rejection> {
    let state = proofs.s;
    if state.recovery_keys.get(recovery_key).map(String::as_str) != Some(account) {
        return Err(UNKNOWN_RECOVERY_CODE.into());
    }
    let used = crate::proof::verify_recover(recovery_key, account, credential, at, signature)?;
    if state.used_proofs.contains(&used) {
        return Err(RECOVERY_USED);
    }
    Ok(Some(used))
}

/// The passkey the start logged signs the session the finish opens, as any
/// sign-in does (`CreateRefresh`): user verified, over a challenge stating
/// that refresh token, for that passkey, unbound.
fn authorize_account_recovery_finish(
    proofs: &Proofs<'_>,
    account: &str,
    refresh: &RefreshGrant,
    consent: &Value,
) -> Result<Option<String>, Rejection> {
    let pending = proofs
        .s
        .account_recoveries
        .get(account)
        .ok_or(NO_ACCOUNT_RECOVERY_PENDING)?;
    let new = std::slice::from_ref(&pending.credential);
    let (statement, used) = proofs.fresh(verify_consent(consent, new)?)?;
    if refresh.record.account != account
        || refresh.record.passkey.as_deref() != Some(pending.credential.id.as_str())
        || refresh.record.app.is_some()
    {
        return Err(SESSION_FOR_ANOTHER_PASSKEY);
    }
    let stated = sign_in_args(&refresh.hash, &refresh.record);
    proofs.states(&statement, "session", "__sign_in", &|args| *args == stated)?;
    Ok(Some(used))
}

/// An organization recovery's start: the recovery key the organization holds
/// now signs it.
fn authorize_recovery_start(
    proofs: &Proofs<'_>,
    org: &str,
    account: &str,
    nonce: &str,
    signature: &str,
) -> Result<Option<String>, Rejection> {
    let state = proofs.s;
    let key = state
        .orgs
        .get(org)
        .ok_or(NO_SUCH_ORG)?
        .recovery
        .as_ref()
        .ok_or(NO_RECOVERY_KEY)?;
    let used = crate::proof::verify_recovery(&key.public_key, org, account, nonce, signature)?;
    if state.used_proofs.contains(&used) {
        return Err(RECOVERY_SIGNATURE_USED);
    }
    Ok(Some(used))
}

/// A backup key: the operator's signature for the operator's own, else the
/// actor's passkey; either over the key it sets. Who may set which holder's
/// is judged at apply, on the state then.
fn authorize_backup_key(
    proofs: &Proofs<'_>,
    holder: &BackupHolder,
    public_key: &str,
    actor: &str,
    proof: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = if *holder == BackupHolder::Operator {
        proofs.fresh(proofs.operator(proof)?)?
    } else {
        proofs.fresh(proofs.consent_of(actor, proof)?)?
    };
    let stated = json!({ "publicKey": public_key });
    proofs.states(&statement, &holder.key(), "__set_backup_key", &|args| {
        *args == stated
    })?;
    Ok(Some(used))
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let state = &mut m.state;
    match cmd {
        SystemCmd::ImportHolder {
            holder,
            slice,
            public_key,
            seed,
            apps,
            at,
            ..
        } => import_holder(state, holder, *slice, public_key, seed, apps, at),
        SystemCmd::SetBackupKey {
            holder,
            public_key,
            actor,
            at,
            ..
        } => set_backup_key(state, holder, public_key, actor, at),
        SystemCmd::SetAccountRecoveryKey {
            account,
            recovery_key,
            ..
        } => set_account_recovery_key(state, account, recovery_key),
        SystemCmd::RemoveAccountRecoveryKey { account, .. } => {
            let before = state.recovery_keys.len();
            state.recovery_keys.retain(|_, holder| *holder != account);
            if state.recovery_keys.len() == before {
                return Err(NO_CODE_SET);
            }
            Ok(Value::Null)
        }
        SystemCmd::StartAccountRecovery {
            recovery_key,
            account,
            credential,
            at,
            ..
        } => start_account_recovery(state, &recovery_key, account, credential, at),
        SystemCmd::CancelAccountRecovery {
            account,
            credential_id,
            ..
        } => {
            if state
                .account_recoveries
                .get(&account)
                .is_none_or(|pending| pending.credential.id != credential_id)
            {
                return Err(NO_ACCOUNT_RECOVERY_PENDING.into());
            }
            let _gone = state.account_recoveries.remove(&account);
            Ok(Value::Null)
        }
        SystemCmd::FinishAccountRecovery {
            account,
            at,
            refresh,
            consent,
            revoke_api_keys,
        } => finish_account_recovery(state, account, at, refresh, consent, revoke_api_keys),
        SystemCmd::SetRecoveryKey {
            org,
            public_key,
            at,
            actor,
            deployment_needs_recovery,
            ..
        } => set_recovery_key(
            state,
            &org,
            public_key,
            at,
            &actor,
            deployment_needs_recovery,
        ),
        SystemCmd::StartRecovery {
            org, account, at, ..
        } => start_recovery(state, &org, account, at),
        SystemCmd::CancelRecovery {
            org,
            account,
            actor,
            ..
        } => cancel_recovery(state, &org, &account, &actor),
        SystemCmd::FinishRecovery { org, at, actor, .. } => finish_recovery(state, org, at, actor),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached recovery::apply"),
    }
}

/// Set a holder's backup key: well formed, by whoever may set that holder's.
fn set_backup_key(
    state: &mut SystemState,
    holder: BackupHolder,
    public_key: String,
    actor: AccountId,
    at: u64,
) -> Result<Value, Rejection> {
    if !backup_key_fits(&public_key) {
        return Err(BAD_BACKUP_KEY.into());
    }
    if !state.may_set_backup_key(&holder, &actor) {
        return Err(NOT_BACKUP_HOLDER.into());
    }
    let key = BackupKey {
        holder: holder.clone(),
        public_key,
        set_at: at,
        set_by: actor,
    };
    let _prev = state.backup_keys.insert(holder.key(), key);
    Ok(Value::Null)
}

/// Set an account's recovery code, one per account (a new one replaces the
/// old), never one another account holds.
fn set_account_recovery_key(
    state: &mut SystemState,
    account: AccountId,
    recovery_key: String,
) -> Result<Value, Rejection> {
    if !state.accounts.contains_key(&account) {
        return Err(UNKNOWN_ACCOUNT);
    }
    if state.recovery_keys.contains_key(&recovery_key) {
        return Err(CODE_IN_USE);
    }
    state.recovery_keys.retain(|_, holder| *holder != account);
    let _prev = state.recovery_keys.insert(recovery_key, account);
    Ok(Value::Null)
}

/// Start an account recovery with its code, which it spends: one at a time
/// per account, with a passkey registered nowhere.
fn start_account_recovery(
    state: &mut SystemState,
    recovery_key: &str,
    account: AccountId,
    credential: Credential,
    at: u64,
) -> Result<Value, Rejection> {
    if state.recovery_keys.get(recovery_key) != Some(&account) {
        return Err(UNKNOWN_RECOVERY_CODE.into());
    }
    if !state.accounts.contains_key(&account) {
        return Err(UNKNOWN_ACCOUNT);
    }
    // One at a time: a code holder cannot replace a pending recovery with
    // their own, nor stack a second.
    if state.account_recoveries.contains_key(&account) {
        return Err(ACCOUNT_RECOVERY_PENDING);
    }
    if state.credentials.contains_key(&credential.id)
        || state
            .account_recoveries
            .values()
            .any(|pending| pending.credential.id == credential.id)
    {
        return Err(CREDENTIAL_REGISTERED);
    }
    // Single use, whether it finishes or is cancelled.
    let _spent = state.recovery_keys.remove(recovery_key);
    let pending = crate::types::PendingAccountRecovery {
        credential,
        started_at: at,
        ready_at: at.saturating_add(crate::types::RECOVERY_DELAY_SECS),
    };
    let out = json!(pending.view(&account));
    let _prev = state.account_recoveries.insert(account, pending);
    Ok(out)
}

/// Finish an account recovery once it is ready: the new passkey joins the
/// account and opens the session, and whoever holds the lost devices keeps no
/// session (nor, when asked, an API key).
fn finish_account_recovery(
    state: &mut SystemState,
    account: AccountId,
    at: u64,
    refresh: RefreshGrant,
    consent: Value,
    revoke_api_keys: bool,
) -> Result<Value, Rejection> {
    let pending = state
        .account_recoveries
        .get(&account)
        .ok_or(NO_ACCOUNT_RECOVERY_PENDING)?;
    if at < pending.ready_at {
        return Err(RECOVERY_NOT_READY);
    }
    if state.credentials.contains_key(&pending.credential.id) {
        return Err(CREDENTIAL_REGISTERED);
    }
    if state.refresh_known(&refresh.hash) {
        return Err(REFRESH_ISSUED);
    }
    let mut credential = pending.credential.clone();
    // The finish's assertion moved the passkey's counter: the next sign-in
    // must count past it.
    if let Some(count) = serde_json::from_value::<crate::proof::Consent>(consent)
        .ok()
        .and_then(|proof| calimero_identity::assertion_counter(&credential, &proof.assertion).ok())
    {
        credential.sign_count = credential.sign_count.max(count);
    }
    let record = state.accounts.get_mut(&account).ok_or(UNKNOWN_ACCOUNT)?;
    // Not held to the 10-passkey cap: a person who lost every passkey of a
    // full account must still get back in. Each recovery spends a code a
    // passkey approved, so this grows by one per code at most.
    record.passkeys.push(credential.clone());
    // Added and signed in with when the session opened (none: a finish an
    // earlier release logged).
    let opened = refresh.record.opened_at;
    if opened > 0 {
        let mut times = crate::types::PasskeyTimes {
            added_at: Some(opened),
            last_sign_in_at: None,
        };
        times.signed_in(opened);
        let _prev = record.passkey_times.insert(credential.id.clone(), times);
    }
    let _prev = state.credentials.insert(credential.id, account.clone());
    let _done = state.account_recoveries.remove(&account);
    let revoked = state.revoke_account_sessions(&account, at);
    let keys_before = state.api_keys.len();
    if revoke_api_keys {
        state.api_keys.retain(|_, key| key.account != account);
    }
    let revoked_keys = keys_before - state.api_keys.len();
    if let Some(entry) = sign_in_of(&refresh.record) {
        state.record_activity(&account, entry);
    }
    let _prev = state.refresh.insert(refresh.hash, refresh.record);
    Ok(json!({
        "accountId": account,
        "revoked": revoked,
        "revokedApiKeys": revoked_keys,
    }))
}

/// Set (or, with none, remove) an organization's recovery key: its owner's, and never
/// removed while the organization owns a deployment that needs one. A new key, or none,
/// ends a recovery under way with the old one.
fn set_recovery_key(
    state: &mut SystemState,
    org: &str,
    public_key: Option<String>,
    at: u64,
    actor: &str,
    deployment_needs_recovery: bool,
) -> Result<Value, Rejection> {
    let keeps_key =
        public_key.is_none() && !state.may_remove_recovery(org, deployment_needs_recovery);
    let recovery = public_key.map(|key| recovery_key_of(key, at)).transpose()?;
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if organization.role(actor) != Some(OrgRole::Owner) {
        return Err(OWNER_ONLY_RECOVERY.into());
    }
    if keeps_key {
        return Err(RECOVERY_KEY_REQUIRED);
    }
    organization.recovery = recovery;
    Ok(Value::Null)
}

/// Start an organization's recovery for an account not its owner, under its
/// recovery key, one at a time.
fn start_recovery(
    state: &mut SystemState,
    org: &str,
    account: AccountId,
    at: u64,
) -> Result<Value, Rejection> {
    if !state.accounts.contains_key(&account) {
        return Err(UNKNOWN_ACCOUNT);
    }
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if organization.role(&account) == Some(OrgRole::Owner) {
        return Err(OWNS_ALREADY);
    }
    let recovery = organization.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
    if recovery.pending.is_some() {
        return Err(RECOVERY_PENDING);
    }
    let pending = crate::types::PendingRecovery {
        account,
        started_at: at,
        ready_at: at.saturating_add(crate::types::RECOVERY_DELAY_SECS),
    };
    let out = json!(RecoveryStarted {
        started_at: pending.started_at,
        ready_at: pending.ready_at,
    });
    recovery.pending = Some(pending);
    Ok(out)
}

/// Cancel an organization's recovery for `account`: its owner's.
fn cancel_recovery(
    state: &mut SystemState,
    org: &str,
    account: &str,
    actor: &str,
) -> Result<Value, Rejection> {
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if organization.role(actor) != Some(OrgRole::Owner) {
        return Err(OWNER_ONLY_RECOVERY.into());
    }
    let recovery = organization.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
    if recovery
        .pending
        .as_ref()
        .map(|pending| pending.account.as_str())
        != Some(account)
    {
        return Err(NO_RECOVERY_PENDING.into());
    }
    recovery.pending = None;
    Ok(Value::Null)
}

/// Finish an organization's recovery once it is ready: the recovering
/// account becomes its owner, the previous owners leave it, and their apps in
/// it move to the new owner.
fn finish_recovery(
    state: &mut SystemState,
    org: OrgId,
    at: u64,
    actor: AccountId,
) -> Result<Value, Rejection> {
    if !state.accounts.contains_key(&actor) {
        return Err(UNKNOWN_ACCOUNT);
    }
    let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
    let recovery = organization.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
    let Some(pending) = recovery
        .pending
        .as_ref()
        .filter(|pending| pending.account == actor)
    else {
        return Err(NO_RECOVERY_PENDING.into());
    };
    if at < pending.ready_at {
        return Err(RECOVERY_NOT_READY);
    }
    recovery.pending = None;
    let previous: Vec<AccountId> = organization
        .members
        .iter()
        .filter(|(_, role)| **role == OrgRole::Owner)
        .map(|(member, _)| member.clone())
        .collect();
    for owner in &previous {
        drop_member_rows(organization, owner);
    }
    let _prev = organization.members.insert(actor.clone(), OrgRole::Owner);
    // The previous owner's apps in the organization reach their owner
    // whatever the membership: they move with it.
    for app in state.apps.values_mut() {
        if app.org == org && previous.contains(&app.owner) {
            app.owner.clone_from(&actor);
        }
    }
    Ok(json!(RecoveryFinished {
        org,
        previous_owners: previous,
    }))
}

/// Every row `account` holds in the organization and its teams.
fn drop_member_rows(organization: &mut Org, account: &str) {
    let _prev = organization.members.remove(account);
    let _caps = organization.capabilities.remove(account);
    let _meta = organization.member_metadata.remove(account);
    let _all = organization.all_apps.remove(account);
    for team in organization.teams.values_mut() {
        let _prev = team.members.remove(account);
        let _was = team.read_only.remove(account);
        let _caps = team.capabilities.remove(account);
    }
}

/// Apply `ImportHolder` (see it): all of it or nothing.
fn import_holder(
    state: &mut SystemState,
    holder: BackupHolder,
    slice: HolderSlice,
    public_key: String,
    seed: u64,
    apps: Vec<ImportedApp>,
    at: u64,
) -> Result<Value, Rejection> {
    check_importable(state, &holder, &slice, &public_key)?;
    check_imported_apps(state, &slice, seed, &apps)?;
    // A passkey another account here holds would sign in as either.
    let incoming: Vec<&SliceAccount> = slice
        .accounts
        .iter()
        .filter(|account| !state.accounts.contains_key(&account.account_id))
        .collect();
    if incoming
        .iter()
        .flat_map(|account| account.passkeys.iter())
        .any(|credential| state.credentials.contains_key(&credential.id))
    {
        return Err(ALREADY_HERE.into());
    }
    let mut pins = BTreeMap::new();
    for app in &apps {
        let record = &slice.apps[&app.app_id];
        if !state.bundles.contains_key(&record.bundle)
            && !slice.bundles.contains_key(&record.bundle)
        {
            return Err(RESTORE_BUNDLE_GONE.into());
        }
        let _prev = pins.insert(app.app_id.clone(), imported_lineage(state, &slice, record)?);
        // Judged from the version that brings lineages, so an earlier entry replays as it did.
        if state.cluster_brings(cluster_version::Feature::PinnedLineage)
            && !imported_release_ships(state, &slice, record)
        {
            return Err(RELEASE_NOT_SHIPPED_HERE);
        }
    }
    // Checked: from here nothing is refused.
    for (hash, bundle) in &slice.bundles {
        if !state.bundles.contains_key(hash) {
            let _prev = state.bundles.insert(hash.clone(), bundle.clone());
        }
    }
    add_imported_accounts(state, &incoming);
    for (name, package) in slice.packages {
        let _kept = state.packages.entry(name).or_insert(package);
    }
    if let Some(org) = slice.org {
        let _prev = state.orgs.insert(org.org_id.clone(), org);
    }
    let mut groups = Vec::new();
    for app in apps {
        let mut record = slice.apps[&app.app_id].clone();
        record.lineage = pins.remove(&app.app_id).flatten();
        if let Some(l) = record.lineage.as_mut() {
            // At least the release it runs (named, else the one its bundle ships in here).
            let runs = record
                .release
                .as_ref()
                .map(|r| r.version.clone())
                .or_else(|| {
                    state.lineage_shipping(l, &record.bundle, record.compute_bundle.as_deref())
                });
            if let Some(r) = runs {
                l.raise_floor(&r);
            }
        }
        let group = state.next_group.max(1) + 1;
        state.next_group = group;
        record.group = group;
        record.replicas = app.replicas;
        record.restore = Some(crate::types::PendingRestore {
            seed: Some(seed),
            source: Some(app.source),
        });
        groups.push(json!({ "app": app.app_id, "group": group }));
        let _prev = state.apps.insert(app.app_id, record);
    }
    let key = BackupKey {
        holder: holder.clone(),
        public_key,
        set_at: at,
        set_by: OPERATOR.to_owned(),
    };
    let _prev = state.backup_keys.insert(holder.key(), key);
    Ok(json!({ "apps": groups }))
}

/// Whether `record`'s release ships its bundle and compute part as this cluster will hold its
/// package (here, else the slice's), under the signer the slice holds it under. A record that
/// names no release is judged by the release its bundle is recorded from, which the slice must
/// carry and, where this cluster records that bundle too, must agree with; a raw bundle passes.
fn imported_release_ships(state: &SystemState, slice: &HolderSlice, record: &AppRecord) -> bool {
    let attributed = |hash: &str| {
        let theirs = slice.bundles.get(hash)?;
        let ours = state.bundles.get(hash).unwrap_or(theirs);
        (ours.package == theirs.package).then_some(theirs.package.clone())
    };
    let r = match &record.release {
        Some(r) => r.clone(),
        None => match attributed(&record.bundle) {
            Some(Some(r)) => r,
            Some(None) => return true,
            None => return false,
        },
    };
    if record
        .compute_bundle
        .as_deref()
        .is_some_and(|c| record.release.is_none() && attributed(c).is_none())
    {
        return false;
    }
    let (Some(theirs), Some(held)) = (
        slice.packages.get(&r.package),
        state
            .packages
            .get(&r.package)
            .or(slice.packages.get(&r.package)),
    ) else {
        return false;
    };
    let ships = |hash: &str| {
        held.versions
            .get(&r.version)
            .is_some_and(|v| v.parts(&r.package).any(|(h, _)| h == hash))
    };
    theirs.signer_id == held.signer_id
        && ships(&record.bundle)
        && record.compute_bundle.as_deref().is_none_or(ships)
}

/// The lineage `record` keeps on import: its own, else its release's as the slice records it,
/// and only where this cluster will hold that package under the same signer.
///
/// # Errors
/// [`OUTSIDE_LINEAGE`] when the package is held under another signer, or by none.
fn imported_lineage(
    state: &SystemState,
    slice: &HolderSlice,
    record: &AppRecord,
) -> Result<Option<AppLineage>, Rejection> {
    let release = record
        .release
        .as_ref()
        .or_else(|| slice.bundles.get(&record.bundle)?.package.as_ref());
    // Pinning as the slice records the release, or as this cluster holds it.
    let pinning = |r: &PackageRef| {
        [
            slice.packages.get(&r.package),
            state.packages.get(&r.package),
        ]
        .into_iter()
        .flatten()
        .find(|p| {
            p.versions
                .get(&r.version)
                .is_some_and(|v| !v.lineage.is_free())
        })
        .map(|p| AppLineage {
            package: r.package.clone(),
            signer_id: p.signer_id.clone(),
            floor: r.version.clone(),
        })
    };
    let Some(pin) = record.lineage.clone().or_else(|| release.and_then(pinning)) else {
        return Ok(None);
    };
    if release.is_some_and(|r| r.package != pin.package) {
        return Err(OUTSIDE_LINEAGE);
    }
    let held = state
        .packages
        .get(&pin.package)
        .or(slice.packages.get(&pin.package));
    if held.is_some_and(|p| p.signer_id == pin.signer_id) {
        Ok(Some(pin))
    } else {
        Err(OUTSIDE_LINEAGE)
    }
}

/// An import is of an organization or account this cluster does not hold,
/// where owners upgrade apps, from that holder's slice with a well-formed key.
fn check_importable(
    state: &SystemState,
    holder: &BackupHolder,
    slice: &HolderSlice,
    public_key: &str,
) -> Result<(), Rejection> {
    if *holder == BackupHolder::Operator || state.governance().app_upgrades == Authority::Operator {
        return Err(NOT_IMPORTABLE.into());
    }
    if slice.holder != *holder || !backup_key_fits(public_key) {
        return Err(NOT_THE_HOLDERS_SLICE);
    }
    match holder {
        BackupHolder::Org(org) => {
            if state.orgs.contains_key(org)
                || slice.org.as_ref().map(|slice_org| &slice_org.org_id) != Some(org)
            {
                return Err(ALREADY_HERE.into());
            }
        }
        BackupHolder::Account(account) => {
            if state.accounts.contains_key(account) {
                return Err(ALREADY_HERE.into());
            }
        }
        BackupHolder::Operator => return Err(NOT_IMPORTABLE.into()),
    }
    Ok(())
}

/// Each imported app is the slice's, new here, named once, seeded from
/// `seed` first, on distinct replica nodes this cluster holds.
fn check_imported_apps(
    state: &SystemState,
    slice: &HolderSlice,
    seed: u64,
    apps: &[ImportedApp],
) -> Result<(), Rejection> {
    let mut seen = BTreeSet::new();
    for app in apps {
        let fits = slice.apps.contains_key(&app.app_id)
            && !state.apps.contains_key(&app.app_id)
            && seen.insert(app.app_id.clone())
            && app.replicas.first() == Some(&seed)
            && app.replicas.iter().collect::<BTreeSet<_>>().len() == app.replicas.len()
            && app
                .replicas
                .iter()
                .all(|id| state.replica_node(id).is_some());
        if !fits {
            return Err(BAD_IMPORTED_APP);
        }
    }
    Ok(())
}

/// The slice's accounts this cluster did not hold, with their passkeys.
fn add_imported_accounts(state: &mut SystemState, incoming: &[&SliceAccount]) {
    for imported in incoming {
        let account = Account {
            account_id: imported.account_id.clone(),
            name: imported.name.clone(),
            created_at: imported.created_at,
            passkeys: imported.passkeys.clone(),
            passkey_labels: BTreeMap::new(),
            aliases: BTreeMap::new(),
            passkey_times: BTreeMap::new(),
        };
        for credential in &imported.passkeys {
            let _prev = state
                .credentials
                .insert(credential.id.clone(), imported.account_id.clone());
        }
        let _prev = state.accounts.insert(imported.account_id.clone(), account);
    }
}
