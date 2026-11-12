//! System commands for the sign-up policy and sign-up invites.
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
    match cmd {
        SystemCmd::SetSignUp {
            policy,
            actor,
            proof,
            consent,
        } => {
            let (st, used) = proofs.sign_up_proof(actor, proof.as_ref(), consent.as_ref())?;
            let args = json!({ "policy": policy });
            proofs.states(&st, "cluster", "__sign_up", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::CreateSignUpInvite {
            invite_key,
            invite,
            actor,
            proof,
            consent,
        } => {
            let (st, used) = proofs.sign_up_proof(actor, proof.as_ref(), consent.as_ref())?;
            let args = sign_up_invite_args(invite_key, invite);
            proofs.states(&st, "cluster", "__create_sign_up_invite", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RevokeSignUpInvite {
            invite_key,
            actor,
            proof,
            consent,
        } => {
            let (st, used) = proofs.sign_up_proof(actor, proof.as_ref(), consent.as_ref())?;
            let args = json!({ "inviteKey": invite_key });
            proofs.states(&st, "cluster", "__revoke_sign_up_invite", &|a| *a == args)?;
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
        SystemCmd::SetSignUp { policy, actor, .. } => {
            s.runs_sign_up(&actor)?;
            let g = s
                .governance
                .as_mut()
                .ok_or("the cluster has no governance yet")?;
            g.sign_up = policy;
            Ok(json!({ "governance": g }))
        }
        SystemCmd::CreateSignUpInvite {
            invite_key,
            invite,
            actor,
            ..
        } => {
            s.runs_sign_up(&actor)?;
            if invite.created_by != actor {
                return Err("a sign-up invite names who issued it");
            }
            hex32(&invite_key).ok_or("an invite key is 32 bytes, hex")?;
            if s.invites.contains_key(&invite_key) || s.sign_up_invites.contains_key(&invite_key) {
                return Err("invite exists");
            }
            if s.sign_up_invites.len() >= MAX_SIGN_UP_INVITES {
                return Err("quota exceeded: sign-up invites");
            }
            if invite.uses_left == 0 || invite.uses_left > MAX_SIGN_UP_INVITE_USES {
                return Err("a sign-up invite creates 1 to 10000 accounts");
            }
            if invite.expires_at <= invite.created_at {
                return Err("a sign-up invite expires after it is made");
            }
            let _prev = s.sign_up_invites.insert(invite_key.clone(), invite);
            Ok(json!({ "inviteKey": invite_key }))
        }
        SystemCmd::RevokeSignUpInvite {
            invite_key, actor, ..
        } => {
            s.runs_sign_up(&actor)?;
            let _gone = s
                .sign_up_invites
                .remove(&invite_key)
                .ok_or("no such sign-up invite")?;
            Ok(json!({ "inviteKey": invite_key }))
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached sign_up::apply"),
    }
}
