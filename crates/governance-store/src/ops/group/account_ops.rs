//! Apply handlers for the account plane: device link, device revocation, and
//! account root-key rotation.
//!
//! The three share a file because they share one invariant, and separating them
//! would let it drift: **every one of them must be idempotent and
//! order-independent**. The apply pipeline re-runs a mutation before the op-log
//! dedup gate fires, and governance ops arrive in whatever order the DAG hands
//! them over, so a handler that only works on first application or only in
//! causal order will silently diverge two replicas.
//!
//! [`crate::AccountBindingRepository`] carries the actual rules; these handlers
//! are the authorization gate plus the call.

use super::context::GroupApplyCtx;
use crate::authorizer::AtCutMembershipPath;
use crate::membership::MembershipPath;
use crate::{AccountBindingRepository, BindingRejected, MembershipRepository};
use calimero_account::{
    AccountGenesis, AccountId, AccountMemberEndorsement, DeviceCert, DeviceId, RootKeyHandoff,
    SignedDeviceRevocation,
};
use eyre::Result as EyreResult;

/// `GroupOp::AccountDeviceLinked` — record a device as speaking for an account.
///
/// Authorization is two questions, and it takes both: **did a granted member
/// endorse this account, and is that endorser a member at this cut?**
///
/// It used to ask whether the account's own root key was a member, which worked
/// only while accounts were rooted at the node's namespace identity. The root is
/// now a dedicated offline key — that is what lets it survive losing every device
/// and certify a replacement — and such a key is a member nowhere. So a granted
/// member key signs the account id instead, and the gate checks the endorser.
///
/// Equally strong: only a member can produce a valid endorsement, and only the root
/// holder can certify a device into the account. Neither alone enrolls anything.
/// Anyone may endorse an account they do not own — ids are public — and it gains
/// them nothing for exactly that reason.
///
/// Note what is *not* checked: whether the signer is the device being enrolled.
/// The certificate is root-signed and the credential is self-certifying, so a
/// replayed link records a binding the account had already authorized — it
/// gains an attacker nothing, and refusing it would break legitimate re-gossip
/// of the same op.
pub(crate) fn apply_device_linked(
    ctx: &mut GroupApplyCtx<'_>,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
    endorsement: &AccountMemberEndorsement,
) -> EyreResult<()> {
    let group_id = *ctx.group_id();

    // The policy gate, in three steps: the endorsement is about this account, it
    // is validly signed, and its signer is a member at the cut. Membership rows
    // are keyed by member key and an `AccountId` is a one-way hash, so the account
    // cannot be looked up in them at all — the endorsement is what bridges the
    // two, and the only thing that can.
    //
    // Membership is resolved at the op's causal cut, like every other apply-time
    // authority question. Reading live rows would decide against whatever this
    // replica has folded so far, so a node that had already applied a concurrent
    // removal of the endorser would refuse a link its peers recorded — and since a
    // refusal writes nothing while the op still occupies its place in the DAG, the
    // two would disagree about who may author with no later op to reconcile them.
    //
    // Step one: the endorsement must be about THIS account, or a valid endorsement
    // of some other account could be presented alongside an unrelated credential.
    if endorsement.account != cert.account {
        log_refusal(
            &group_id,
            "device link",
            &BindingRejected::EndorsementAccountMismatch,
        );
        return Ok(());
    }
    // ...and be validly signed by the key it names. Cheap, self-contained, and
    // checked before the at-cut membership question so a forged endorsement costs
    // no fold work.
    // Shadowed by the verified form deliberately: the membership question below
    // reads the endorser's key, and reading it off the wrapper means it cannot be
    // read off a struct whose signature was never checked.
    let Ok(endorsement) = endorsement.verify() else {
        log_refusal(
            &group_id,
            "device link",
            &BindingRejected::EndorsementInvalid,
        );
        return Ok(());
    };
    if !endorser_is_member(ctx, &endorsement.member)? {
        log_refusal(&group_id, "device link", &BindingRejected::AccountNotMember);
        return Ok(());
    }
    let store = ctx.store();
    let bindings = AccountBindingRepository::new(store);

    let outcome = bindings.apply_link(&group_id, genesis, chain, cert)?;

    // Record the vouch even when the link itself is refused, for the same reason
    // the genesis is absorbed unconditionally: the endorsement is self-certifying
    // and was verified above, so accepting it is safe, and making it conditional on
    // an ORDER-DEPENDENT outcome (revoked, epoch not advanced) would let two
    // arrival orders leave different endorser sets behind.
    //
    // But not for a credential that can never succeed — see
    // [`BindingRejected::is_permanent`], which both endorser-recording paths
    // share so the two cannot drift into disagreeing about which refusals are
    // terminal.
    let credential_can_never_succeed = outcome
        .as_ref()
        .err()
        .is_some_and(BindingRejected::is_permanent);
    if !credential_can_never_succeed {
        // The endorsement names the SIGNING KEY that made it — a signature can
        // name nothing else — but the row records the account that key speaks
        // for, because that is what a membership check consults. `endorser_is_member`
        // above already refused an unresolvable key, so this resolves.
        if let Some(endorser) =
            crate::member_account_in_namespace(ctx.store(), &group_id, &endorsement.member)?
        {
            bindings.record_endorser(&group_id, cert.account, &endorser)?;
        }
    }

    match outcome {
        Ok(binding) => {
            remember_if_this_accounts_own(ctx, genesis, chain, cert);
            tracing::info!(
                group_id = ?group_id,
                account = %binding.account,
                device = %binding.device,
                device_epoch = binding.device_epoch,
                "account device linked"
            );
            // The device saw the group's earlier context registrations as
            // nobody; the sweep a new member row starts catches it up.
            if let Some(event) =
                crate::build_auto_follow_set_if_enabled(ctx.store(), &group_id, &binding.account)?
            {
                ctx.queue_event(event);
            }
        }
        Err(reason) => {
            // Deterministically inadmissible: every replica reaches the same
            // verdict from the same rows, so declining to record it keeps the
            // group convergent. The op still occupies its place in the DAG.
            log_refusal(&group_id, "device link", &reason);
        }
    }
    Ok(())
}

/// Cache a certificate this node's OWN account root signed, wherever it applied
/// from.
///
/// The multi-holder case: a second holder device certified a third, and this node
/// learns of it only here. Without the cache, a namespace this node gains later
/// would have no way to bind that device - the replicated binding row drops the
/// root signature, so the certificate cannot be rebuilt from folded state.
///
/// Read-only on the root, never `ensure_account_root`: an apply path must not
/// mint a key as a side effect of folding somebody else's op. A node holding no
/// root owns no account and so can own no certificate here.
///
/// Failures are logged rather than propagated. The cache is an optimisation over
/// re-pairing; refusing an op the group accepted because a node-local row could
/// not be written would diverge this replica from its peers.
fn remember_if_this_accounts_own(
    ctx: &GroupApplyCtx<'_>,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) {
    let devices = crate::NodeDeviceRepository::new(ctx.store());
    let own = match devices.account_root() {
        Ok(Some(root)) => root.account(),
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(%err, "could not read this node's account root while folding a link");
            return;
        }
    };
    if own != cert.account {
        return;
    }
    let proof = calimero_account::AccountProof {
        genesis: *genesis,
        chain: chain.to_vec(),
        statement: *cert,
    };
    if let Err(err) = devices.remember_device_cert_if_new(&proof) {
        tracing::warn!(device = %cert.device, %err,
                       "could not remember a certificate this account signed");
    }
}

/// `GroupOp::AccountDeviceUnlinked` — withdraw a device.
///
/// Two ways to be authorized, and a revocation needs exactly one of them.
///
/// **A group admin at the op's cut.** A revocation is terminal — the `DeviceId`
/// is spent for good — so an ungated one is a permanent denial of service any
/// member could inflict on any other. Membership in a group is not authority
/// over other members' devices. This is the path that ejects a device whose
/// account holder is unreachable.
///
/// **Or a root-signed proof that the account withdrew its own device.** The
/// lost-laptop case, where the owner may be the only person who knows. It is
/// deliberately NOT a gate on folded state: "is the signer this account's
/// current root key" depends on which rotations this replica has folded, so two
/// replicas would decide one op differently and disagree permanently about who
/// may author. The proof travels with the op and verifies from the account id
/// alone, exactly as a `DeviceCert` does — so every replica reaches the same
/// verdict regardless of what it has folded.
///
/// The proof is checked FIRST, and deliberately: it costs no fold work, so a
/// self-service revocation never has to resolve the cut at all, and can therefore
/// never park on `AuthorityUndecidable` for authority it does not need.
///
/// Applied unconditionally once authorized, including for a device this group
/// has never seen linked: the tombstone is what a later link consults, so
/// dropping an early revocation would make the outcome depend on arrival order.
pub(crate) fn apply_device_unlinked(
    ctx: &mut GroupApplyCtx<'_>,
    account: &AccountId,
    device: &DeviceId,
    proof: Option<&SignedDeviceRevocation>,
) -> EyreResult<()> {
    let group_id = *ctx.group_id();

    // A proof establishes that its signer holds the root of the account it NAMES —
    // and nothing else. The `DeviceId` inside is whatever the signer chose; owning
    // that device is not a precondition of signing a statement about it. So the
    // proof alone is not self-service authority: an attacker holding any account
    // root could name their own account beside somebody else's device and spend
    // that replica id for good, since a tombstone is terminal. That is the same
    // hole the admin path was gated for, arriving by the other door.
    //
    // The stored binding is what ties the two together. `authz`'s unified-plane
    // arm already requires it; this is the governance path catching up.
    //
    // A device with NO binding here is deliberately not a refusal — an admin must
    // still be able to eject one whose link this replica has not folded. It only
    // means the self-service claim cannot be checked, so it does not authorize, and
    // the admin gate below decides.
    let device_belongs_to_account = AccountBindingRepository::new(ctx.store())
        .raw_binding(&group_id, *device)?
        .is_some_and(|binding| binding.account == *account.as_bytes());

    let self_service = match proof {
        Some(_) if !device_belongs_to_account => {
            tracing::warn!(
                group_id = ?group_id,
                account = %account,
                device = %device,
                "account device unlink: revocation proof names an account this group \
                 does not have this device bound to; falling back to the admin gate"
            );
            false
        }
        Some(proof) => match proof.authorises(*account, *device) {
            Ok(_) => true,
            Err(err) => {
                // A proof that does not verify is a deterministic refusal, not an
                // error: every replica reaches it identically from the op alone.
                // Falling through to the admin path would be wrong — an attacker
                // could attach a garbage proof to an op they were not otherwise
                // entitled to author and learn nothing, but a *valid admin* whose
                // proof was malformed should still see why.
                tracing::warn!(
                    group_id = ?group_id,
                    account = %account,
                    device = %device,
                    %err,
                    "account device unlink: revocation proof did not verify"
                );
                false
            }
        },
        None => false,
    };

    // `?` rather than a swallowed `false`: `is_admin` returns
    // `AuthorityUndecidable` when the op's cut is real but unfolded here, and that
    // must park the apply for retry rather than be read as "not an admin". A
    // genuine non-admin is a deterministic refusal every replica reaches
    // identically, so it records nothing and returns `Ok` — erroring would stall
    // the apply forever on an op that can never succeed.
    if !self_service && !ctx.permissions().is_admin(ctx.signer())? {
        tracing::warn!(
            group_id = ?group_id,
            signer = %ctx.signer(),
            account = %account,
            device = %device,
            "account device unlink not recorded: signer is neither an admin at the \
             op's cut nor the holder of a valid revocation proof"
        );
        return Ok(());
    }

    AccountBindingRepository::new(ctx.store()).apply_revocation(&group_id, *device)?;

    // Record the rotation this revocation owes, on EVERY node that folds it, so
    // the worklist is replicated rather than gossiped and any admin may discharge
    // it. Marked unconditionally: when an admin published the revocation, its own
    // rotation sidecar rides the same op and clears this row immediately after, so
    // the alternative — trying to detect here whether a rotation came along — buys
    // nothing and gets the un-rotated case wrong if it guesses.
    //
    // Without this the debt is simply lost. The revoked device stops WRITING at
    // once, but it keeps the key it already holds, so it can keep READING until
    // someone rotates for an unrelated reason.
    crate::PendingDeviceRotationRepository::new(ctx.store()).mark(&group_id, device)?;
    ctx.queue_event(crate::op_events::OpEvent::DeviceRevoked {
        group_id: group_id.to_bytes(),
        account: *account,
        device: *device,
    });

    tracing::info!(
        group_id = ?group_id,
        account = %account,
        device = %device,
        self_service,
        "account device unlinked"
    );
    Ok(())
}

/// Is `endorser` a member of this group at the op's causal cut?
///
/// The key asked about is the **endorser's**, never the account root: the root is
/// a dedicated offline key and is a member nowhere, so asking about it would
/// refuse every link. That is why the link carries an endorsement at all.
///
/// Direct or inherited both count: a member who reaches the group through an
/// Open-subgroup chain holds every right the endorsed account's devices would
/// gain, which is the whole basis for the link needing no admin.
///
/// The live resolver is used only when the projection has no cut to resolve
/// against at all, and `ensure_live_fallback_is_sound` is what separates that
/// from an unfolded cut — where falling back would answer against a different
/// cut and let two replicas decide the same op differently.
/// A refusal here is reported with **both** verdicts, because "the projection says
/// no while live says yes" is not a detail — it is the signature of a divergence.
/// The publisher applies its own op through the live resolver, so a receiver whose
/// projection disagrees records nothing for an op the publisher recorded, and the
/// two `scope_root`s part company with no later op able to reconcile them. Logging
/// only "not a member" leaves that indistinguishable from an ordinary refusal.
fn endorser_is_member(
    ctx: &GroupApplyCtx<'_>,
    endorser: &calimero_primitives::identity::PublicKey,
) -> EyreResult<bool> {
    // The endorsement names a member KEY, but membership is recorded against
    // the account it speaks for, so resolve before asking either plane. An
    // endorser whose key is bound to no account here vouches for nobody — the
    // same refusal an unknown key gets, reached one step earlier.
    let endorser_key = *endorser;
    let Some(endorser) = crate::member_account_in_namespace(ctx.store(), ctx.group_id(), endorser)?
    else {
        // "Bound to no account" is a LIVE answer — binding rows are plane state
        // that a replica folds like any other, so an endorser this node cannot
        // resolve yet may be one whose link simply has not arrived. Returning a
        // refusal here would decide from live rows exactly where the at-cut gate
        // below refuses to, and two replicas at different fold depths would
        // record different outcomes for the same op. So a live answer is only
        // permitted where a live answer is sound at all; otherwise park.
        ctx.ensure_live_fallback_is_sound(&endorser_key)?;
        return Ok(false);
    };
    let endorser = &endorser;
    let projected = ctx.projection_membership_path(endorser);
    let path = match projected {
        Some(projected) => projected,
        None => {
            ctx.ensure_live_fallback_is_sound(&endorser_key)?;
            match MembershipRepository::new(ctx.store()).check_path(ctx.group_id(), endorser)? {
                MembershipPath::None => AtCutMembershipPath::None,
                MembershipPath::Direct => AtCutMembershipPath::Direct,
                MembershipPath::Inherited { .. } => AtCutMembershipPath::Inherited,
            }
        }
    };

    let is_member = path != AtCutMembershipPath::None;
    if !is_member {
        // Read live too, purely to classify the refusal. Best-effort: a store
        // fault here must not turn a decided refusal into an error.
        let live = MembershipRepository::new(ctx.store())
            .check_path(ctx.group_id(), endorser)
            .ok();
        let live_is_member = live.map(|path| path != MembershipPath::None);
        tracing::warn!(
            group_id = ?ctx.group_id(),
            %endorser,
            verdict = if projected.is_some() { "projection" } else { "live-fallback" },
            ?live_is_member,
            cut_len = ctx.cut().len(),
            cut_head = ?ctx.cut().first().map(hex::encode),
            divergence_risk = projected.is_some() && live_is_member == Some(true),
            "endorser is not a member at this op's cut"
        );
    }
    Ok(is_member)
}

/// `GroupOp::AccountKeysRotated` — roll an account's root key.
///
/// Raises the account's epoch, after which certificates signed by any
/// superseded key stop being honoured. The handoff's own signature is verified
/// by the repository against the key currently in force, so a rotation cannot
/// be forged by anyone who does not hold that key.
///
/// Deliberately ungated, unlike its two siblings, and for a reason rather than by
/// omission: the handoff is self-certifying against state the group already holds.
/// A rotation for an account this group has never learned is refused outright
/// (`RotationAccountUnknown` — there is no chain to be discontinuous with), and the
/// only way the group learns an account is through a link that already passed the
/// membership gate. So relaying someone else's rotation writes nothing an
/// attacker chose, and gating it on the relayer would only break legitimate
/// re-gossip.
pub(crate) fn apply_keys_rotated(
    ctx: &mut GroupApplyCtx<'_>,
    handoff: &RootKeyHandoff,
) -> EyreResult<()> {
    let group_id = *ctx.group_id();
    let store = ctx.store();

    match AccountBindingRepository::new(store).apply_rotation(&group_id, handoff)? {
        Ok(()) => {
            tracing::info!(
                group_id = ?group_id,
                account = %handoff.account,
                from_epoch = handoff.from_epoch,
                "account root key rotated"
            );
        }
        Err(reason) => log_refusal(&group_id, "key rotation", &reason),
    }
    Ok(())
}

/// Log an inadmissible credential at `warn`.
///
/// Not an error: the op is validly signed and belongs in the DAG, it simply
/// records nothing. Returning `Err` would stall the apply and burn no nonce,
/// leaving the node retrying an op that can never succeed.
fn log_refusal(
    group_id: &calimero_context_config::types::ContextGroupId,
    what: &str,
    reason: &BindingRejected,
) {
    tracing::warn!(
        group_id = ?group_id,
        %reason,
        "account {what} not recorded"
    );
}
