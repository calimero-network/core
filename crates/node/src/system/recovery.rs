//! System commands for account and organization recovery.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;
use crate::proof::verify_consent;

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(
    proofs: &Proofs<'_>,
    cmd: &SystemCmd,
) -> Result<Option<String>, &'static str> {
    let s = proofs.s;
    match cmd {
        // Account recovery (`docs/specs/account-recovery.md`): making and
        // removing a code, and cancelling a recovery, are the account's own
        // passkey consent; starting is the code key's signature over the
        // new passkey; finishing is that new passkey's sign-in assertion
        // over the session it opens.
        SystemCmd::SetAccountRecoveryKey {
            account,
            recovery_key,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
            let args = json!({ "recoveryKey": recovery_key });
            proofs.states(&st, &format!("account:{account}"), "__set_recovery", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemoveAccountRecoveryKey { account, consent } => {
            let (st, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
            proofs.states(
                &st,
                &format!("account:{account}"),
                "__remove_recovery",
                &|a| *a == json!({}),
            )?;
            Ok(Some(used))
        }
        // The code's key signs the start: only a node the person typed the
        // code into can make it, never one that only saw the log.
        SystemCmd::StartAccountRecovery {
            recovery_key,
            account,
            credential,
            at,
            signature,
        } => {
            if s.recovery_keys.get(recovery_key) != Some(account) {
                return Err(UNKNOWN_RECOVERY_CODE);
            }
            let used =
                crate::proof::verify_recover(recovery_key, account, credential, *at, signature)?;
            if s.used_proofs.contains(&used) {
                return Err("that recovery was used already");
            }
            Ok(Some(used))
        }
        SystemCmd::CancelAccountRecovery {
            account,
            credential_id,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
            let args = json!({ "credentialId": credential_id });
            proofs.states(
                &st,
                &format!("account:{account}"),
                "__cancel_recovery",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        // The passkey the start logged signs the session the finish opens,
        // as any sign-in does (`CreateRefresh`): user verified, over a
        // challenge stating that refresh token, for that passkey, unbound.
        SystemCmd::FinishAccountRecovery {
            account,
            refresh,
            consent,
            ..
        } => {
            let pending = s
                .account_recoveries
                .get(account)
                .ok_or(NO_ACCOUNT_RECOVERY_PENDING)?;
            let new = std::slice::from_ref(&pending.credential);
            let (st, used) = proofs.fresh(verify_consent(consent, new)?)?;
            if refresh.record.account != *account
                || refresh.record.passkey.as_deref() != Some(pending.credential.id.as_str())
                || refresh.record.app.is_some()
            {
                return Err("the session is for another passkey");
            }
            let args = sign_in_args(&refresh.hash, &refresh.record);
            proofs.states(&st, "session", "__sign_in", &|a| *a == args)?;
            Ok(Some(used))
        }
        // Recovery: setting the key and cancelling are the owner's passkey
        // consent, finishing the recovering account's; starting is the
        // recovery key's signature, checked against the key the organization
        // holds now, so a rotated key's signatures are refused.
        SystemCmd::SetRecoveryKey {
            org,
            public_key,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "publicKey": public_key });
            proofs.states(&st, &format!("org:{org}"), "__set_recovery_key", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::StartRecovery {
            org,
            account,
            nonce,
            signature,
            ..
        } => {
            let key = s
                .orgs
                .get(org)
                .ok_or("no such organization")?
                .recovery
                .as_ref()
                .ok_or(NO_RECOVERY_KEY)?;
            let used =
                crate::proof::verify_recovery(&key.public_key, org, account, nonce, signature)?;
            if s.used_proofs.contains(&used) {
                return Err("that recovery signature was used already");
            }
            Ok(Some(used))
        }
        SystemCmd::CancelRecovery {
            org,
            account,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "account": account });
            proofs.states(&st, &format!("org:{org}"), "__cancel_recovery", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::FinishRecovery {
            org,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "org": org });
            proofs.states(&st, &format!("org:{org}"), "__finish_recovery", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        // A backup key: the operator's signature for the operator's own,
        // else the actor's passkey; either over the key it sets. Who may set
        // which holder's is judged at apply, on the state then.
        SystemCmd::SetBackupKey {
            holder,
            public_key,
            actor,
            proof,
            ..
        } => {
            let (st, used) = if *holder == BackupHolder::Operator {
                proofs.fresh(proofs.operator(proof)?)?
            } else {
                proofs.fresh(proofs.consent_of(actor, proof)?)?
            };
            let args = json!({ "publicKey": public_key });
            proofs.states(&st, &holder.key(), "__set_backup_key", &|a| *a == args)?;
            Ok(Some(used))
        }
        // The cluster admits an import with the operator's signature over
        // exactly its files.
        SystemCmd::ImportHolder {
            holder,
            slice_sha256,
            apps,
            proof,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let args = import_holder_args(holder, slice_sha256, apps);
            proofs.states(&st, "cluster", "__recover_holder", &|a| *a == args)?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let s = &mut m.state;
    match cmd {
        SystemCmd::ImportHolder {
            holder,
            slice,
            public_key,
            seed,
            apps,
            at,
            ..
        } => import_holder(s, holder, *slice, public_key, seed, apps, at),
        SystemCmd::SetBackupKey {
            holder,
            public_key,
            actor,
            at,
            ..
        } => {
            if !backup_key_fits(&public_key) {
                return Err(BAD_BACKUP_KEY);
            }
            if !s.may_set_backup_key(&holder, &actor) {
                return Err(NOT_BACKUP_HOLDER);
            }
            let key = BackupKey {
                holder: holder.clone(),
                public_key,
                set_at: at,
                set_by: actor,
            };
            let _prev = s.backup_keys.insert(holder.key(), key);
            Ok(Value::Null)
        }
        SystemCmd::SetAccountRecoveryKey {
            account,
            recovery_key,
            ..
        } => {
            if !s.accounts.contains_key(&account) {
                return Err("unknown account");
            }
            if s.recovery_keys.contains_key(&recovery_key) {
                return Err("that recovery code is in use");
            }
            // One code per account: a new one replaces the old.
            s.recovery_keys.retain(|_, a| *a != account);
            let _prev = s.recovery_keys.insert(recovery_key, account);
            Ok(Value::Null)
        }
        SystemCmd::RemoveAccountRecoveryKey { account, .. } => {
            let before = s.recovery_keys.len();
            s.recovery_keys.retain(|_, a| *a != account);
            if s.recovery_keys.len() == before {
                return Err("no recovery code is set");
            }
            Ok(Value::Null)
        }
        SystemCmd::StartAccountRecovery {
            recovery_key,
            account,
            credential,
            at,
            ..
        } => {
            if s.recovery_keys.get(&recovery_key) != Some(&account) {
                return Err(UNKNOWN_RECOVERY_CODE);
            }
            if !s.accounts.contains_key(&account) {
                return Err("unknown account");
            }
            // One at a time: a code holder cannot replace a pending
            // recovery with their own, nor stack a second.
            if s.account_recoveries.contains_key(&account) {
                return Err(ACCOUNT_RECOVERY_PENDING);
            }
            if s.credentials.contains_key(&credential.id)
                || s.account_recoveries
                    .values()
                    .any(|p| p.credential.id == credential.id)
            {
                return Err("credential already registered");
            }
            // Single use, whether it finishes or is cancelled.
            let _spent = s.recovery_keys.remove(&recovery_key);
            let pending = crate::types::PendingAccountRecovery {
                credential,
                started_at: at,
                ready_at: at.saturating_add(crate::types::RECOVERY_DELAY_SECS),
            };
            let out = pending.json(&account);
            let _prev = s.account_recoveries.insert(account, pending);
            Ok(out)
        }
        SystemCmd::CancelAccountRecovery {
            account,
            credential_id,
            ..
        } => {
            if s.account_recoveries
                .get(&account)
                .is_none_or(|p| p.credential.id != credential_id)
            {
                return Err(NO_ACCOUNT_RECOVERY_PENDING);
            }
            let _gone = s.account_recoveries.remove(&account);
            Ok(Value::Null)
        }
        SystemCmd::FinishAccountRecovery {
            account,
            at,
            refresh,
            consent,
            revoke_api_keys,
        } => {
            let p = s
                .account_recoveries
                .get(&account)
                .ok_or(NO_ACCOUNT_RECOVERY_PENDING)?;
            if at < p.ready_at {
                return Err(RECOVERY_NOT_READY);
            }
            if s.credentials.contains_key(&p.credential.id) {
                return Err("credential already registered");
            }
            if s.refresh_known(&refresh.hash) {
                return Err("refresh token already issued");
            }
            let mut credential = p.credential.clone();
            // The finish's assertion moved the passkey's counter: the
            // next sign-in must count past it.
            if let Some(count) = serde_json::from_value::<crate::proof::Consent>(consent)
                .ok()
                .and_then(|c| calimero_identity::assertion_counter(&credential, &c.assertion).ok())
            {
                credential.sign_count = credential.sign_count.max(count);
            }
            let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
            // Not held to the 10-passkey cap: a person who lost every
            // passkey of a full account must still get back in. Each
            // recovery spends a code a passkey approved, so this grows
            // by one per code at most.
            acc.passkeys.push(credential.clone());
            // Added and signed in with when the session opened (none: a
            // finish an earlier release logged).
            let opened = refresh.record.opened_at;
            if opened > 0 {
                let mut times = crate::types::PasskeyTimes {
                    added_at: Some(opened),
                    last_sign_in_at: None,
                };
                times.signed_in(opened);
                let _prev = acc.passkey_times.insert(credential.id.clone(), times);
            }
            let _prev = s.credentials.insert(credential.id, account.clone());
            let _done = s.account_recoveries.remove(&account);
            // Whoever holds the lost devices keeps no session, and no
            // machine credential either.
            let revoked = s.revoke_account_sessions(&account, at);
            let keys_before = s.api_keys.len();
            if revoke_api_keys {
                s.api_keys.retain(|_, k| k.account != account);
            }
            let revoked_keys = keys_before - s.api_keys.len();
            if let Some(entry) = sign_in_of(&refresh.record) {
                s.record_activity(&account, entry);
            }
            let _prev = s.refresh.insert(refresh.hash, refresh.record);
            Ok(json!({
                "accountId": account,
                "revoked": revoked,
                "revokedApiKeys": revoked_keys,
            }))
        }
        SystemCmd::SetRecoveryKey {
            org,
            public_key,
            at,
            actor,
            ..
        } => {
            let recovery = public_key.map(|k| recovery_key_of(k, at)).transpose()?;
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            if o.role(&actor) != Some(OrgRole::Owner) {
                return Err(OWNER_ONLY_RECOVERY);
            }
            // A new key, or none: a recovery under way ends with the old one.
            o.recovery = recovery;
            Ok(Value::Null)
        }
        SystemCmd::StartRecovery {
            org, account, at, ..
        } => {
            if !s.accounts.contains_key(&account) {
                return Err("unknown account");
            }
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            if o.role(&account) == Some(OrgRole::Owner) {
                return Err("that account owns the organization already");
            }
            let r = o.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
            if r.pending.is_some() {
                return Err(RECOVERY_PENDING);
            }
            let pending = crate::types::PendingRecovery {
                account,
                started_at: at,
                ready_at: at.saturating_add(crate::types::RECOVERY_DELAY_SECS),
            };
            let out = json!({ "startedAt": pending.started_at, "readyAt": pending.ready_at });
            r.pending = Some(pending);
            Ok(out)
        }
        SystemCmd::CancelRecovery {
            org,
            account,
            actor,
            ..
        } => {
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            if o.role(&actor) != Some(OrgRole::Owner) {
                return Err(OWNER_ONLY_RECOVERY);
            }
            let r = o.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
            if r.pending.as_ref().map(|p| &p.account) != Some(&account) {
                return Err(NO_RECOVERY_PENDING);
            }
            r.pending = None;
            Ok(Value::Null)
        }
        SystemCmd::FinishRecovery { org, at, actor, .. } => {
            if !s.accounts.contains_key(&actor) {
                return Err("unknown account");
            }
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            let r = o.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
            let Some(p) = r.pending.as_ref().filter(|p| p.account == actor) else {
                return Err(NO_RECOVERY_PENDING);
            };
            if at < p.ready_at {
                return Err(RECOVERY_NOT_READY);
            }
            r.pending = None;
            let previous: Vec<AccountId> = o
                .members
                .iter()
                .filter(|(_, r)| **r == OrgRole::Owner)
                .map(|(a, _)| a.clone())
                .collect();
            for a in &previous {
                let _prev = o.members.remove(a);
                let _caps = o.capabilities.remove(a);
                let _meta = o.member_metadata.remove(a);
                let _all = o.all_apps.remove(a);
                for t in o.teams.values_mut() {
                    let _prev = t.members.remove(a);
                    let _was = t.read_only.remove(a);
                    let _caps = t.capabilities.remove(a);
                }
            }
            let _prev = o.members.insert(actor.clone(), OrgRole::Owner);
            // The previous owner's apps in the organization reach their
            // owner whatever the membership: they move with it.
            for app in s.apps.values_mut() {
                if app.org == org && previous.contains(&app.owner) {
                    app.owner.clone_from(&actor);
                }
            }
            Ok(json!({ "org": org, "previousOwners": previous }))
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached recovery::apply"),
    }
}

/// Apply `ImportHolder` (see it): all of it or nothing.
fn import_holder(
    s: &mut SystemState,
    holder: BackupHolder,
    slice: HolderSlice,
    public_key: String,
    seed: u64,
    apps: Vec<ImportedApp>,
    at: u64,
) -> Result<Value, &'static str> {
    if holder == BackupHolder::Operator || s.governance().app_upgrades == Authority::Operator {
        return Err(NOT_IMPORTABLE);
    }
    if slice.holder != holder || !backup_key_fits(&public_key) {
        return Err("the slice is not that holder's, or its key is malformed");
    }
    match &holder {
        BackupHolder::Org(org) => {
            if s.orgs.contains_key(org) || slice.org.as_ref().map(|o| &o.org_id) != Some(org) {
                return Err(ALREADY_HERE);
            }
        }
        BackupHolder::Account(account) => {
            if s.accounts.contains_key(account) {
                return Err(ALREADY_HERE);
            }
        }
        BackupHolder::Operator => return Err(NOT_IMPORTABLE),
    }
    let mut seen = BTreeSet::new();
    for a in &apps {
        let fits = slice.apps.contains_key(&a.app_id)
            && !s.apps.contains_key(&a.app_id)
            && seen.insert(a.app_id.clone())
            && a.replicas.first() == Some(&seed)
            && a.replicas.iter().collect::<BTreeSet<_>>().len() == a.replicas.len()
            && a.replicas.iter().all(|id| s.replica_node(id).is_some());
        if !fits {
            return Err(
                "an imported app is not the slice's, is here already, or names unknown replicas",
            );
        }
    }
    // A passkey another account here holds would sign in as either.
    let incoming: Vec<&SliceAccount> = slice
        .accounts
        .iter()
        .filter(|a| !s.accounts.contains_key(&a.account_id))
        .collect();
    if incoming
        .iter()
        .flat_map(|a| a.passkeys.iter())
        .any(|c| s.credentials.contains_key(&c.id))
    {
        return Err(ALREADY_HERE);
    }
    for (hash, bundle) in &slice.bundles {
        if !s.bundles.contains_key(hash) {
            let _prev = s.bundles.insert(hash.clone(), bundle.clone());
        }
    }
    for a in &apps {
        let record = &slice.apps[&a.app_id];
        if !s.bundles.contains_key(&record.bundle) {
            return Err(RESTORE_BUNDLE_GONE);
        }
    }
    // Checked: from here nothing is refused.
    for a in incoming {
        let account = Account {
            account_id: a.account_id.clone(),
            name: a.name.clone(),
            created_at: a.created_at,
            passkeys: a.passkeys.clone(),
            passkey_labels: BTreeMap::new(),
            aliases: BTreeMap::new(),
            passkey_times: BTreeMap::new(),
        };
        for c in &a.passkeys {
            let _prev = s.credentials.insert(c.id.clone(), a.account_id.clone());
        }
        let _prev = s.accounts.insert(a.account_id.clone(), account);
    }
    for (name, package) in slice.packages {
        let _kept = s.packages.entry(name).or_insert(package);
    }
    if let Some(org) = slice.org {
        let _prev = s.orgs.insert(org.org_id.clone(), org);
    }
    let mut groups = Vec::new();
    for a in apps {
        let mut record = slice.apps[&a.app_id].clone();
        let group = s.next_group.max(1) + 1;
        s.next_group = group;
        record.group = group;
        record.replicas = a.replicas;
        record.restore = Some(crate::types::PendingRestore {
            seed: Some(seed),
            source: Some(a.source),
        });
        groups.push(json!({ "app": a.app_id, "group": group }));
        let _prev = s.apps.insert(a.app_id, record);
    }
    let key = BackupKey {
        holder: holder.clone(),
        public_key,
        set_at: at,
        set_by: OPERATOR.to_owned(),
    };
    let _prev = s.backup_keys.insert(holder.key(), key);
    Ok(json!({ "apps": groups }))
}
