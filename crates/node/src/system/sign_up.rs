//! System commands for the sign-up policy and sign-up invites.
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

// This area's refusals. Each is `400 REJECTED`, the kind the bare message it
// names was answered with; the message is what a client reads.

/// A sign-up invite whose issuer is not the actor.
const INVITE_NAMES_ISSUER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a sign-up invite names who issued it",
);
/// An invite key that is not 32 bytes of hex.
const BAD_INVITE_KEY: Rejection =
    Rejection::new(RejectionKind::Rejected, "an invite key is 32 bytes, hex");
/// A sign-up invite outside 1 to [`MAX_SIGN_UP_INVITE_USES`] uses.
const BAD_INVITE_USES: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a sign-up invite creates 1 to 10000 accounts",
);
/// A sign-up invite that expires no later than it is made.
const INVITE_EXPIRES_BEFORE_MADE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a sign-up invite expires after it is made",
);
/// A revocation of a sign-up invite the cluster does not keep.
const NO_SUCH_SIGN_UP_INVITE: Rejection =
    Rejection::new(RejectionKind::Rejected, "no such sign-up invite");

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    match cmd {
        SystemCmd::SetSignUp {
            policy,
            actor,
            proof,
            consent,
        } => sign_up_consent(
            proofs,
            actor,
            proof.as_ref(),
            consent.as_ref(),
            "__sign_up",
            &json!({ "policy": policy }),
        ),
        SystemCmd::CreateSignUpInvite {
            invite_key,
            invite,
            actor,
            proof,
            consent,
        } => sign_up_consent(
            proofs,
            actor,
            proof.as_ref(),
            consent.as_ref(),
            "__create_sign_up_invite",
            &sign_up_invite_args(invite_key, invite),
        ),
        SystemCmd::RevokeSignUpInvite {
            invite_key,
            actor,
            proof,
            consent,
        } => sign_up_consent(
            proofs,
            actor,
            proof.as_ref(),
            consent.as_ref(),
            "__revoke_sign_up_invite",
            &json!({ "inviteKey": invite_key }),
        ),
        _ => Ok(None),
    }
}

/// The proof of whoever runs the cluster's sign-up (`Proofs::sign_up_proof`:
/// the operator's signature, or the owner's passkey), stating `method` on the
/// cluster with exactly the arguments `stated`.
fn sign_up_consent(
    proofs: &Proofs<'_>,
    actor: &str,
    proof: Option<&Value>,
    consent: Option<&Value>,
    method: &str,
    stated: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.sign_up_proof(actor, proof, consent)?;
    proofs.states(&statement, "cluster", method, &|args| args == stated)?;
    Ok(Some(used))
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let state = &mut m.state;
    match cmd {
        SystemCmd::SetSignUp { policy, actor, .. } => set_sign_up(state, policy, &actor),
        SystemCmd::CreateSignUpInvite {
            invite_key,
            invite,
            actor,
            ..
        } => create_sign_up_invite(state, invite_key, invite, &actor),
        SystemCmd::RevokeSignUpInvite {
            invite_key, actor, ..
        } => revoke_sign_up_invite(state, invite_key, &actor),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached sign_up::apply"),
    }
}

/// Set the cluster's sign-up policy: whoever runs its sign-up.
fn set_sign_up(state: &mut SystemState, policy: SignUp, actor: &str) -> Result<Value, Rejection> {
    state.runs_sign_up(actor)?;
    let governance = state.governance.as_mut().ok_or(NO_GOVERNANCE)?;
    governance.sign_up = policy;
    Ok(json!({ "governance": governance }))
}

/// Create a sign-up invite: whoever runs the cluster's sign-up, under a new
/// key, within the cluster's bound and an invite's uses and life.
fn create_sign_up_invite(
    state: &mut SystemState,
    invite_key: String,
    invite: SignUpInvite,
    actor: &str,
) -> Result<Value, Rejection> {
    state.runs_sign_up(actor)?;
    if invite.created_by != actor {
        return Err(INVITE_NAMES_ISSUER);
    }
    hex32(&invite_key).ok_or(BAD_INVITE_KEY)?;
    if state.invites.contains_key(&invite_key) || state.sign_up_invites.contains_key(&invite_key) {
        return Err(INVITE_EXISTS);
    }
    if state.sign_up_invites.len() >= MAX_SIGN_UP_INVITES {
        return Err(Rejection::quota("sign-up invites"));
    }
    if invite.uses_left == 0 || invite.uses_left > MAX_SIGN_UP_INVITE_USES {
        return Err(BAD_INVITE_USES);
    }
    if invite.expires_at <= invite.created_at {
        return Err(INVITE_EXPIRES_BEFORE_MADE);
    }
    let _prev = state.sign_up_invites.insert(invite_key.clone(), invite);
    Ok(json!({ "inviteKey": invite_key }))
}

/// Revoke a sign-up invite: whoever runs the cluster's sign-up.
fn revoke_sign_up_invite(
    state: &mut SystemState,
    invite_key: String,
    actor: &str,
) -> Result<Value, Rejection> {
    state.runs_sign_up(actor)?;
    let _gone = state
        .sign_up_invites
        .remove(&invite_key)
        .ok_or(NO_SUCH_SIGN_UP_INVITE)?;
    Ok(json!({ "inviteKey": invite_key }))
}
