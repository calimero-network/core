//! TEE attestation-based admission handler.
//!
//! When a fleet TEE node broadcasts `TeeAttestationAnnounce` on a group topic,
//! existing peers verify the TDX quote against the group's `TeeAdmissionPolicy`
//! and, if valid, admit the node via a `MemberJoinedViaTeeAttestation` governance op.

use calimero_context::group_store::TeeAdmissionPolicy;
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use calimero_tee_attestation::{is_mock_quote, verify_attestation, verify_mock_attestation};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

/// Handle a `TeeAttestationAnnounce` broadcast on a group gossip topic.
///
/// 1. Loads the group's TEE admission policy from the governance DAG
/// 2. Verifies the TDX quote (or mock) against the policy
/// 3. Validates measurements (MRTD, RTMR0-3) and TCB status against allowlists
/// 4. If valid, signs and publishes `MemberJoinedViaTeeAttestation` governance op
pub async fn handle_tee_attestation_announce(
    datastore: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    source: libp2p::PeerId,
    quote_bytes: Vec<u8>,
    public_key: PublicKey,
    nonce: [u8; 32],
    group_id_bytes: [u8; 32],
) -> eyre::Result<()> {
    let group_id = ContextGroupId::from(group_id_bytes);

    let policy = match calimero_context::group_store::read_tee_admission_policy(datastore, &group_id)? {
        Some(p) => p,
        None => {
            debug!(?group_id, "No TEE admission policy, ignoring TeeAttestationAnnounce");
            return Ok(());
        }
    };

    let is_mock = is_mock_quote(&quote_bytes);
    if is_mock && !policy.accept_mock {
        warn!("Mock TEE attestation rejected by policy");
        return Ok(());
    }

    let verification_result = if is_mock {
        warn!("Verifying MOCK attestation for TEE admission");
        verify_mock_attestation(&quote_bytes, &nonce, None)?
    } else {
        verify_attestation(&quote_bytes, &nonce, None).await?
    };

    if !verification_result.is_valid() {
        warn!(
            %source,
            quote_verified = verification_result.quote_verified,
            nonce_verified = verification_result.nonce_verified,
            "TEE attestation verification failed"
        );
        return Ok(());
    }

    if let Some(ref quote) = verification_result.quote {
        for (allowlist, actual, label) in [
            (&policy.allowed_mrtd, &quote.body.mrtd, "MRTD"),
            (&policy.allowed_rtmr0, &quote.body.rtmr0, "RTMR0"),
            (&policy.allowed_rtmr1, &quote.body.rtmr1, "RTMR1"),
            (&policy.allowed_rtmr2, &quote.body.rtmr2, "RTMR2"),
            (&policy.allowed_rtmr3, &quote.body.rtmr3, "RTMR3"),
        ] {
            if !allowlist.is_empty() && !allowlist.iter().any(|a| a == actual) {
                warn!(%source, register = label, actual_value = actual, "Measurement not in policy allowlist");
                return Ok(());
            }
        }
    }

    let tcb_status = verification_result
        .tcb_status
        .clone()
        .unwrap_or_else(|| "Unknown".to_owned());

    if !policy.allowed_tcb_statuses.is_empty()
        && !policy.allowed_tcb_statuses.iter().any(|s| s == &tcb_status)
    {
        warn!(
            %source,
            %tcb_status,
            allowed = ?policy.allowed_tcb_statuses,
            "TCB status not in policy allowlist"
        );
        return Ok(());
    }

    let quote_hash: [u8; 32] = Sha256::digest(&quote_bytes).into();

    if calimero_context::group_store::check_group_membership(datastore, &group_id, &public_key)? {
        debug!(%source, %public_key, ?group_id, "TEE node already a group member, skipping");
        return Ok(());
    }

    if calimero_context::group_store::is_quote_hash_used(datastore, &group_id, &quote_hash)? {
        warn!(
            %source,
            quote_hash = %hex::encode(quote_hash),
            "TEE attestation quote already used (replay), rejecting"
        );
        return Ok(());
    }

    let mrtd = verification_result
        .quote
        .as_ref()
        .map(|q| q.body.mrtd.clone())
        .unwrap_or_default();

    info!(
        %source, %public_key, ?group_id, %mrtd, %tcb_status, is_mock,
        "TEE attestation verified, publishing MemberJoinedViaTeeAttestation"
    );

    use calimero_primitives::identity::PrivateKey;

    let (_signer_pk, signing_key) = get_group_signing_key(datastore, &group_id)?;
    let sk = PrivateKey::from(signing_key);

    calimero_context::group_store::sign_apply_and_publish(
        datastore,
        node_client,
        &group_id,
        &sk,
        GroupOp::MemberJoinedViaTeeAttestation {
            member: public_key,
            quote_hash,
            mrtd,
            tcb_status,
            role: GroupMemberRole::Member,
        },
    )
    .await?;

    info!(%public_key, ?group_id, "TEE node admitted via attestation policy");
    Ok(())
}

/// Find a signing key for the current node in the given group.
///
/// Iterates signing keys for the group and returns the first one found.
/// In practice each node has one identity per group.
fn get_group_signing_key(
    datastore: &Store,
    group_id: &ContextGroupId,
) -> eyre::Result<(PublicKey, [u8; 32])> {
    use calimero_store::key::{GroupSigningKey, GROUP_SIGNING_KEY_PREFIX};

    let handle = datastore.handle();
    let start = GroupSigningKey::new(group_id.to_bytes(), [0u8; 32].into());
    let mut iter = handle.iter::<GroupSigningKey>()?;
    let first = iter.seek(start).transpose();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_SIGNING_KEY_PREFIX {
            break;
        }
        if key.group_id() != group_id.to_bytes() {
            break;
        }
        if let Some(value) = handle.get(&key)? {
            let pk = PublicKey::from(*key.public_key());
            return Ok((pk, value.private_key));
        }
    }

    eyre::bail!("no signing key found for group {group_id:?} — node must be a group member")
}
