//! Availability prefetch: react to "I now hold this blob for this context".
//!
//! An availability node that replicates a context's state but not the bytes
//! that state references is not delivering availability — it can answer a probe
//! honestly and still always answer "no". This is the path that makes the
//! answer usually "yes".
//!
//! ## Why the announce site and not state enumeration
//!
//! The obvious design — "on sync, fetch every blob the context's state
//! references" — is not implementable: `BlobMeta` is keyed by blob id alone
//! with no context column, and state deltas carry opaque borsh values with no
//! type tag, so a `BlobRef` sitting inside application state is unrecognisable
//! at apply time. The blob→context association exists in exactly one place: the
//! moment a producer announces it. So the announce is what drives prefetch.
//!
//! ## Known gap: an anchor that is offline at announce time
//!
//! There is no catch-up path. An availability node that is down when a blob is
//! announced never learns about that blob, and nothing re-announces it later.
//! Correctness is unaffected — the blob stays findable by probing its original
//! holder — but availability for that one blob degrades to "the uploader must
//! be online" until something announces it again. Closing this wants its own
//! design (a per-context announce log, or a digest exchanged at sync), not a
//! retry bolted on here.

use core::time::Duration;

use calimero_context_client::client::ContextClient;
use calimero_governance_store::{get_group_for_context, MembershipRepository, NamespaceRepository};
use calimero_network_primitives::{blob_types::BlobAnnouncement, stream::Stream};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use futures_util::StreamExt;
use libp2p::PeerId;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// How many announced blobs this node fetches at once.
///
/// Prefetch is background work that competes with sync for the same node; two
/// concurrent transfers keep a backlog moving without letting it monopolise the
/// runtime or the peer's serve slots.
const PREFETCH_CONCURRENCY: usize = 2;

/// Hard ceiling on a prefetched blob, mirroring the transfer layer's
/// `MAX_BLOB_SIZE_BYTES` (500 MiB). Checked here from the ADVERTISED size so an
/// oversized blob costs no stream at all; the transfer path enforces the same
/// limit again against the bytes actually received, which is the check that
/// binds when a peer lies.
const MAX_PREFETCH_SIZE_BYTES: u64 = 500 * 1024 * 1024;

/// A bare announcement earns at most this long of transfer work before the
/// slot is released. The discovery path is itself capped at 30s and one
/// in-flight fetch, so this is a backstop against a fetch wedged below that.
const PREFETCH_TIMEOUT: Duration = Duration::from_secs(600);

/// Global prefetch budget, shared by every inbound announcement.
static PREFETCH_SLOTS: Semaphore = Semaphore::const_new(PREFETCH_CONCURRENCY);

/// Read the single announcement frame and prefetch if this node is an
/// availability node for that context.
pub async fn handle_blob_announce_stream(
    node_client: NodeClient,
    context_client: ContextClient,
    peer_id: PeerId,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    let Some(first_message) = stream.next().await else {
        debug!(%peer_id, "blob announce stream closed without a message");
        return Ok(());
    };
    let announcement: BlobAnnouncement = serde_json::from_slice(&first_message?.data)
        .map_err(|err| eyre::eyre!("failed to parse blob announcement: {err}"))?;

    // Nothing is read from this stream after the announcement, and nothing is
    // written back: the sender gets no signal about what we decided, which is
    // what keeps announcing best-effort on their side.
    drop(stream);

    prefetch_announced_blob(&node_client, &context_client, peer_id, announcement).await
}

/// Decide on, and carry out, the prefetch for one announcement.
async fn prefetch_announced_blob(
    node_client: &NodeClient,
    context_client: &ContextClient,
    peer_id: PeerId,
    announcement: BlobAnnouncement,
) -> eyre::Result<()> {
    let BlobAnnouncement {
        blob_id,
        context_id,
        size,
    } = announcement;

    // The authorisation question, asked explicitly and first: fetching on a
    // stranger's say-so would let any peer make this node pull arbitrary bytes
    // for a context it has no business in. Only an availability node of THIS
    // context prefetches, and that fact is read from local governance state,
    // never from the announcement.
    if !is_availability_node_for(node_client, context_client, &context_id)? {
        debug!(
            %peer_id, %blob_id, %context_id,
            "ignoring blob announcement: this node is not a ReadOnlyTee member of that context"
        );
        return Ok(());
    }

    if size > MAX_PREFETCH_SIZE_BYTES {
        warn!(
            %peer_id, %blob_id, %context_id, size,
            max = MAX_PREFETCH_SIZE_BYTES,
            "declining blob announcement: advertised size exceeds the transfer cap"
        );
        return Ok(());
    }

    if node_client.has_blob(&blob_id)? {
        debug!(%blob_id, %context_id, "blob already held locally, nothing to prefetch");
        return Ok(());
    }

    // `try_acquire`, not `acquire`: waiting would let a burst of announcements
    // queue unbounded work on a remote peer's say-so. A dropped prefetch is
    // recoverable — the blob is still findable by probing its holder — so
    // shedding beats queueing.
    let Ok(_permit) = PREFETCH_SLOTS.try_acquire() else {
        debug!(
            %blob_id, %context_id,
            concurrency = PREFETCH_CONCURRENCY,
            "prefetch slots busy, skipping this announcement"
        );
        return Ok(());
    };

    info!(%peer_id, %blob_id, %context_id, size, "prefetching announced blob");

    // Fetch through the ordinary discovery path rather than straight from the
    // announcer: it signs the request with this node's own context identity,
    // verifies the content hash, stores the result, and falls back to another
    // holder if the announcer has since gone away.
    match tokio::time::timeout(
        PREFETCH_TIMEOUT,
        node_client.get_blob_bytes(&blob_id, Some(&context_id)),
    )
    .await
    {
        Ok(Ok(Some(bytes))) => {
            info!(%blob_id, %context_id, size = bytes.len(), "prefetched announced blob");
        }
        Ok(Ok(None)) => {
            warn!(%blob_id, %context_id, "announced blob could not be fetched from any holder");
        }
        Ok(Err(err)) => {
            warn!(%blob_id, %context_id, %err, "failed to prefetch announced blob");
        }
        Err(_elapsed) => {
            warn!(
                %blob_id, %context_id,
                timeout_secs = PREFETCH_TIMEOUT.as_secs(),
                "prefetch of announced blob timed out"
            );
        }
    }

    Ok(())
}

/// Whether this node holds a `ReadOnlyTee` membership covering `context_id`.
///
/// Resolves the local device key for the context and hands the decision to
/// [`is_availability_member`]. Split that way for the same reason
/// `blob_protocol::is_signed_context_member` is: the governance half is
/// `Store`-only and so is testable end to end against real rows, without a
/// client or a node.
fn is_availability_node_for(
    node_client: &NodeClient,
    context_client: &ContextClient,
    context_id: &ContextId,
) -> eyre::Result<bool> {
    // Without a local identity for this context, this node is not a member of
    // it at all, let alone an availability node.
    let Some((public_key, _private_key)) = node_client.find_owned_identity(context_id)? else {
        return Ok(false);
    };
    is_availability_member(context_client.datastore(), context_id, &public_key)
}

/// Whether `public_key` is a `ReadOnlyTee` member covering `context_id`.
///
/// Answered from the context's own group first, then from each ancestor up to
/// the namespace root: a TEE admitted at the root has no direct row in an
/// `Open` subgroup it follows by inheritance, and it is an availability node
/// for that subgroup's contexts all the same.
fn is_availability_member(
    store: &calimero_store::Store,
    context_id: &ContextId,
    public_key: &calimero_primitives::identity::PublicKey,
) -> eyre::Result<bool> {
    let Some(group_id) = get_group_for_context(store, context_id)? else {
        return Ok(false);
    };
    let Some(account) =
        calimero_governance_store::member_account_in_namespace(store, &group_id, public_key)?
    else {
        return Ok(false);
    };

    let members = MembershipRepository::new(store);
    let namespaces = NamespaceRepository::new(store);
    let mut current = group_id;
    // `<=` because reaching the root at depth D takes D+1 parent hops to
    // observe the root's `None` parent (mirrors `NamespaceRepository::resolve`).
    for _ in 0..=calimero_context_config::MAX_NAMESPACE_DEPTH {
        if members.role_of(&current, &account)? == Some(GroupMemberRole::ReadOnlyTee) {
            return Ok(true);
        }
        match namespaces.parent(&current) {
            Ok(Some(parent)) => current = parent,
            // Root reached (`None`), or a store error: either way there is no
            // further ancestor to consult, so this node is not an availability
            // node for the context.
            _ => break,
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::register_context_in_group;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::*;

    fn context() -> ContextId {
        ContextId::from([0xC0; 32])
    }

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// A namespace with the test context registered directly under it, and `key`
    /// enrolled with `role`.
    fn namespace_with(role: GroupMemberRole, key: &PublicKey) -> Store {
        let store = test_store();
        let group = ContextGroupId::from([0xA0; 32]);
        let account = calimero_context::test_support::enrol(&store, &group, key);
        MembershipRepository::new(&store)
            .add_member(&group, &account, role)
            .expect("add member");
        register_context_in_group(&store, &group, &context()).expect("register context");
        store
    }

    #[test]
    fn a_read_only_tee_member_prefetches() {
        let key = PublicKey::from([0x11; 32]);
        let store = namespace_with(GroupMemberRole::ReadOnlyTee, &key);
        assert!(is_availability_member(&store, &context(), &key).expect("decide"));
    }

    /// The gate that stops a stranger's announcement from making this node
    /// fetch: an ordinary member of the context is NOT an availability node,
    /// and must not prefetch on somebody else's say-so.
    #[test]
    fn an_ordinary_member_does_not_prefetch() {
        let key = PublicKey::from([0x11; 32]);
        let store = namespace_with(GroupMemberRole::Member, &key);
        assert!(!is_availability_member(&store, &context(), &key).expect("decide"));
    }

    #[test]
    fn a_non_member_does_not_prefetch() {
        let member = PublicKey::from([0x11; 32]);
        let stranger = PublicKey::from([0x99; 32]);
        let store = namespace_with(GroupMemberRole::ReadOnlyTee, &member);
        assert!(!is_availability_member(&store, &context(), &stranger).expect("decide"));
    }

    #[test]
    fn a_context_with_no_group_binding_does_not_prefetch() {
        let store = test_store();
        let key = PublicKey::from([0x11; 32]);
        assert!(!is_availability_member(&store, &context(), &key).expect("decide"));
    }

    /// A TEE admitted at the namespace ROOT holds no direct row in a subgroup
    /// it follows by inheritance, and is an availability node for that
    /// subgroup's contexts all the same — hence the parent walk.
    #[test]
    fn a_root_admitted_tee_prefetches_for_a_subgroup_context() {
        let store = test_store();
        let root = ContextGroupId::from([0xA0; 32]);
        let subgroup = ContextGroupId::from([0xB0; 32]);
        let key = PublicKey::from([0x11; 32]);

        NamespaceRepository::new(&store)
            .nest(&root, &subgroup)
            .expect("nest subgroup");
        let account = calimero_context::test_support::enrol(&store, &root, &key);
        MembershipRepository::new(&store)
            .add_member(&root, &account, GroupMemberRole::ReadOnlyTee)
            .expect("add tee at root");
        register_context_in_group(&store, &subgroup, &context()).expect("register context");

        assert!(
            is_availability_member(&store, &context(), &key).expect("decide"),
            "a root-admitted ReadOnlyTee is an availability node for a subgroup context"
        );
    }
}
