//! System commands for accounts, passkeys, sessions and API keys.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(
    proofs: &Proofs<'_>,
    cmd: &SystemCmd,
) -> Result<Option<String>, &'static str> {
    let s = proofs.s;
    match cmd {
        // Credentials: the account's own passkey consent. A new passkey is
        // approved by one the account already has (`consent_of` reads the
        // passkeys as they stand, without the new one).
        SystemCmd::AddPasskey {
            account,
            credential,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
            let args = json!({
                "credentialId": credential.id,
                "publicKey": credential.public_key,
            });
            proofs.states(&st, &format!("account:{account}"), "__add_passkey", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemovePasskey {
            account,
            credential_id,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
            let args = json!({ "credentialId": credential_id });
            proofs.states(
                &st,
                &format!("account:{account}"),
                "__remove_passkey",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        SystemCmd::CreateApiKey {
            hash,
            record,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(&record.account, consent)?)?;
            let args = api_key_args(hash, record);
            proofs.states(
                &st,
                &format!("account:{}", record.account),
                "__create_api_key",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        SystemCmd::RevokeApiKey {
            key_id,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&st, &format!("account:{actor}"), "__revoke_api_key", &|a| {
                *a == json!({ "keyId": key_id })
            })?;
            Ok(Some(used))
        }
        // Sessions. A sign-up: the new passkey's assertion over the whole
        // account and the session it opens, so a node that saw someone's new
        // credential cannot register it first under an account and a release
        // of its own.
        SystemCmd::CreateAccount {
            account,
            invite,
            refresh,
            consent,
            ..
        } => {
            let (st, used) =
                proofs.fresh(crate::proof::verify_sign_in(consent, &account.passkeys)?)?;
            // An invite at sign-up: the token's key signs it, as a redemption
            // of the new account, so only the node the person gave it to can.
            // Its mark is spent where the account applies (`run_authorized`).
            if let Some(r) = invite {
                if !s.invites.contains_key(&r.invite_key)
                    && !s.sign_up_invites.contains_key(&r.invite_key)
                {
                    return Err(
                        "sign-up refused: this invite was used up, revoked or never existed",
                    );
                }
                let redeemed = crate::proof::verify_invite_redeem(
                    &r.invite_key,
                    &account.account_id,
                    r.now,
                    &r.signature,
                )?;
                if s.used_proofs.contains(&redeemed) {
                    return Err("that redemption was used already");
                }
            }
            if refresh
                .as_ref()
                .is_some_and(|g| g.record.account != account.account_id)
            {
                return Err("the consent is for another change");
            }
            let args = account_args(account, refresh.as_ref());
            proofs.states(
                &st,
                &format!("account:{}", account.account_id),
                "__create_account",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        // A sign-in: the assertion it took, by the passkey the session names,
        // over a challenge that states this refresh token.
        SystemCmd::CreateRefresh {
            hash,
            record,
            consent,
        } => {
            let passkey: Vec<Credential> = s
                .accounts
                .get(&record.account)
                .ok_or("unknown account")?
                .passkeys
                .iter()
                .filter(|c| record.passkey.as_deref() == Some(c.id.as_str()))
                .cloned()
                .collect();
            let (st, used) = proofs.fresh(crate::proof::verify_sign_in(consent, &passkey)?)?;
            let args = sign_in_args(hash, record);
            proofs.states(&st, "session", "__sign_in", &|a| *a == args)?;
            Ok(Some(used))
        }
        // An exchange: the old token's key, over the new token and the
        // session it continues, which stays the old one's.
        SystemCmd::UseRefresh {
            old_hash,
            new_hash,
            record,
            now,
            signature,
        } => {
            let old = s.refresh.get(old_hash).ok_or("unknown refresh token")?;
            if old.key.is_empty() {
                return Err("this refresh token has no key; sign in again");
            }
            if old.account != record.account
                || old.session != record.session
                || old.passkey != record.passkey
                || old.app != record.app
                || old.origin != record.origin
            {
                return Err("the exchange is for another session");
            }
            let message = crate::proof::refresh_use_message(
                old_hash,
                new_hash,
                &record.key,
                &record.session,
                record.expires_at,
                *now,
            );
            crate::proof::verify_refresh_use(&old.key, &message, signature)?;
            Ok(None)
        }
        // A counter: an assertion of that passkey carrying it. Only the
        // authenticator signs a counter, so none is set past what it reached.
        SystemCmd::SetSignCount {
            credential_id,
            count,
            assertion,
        } => {
            let (_, credential) = s
                .account_of_credential(credential_id)
                .ok_or("unknown credential")?;
            let assertion: calimero_identity::Assertion = serde_json::from_value(assertion.clone())
                .map_err(|_| "the counter carries no assertion")?;
            let carried = calimero_identity::assertion_counter(credential, &assertion)
                .map_err(|_| "the assertion does not verify")?;
            if carried != *count {
                return Err("the assertion carries another counter");
            }
            Ok(None)
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
        SystemCmd::CreateAccount {
            mut account,
            invite,
            mut refresh,
            at,
            ..
        } => {
            // The first account waits for genesis: the rules it joins
            // under are fixed before anyone relies on them.
            if s.governance.is_none() && s.accounts.is_empty() {
                return Err("the cluster is starting");
            }
            if s.accounts.contains_key(&account.account_id) {
                return Err("account exists");
            }
            // Judged before anything changes: a refusal leaves no trace.
            let admission = s.admits(invite.as_ref())?;
            if s.accounts.len() >= s.quotas.max_accounts as usize {
                return Err("quota exceeded: accounts");
            }
            for c in &account.passkeys {
                if s.credentials.contains_key(&c.id) {
                    return Err("credential already registered");
                }
            }
            if refresh.as_ref().is_some_and(|g| s.refresh_known(&g.hash)) {
                return Err("refresh token already issued");
            }
            for c in &account.passkeys {
                let _prev = s
                    .credentials
                    .insert(c.id.clone(), account.account_id.clone());
            }
            // The sign-up adds its passkey and signs in with it, at the
            // time the entry carries (none: an earlier release's).
            account.passkey_times = BTreeMap::new();
            if at > 0 {
                for c in &account.passkeys {
                    let mut times = crate::types::PasskeyTimes {
                        added_at: Some(at),
                        last_sign_in_at: None,
                    };
                    if refresh.is_some() {
                        times.signed_in(at);
                    }
                    let _prev = account.passkey_times.insert(c.id.clone(), times);
                }
                if let Some(g) = &mut refresh {
                    g.record.opened_at = at;
                }
            }
            let signed_in = refresh.as_ref().and_then(|g| sign_in_of(&g.record));
            if let Some(RefreshGrant { hash, record }) = refresh {
                let _prev = s.refresh.insert(hash, record);
            }
            let id = account.account_id.clone();
            let _prev = s.accounts.insert(id.clone(), account);
            if let Some(entry) = signed_in {
                s.record_activity(&id, entry);
            }
            if let Some(r) = &invite {
                let _new = s.used_proofs.insert(crate::proof::invite_redeem_used(
                    &r.invite_key,
                    &id,
                    r.now,
                ));
            }
            let org = match (admission, invite) {
                (Admission::OrgInvite, Some(r)) => {
                    // A new account is never a member yet: every use is
                    // spent, as before the free redemption existed.
                    Some(s.redeem_org_invite(&r.invite_key, &id, r.now, false)?)
                }
                (Admission::SignUpInvite, Some(r)) => {
                    s.spend_sign_up_invite(&r.invite_key);
                    None
                }
                _ => None,
            };
            Ok(match org {
                Some(org) => json!({ "accountId": id, "org": org }),
                None => json!({ "accountId": id }),
            })
        }
        SystemCmd::AddPasskey {
            account,
            credential,
            at,
            ..
        } => {
            if s.credentials.contains_key(&credential.id) {
                return Err("credential already registered");
            }
            let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
            if acc.passkeys.len() >= 10 {
                return Err("an account holds at most 10 passkeys");
            }
            if at > 0 {
                let _prev = acc.passkey_times.insert(
                    credential.id.clone(),
                    crate::types::PasskeyTimes {
                        added_at: Some(at),
                        last_sign_in_at: None,
                    },
                );
            }
            let _prev = s.credentials.insert(credential.id.clone(), account);
            acc.passkeys.push(credential);
            Ok(Value::Null)
        }
        SystemCmd::RemovePasskey {
            account,
            credential_id,
            now,
            ..
        } => {
            let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
            if acc.passkeys.len() <= 1 {
                return Err("cannot remove the last passkey");
            }
            if !acc.passkeys.iter().any(|c| c.id == credential_id) {
                return Err("no such passkey");
            }
            acc.passkeys.retain(|c| c.id != credential_id);
            let _gone = acc.passkey_times.remove(&credential_id);
            let _prev = s.credentials.remove(&credential_id);
            // A removed device keeps no session (Xil's review, issue 5).
            let sessions: BTreeSet<String> = s
                .refresh
                .values()
                .filter(|r| r.account == account && r.passkey.as_deref() == Some(&credential_id))
                .map(|r| r.session.clone())
                .collect();
            for session in sessions {
                s.revoke_session(&session, now);
            }
            Ok(Value::Null)
        }
        SystemCmd::RevokeAllSessions { actor, now } => {
            let n = s.revoke_account_sessions(&actor, now);
            Ok(json!({ "revoked": n }))
        }
        SystemCmd::RefreshReused { hash, now } => {
            let rec = s
                .rotated_refresh
                .get(&hash)
                .cloned()
                .ok_or("unknown refresh token")?;
            s.revoke_session(&rec.session, now);
            Ok(json!({ "revoked": rec.session }))
        }
        SystemCmd::SetSignCount {
            credential_id,
            count,
            ..
        } => {
            let account = s
                .credentials
                .get(&credential_id)
                .cloned()
                .ok_or("unknown credential")?;
            let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
            let cred = acc
                .passkeys
                .iter_mut()
                .find(|c| c.id == credential_id)
                .ok_or("unknown credential")?;
            // Counters only move forward, whatever order logins committed
            // in; a passkey that does not count (0) has nothing to record.
            if count <= cred.sign_count {
                return Err("stale signature counter");
            }
            cred.sign_count = count;
            Ok(Value::Null)
        }
        SystemCmd::CreateApiKey {
            hash,
            record,
            owner_in_org,
            owner_in_team,
            ..
        } => {
            if !s.accounts.contains_key(&record.account) {
                return Err("unknown account");
            }
            if let Some(scope) = &record.upgrades {
                s.upgrade_key_fits_as_logged(
                    &record,
                    scope,
                    OwnerRule::logged(owner_in_org, owner_in_team),
                )?;
            }
            let keys = s
                .api_keys
                .values()
                .filter(|k| k.account == record.account)
                .count();
            if keys >= s.quotas.max_api_keys_per_account as usize {
                return Err("quota exceeded: API keys");
            }
            let id = record.key_id.clone();
            let _prev = s.api_keys.insert(hash, record);
            Ok(json!({ "keyId": id }))
        }
        SystemCmd::RevokeApiKey { key_id, actor, .. } => {
            let before = s.api_keys.len();
            s.api_keys
                .retain(|_, r| !(r.key_id == key_id && r.account == actor));
            if s.api_keys.len() == before {
                return Err("no such key");
            }
            Ok(Value::Null)
        }
        SystemCmd::CreateRefresh { hash, record, .. } => {
            if s.refresh_known(&hash) {
                return Err("refresh token already issued");
            }
            // A sign-in: its passkey's last sign-in is when the session
            // opened (unknown in an earlier release's entry).
            let mut signed_in = false;
            if let (Some(passkey), Some(acc), true) = (
                &record.passkey,
                s.accounts.get_mut(&record.account),
                record.opened_at > 0,
            ) {
                if acc.passkeys.iter().any(|c| &c.id == passkey) {
                    acc.passkey_times
                        .entry(passkey.clone())
                        .or_default()
                        .signed_in(record.opened_at);
                    signed_in = true;
                }
            }
            if let Some(entry) = sign_in_of(&record).filter(|_| signed_in) {
                s.record_activity(&record.account, entry);
            }
            let _prev = s.refresh.insert(hash, record);
            Ok(Value::Null)
        }
        SystemCmd::UseRefresh {
            old_hash,
            new_hash,
            mut record,
            now,
            ..
        } => {
            if s.refresh_known(&new_hash) {
                return Err("refresh token already issued");
            }
            let old = s.refresh.remove(&old_hash).ok_or("unknown refresh token")?;
            if old.expires_at <= now || old.account != record.account {
                return Err("refresh token expired");
            }
            if s.revoked_sessions.contains(&old.session) {
                return Err("session revoked");
            }
            // The session opened when its first token did, whatever the
            // exchange states (an earlier release's states none).
            record.opened_at = old.opened_at;
            let _prev = s.rotated_refresh.insert(old_hash, old);
            let _prev = s.refresh.insert(new_hash, record);
            s.prune_sessions(now);
            Ok(Value::Null)
        }
        SystemCmd::RevokeSession {
            session,
            actor,
            now,
        } => {
            // Only the session's own account revokes it.
            let theirs = !s
                .refresh
                .values()
                .any(|r| r.session == session && r.account != actor);
            if theirs {
                s.revoke_session(&session, now);
            }
            s.prune_sessions(now);
            Ok(Value::Null)
        }
        SystemCmd::SetPasskeyLabel {
            account,
            credential,
            label,
        } => {
            if label.chars().count() > MAX_LABEL_CHARS {
                return Err("a passkey label is at most 64 characters");
            }
            let a = s.accounts.get_mut(&account).ok_or("unknown account")?;
            if !a.passkeys.iter().any(|c| c.id == credential) {
                return Err("no such passkey");
            }
            if label.is_empty() {
                let _gone = a.passkey_labels.remove(&credential);
            } else {
                let _prev = a.passkey_labels.insert(credential, label);
            }
            Ok(Value::Null)
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached accounts::apply"),
    }
}
