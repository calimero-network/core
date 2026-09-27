//! A TEE's signed statement that it ran a trigger whose run wrote nothing
//! (`BroadcastMessage::TeeFired`).
//!
//! Such a run produces no delta, so no TEE envelope for receivers
//! to record the firing from. Without this, each later-ranked TEE would run the
//! trigger too, a turn apart. The statement is checked like that envelope: the
//! signature, then that the key is an attested TEE's for the context, and a
//! timer's tick is recorded only once it has begun.

use calimero_context_client::tee_trigger;
use calimero_node_primitives::sync::delta_auth::{verify_tee_fired, TeeTriggerCause};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tracing::{debug, warn};

use crate::NodeManager;

/// Why a fired statement was not recorded.
#[derive(Debug)]
enum TeeFiredRefusal {
    /// The signature does not verify under the named key.
    Signature(eyre::Report),
    /// The key is not an attested TEE's for the context: only a TEE's firing
    /// may stand the others down.
    NotAnAttestedTee,
}

impl core::fmt::Display for TeeFiredRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Signature(err) => write!(f, "{err}"),
            Self::NotAnAttestedTee => f.write_str("the signer is not an attested TEE"),
        }
    }
}

pub(super) fn handle_tee_fired(
    manager: &NodeManager,
    source: libp2p::PeerId,
    context_id: ContextId,
    author_id: PublicKey,
    trigger: &TeeTriggerCause,
    signature: &[u8; 64],
) {
    let store = manager.clients.context.datastore();
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let attested = || {
        calimero_governance_store::is_attested_tee_key_for_context(store, &context_id, &author_id)
            .unwrap_or(false)
    };
    match accept_tee_fired(
        store, context_id, author_id, trigger, signature, attested, now_secs,
    ) {
        Ok(()) => {
            debug!(%context_id, %author_id, tee_method = trigger.method(), "Recorded a TEE firing that wrote nothing");
        }
        Err(refusal) => {
            warn!(%context_id, %author_id, %source, %refusal, "Ignoring a TEE fired statement");
        }
    }
}

/// Check a fired statement and, if it holds, record its trigger as fired.
///
/// # Errors
/// The rule the statement breaks. A store error is logged and swallowed: a
/// marker that is not recorded costs at most a duplicate firing.
fn accept_tee_fired(
    store: &Store,
    context_id: ContextId,
    author_id: PublicKey,
    trigger: &TeeTriggerCause,
    signature: &[u8; 64],
    attested: impl FnOnce() -> bool,
    now_secs: u64,
) -> Result<(), TeeFiredRefusal> {
    verify_tee_fired(context_id, author_id, trigger, signature)
        .map_err(TeeFiredRefusal::Signature)?;
    if !attested() {
        return Err(TeeFiredRefusal::NotAnAttestedTee);
    }
    if let Err(err) = tee_trigger::record_tee_firing(store, &context_id, trigger, now_secs) {
        warn!(%context_id, error = %err, "Failed to record a TEE firing");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_node_primitives::sync::delta_auth::tee_fired_payload;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;

    use super::*;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn resolve() -> TeeTriggerCause {
        TeeTriggerCause::Event {
            cause: [3; 32],
            method: "resolve".to_owned(),
        }
    }

    fn sign(ctx: ContextId, sk: &PrivateKey, trigger: &TeeTriggerCause) -> [u8; 64] {
        let payload = tee_fired_payload(ctx, sk.public_key(), trigger).unwrap();
        sk.sign(&payload).unwrap().to_bytes()
    }

    fn fired(store: &Store, ctx: &ContextId, trigger: &TeeTriggerCause) -> bool {
        tee_trigger::tee_fired(store, ctx, &trigger.id(ctx)).unwrap()
    }

    #[test]
    fn an_attested_tees_statement_marks_its_trigger_fired() {
        let (store, ctx, sk) = (store(), ContextId::from([1; 32]), PrivateKey::from([5; 32]));
        let sig = sign(ctx, &sk, &resolve());
        accept_tee_fired(&store, ctx, sk.public_key(), &resolve(), &sig, || true, 0).unwrap();
        assert!(fired(&store, &ctx, &resolve()));
    }

    #[test]
    fn a_statement_from_a_key_that_is_not_an_attested_tee_marks_nothing() {
        let (store, ctx, sk) = (store(), ContextId::from([1; 32]), PrivateKey::from([5; 32]));
        let sig = sign(ctx, &sk, &resolve());
        assert!(matches!(
            accept_tee_fired(&store, ctx, sk.public_key(), &resolve(), &sig, || false, 0),
            Err(TeeFiredRefusal::NotAnAttestedTee)
        ));
        assert!(!fired(&store, &ctx, &resolve()));
    }

    #[test]
    fn a_statement_signed_for_another_trigger_or_by_another_key_marks_nothing() {
        let (store, ctx, sk) = (store(), ContextId::from([1; 32]), PrivateKey::from([5; 32]));
        let other = TeeTriggerCause::Event {
            cause: [4; 32],
            method: "resolve".to_owned(),
        };
        let sig = sign(ctx, &sk, &other);
        assert!(matches!(
            accept_tee_fired(&store, ctx, sk.public_key(), &resolve(), &sig, || true, 0),
            Err(TeeFiredRefusal::Signature(_))
        ));
        let sig = sign(ctx, &sk, &resolve());
        let someone_else = PrivateKey::from([6; 32]).public_key();
        assert!(matches!(
            accept_tee_fired(&store, ctx, someone_else, &resolve(), &sig, || true, 0),
            Err(TeeFiredRefusal::Signature(_))
        ));
        assert!(!fired(&store, &ctx, &resolve()));
    }

    #[test]
    fn a_statement_for_a_tick_that_has_not_begun_marks_nothing() {
        let (store, ctx, sk) = (store(), ContextId::from([1; 32]), PrivateKey::from([5; 32]));
        let tick = |tick| TeeTriggerCause::Timer {
            method: "sweep".to_owned(),
            tick,
            every_secs: 60,
        };
        // Tick 10 begins at 600s; tick 12 at 720s.
        let sig = sign(ctx, &sk, &tick(12));
        accept_tee_fired(&store, ctx, sk.public_key(), &tick(12), &sig, || true, 600).unwrap();
        assert!(!fired(&store, &ctx, &tick(12)));
        let sig = sign(ctx, &sk, &tick(10));
        accept_tee_fired(&store, ctx, sk.public_key(), &tick(10), &sig, || true, 600).unwrap();
        assert!(fired(&store, &ctx, &tick(10)));
    }
}
