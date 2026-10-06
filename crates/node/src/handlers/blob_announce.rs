//! Availability prefetch: react to "I now hold this blob for this context".
//!
//! An availability node that replicates a context's state but not the bytes
//! that state references is not delivering availability — it can answer a probe
//! honestly and still always answer "no". This is the path that makes the
//! answer usually "yes".
//!
//! ## Why the announce site and not state enumeration
//!
//! The obvious design, "on sync, fetch every blob the context's state
//! references", is not implementable: state deltas carry opaque borsh values
//! with no type tag, so a `BlobRef` sitting inside application state is
//! unrecognisable at apply time. The blob→context association is recorded only
//! on the producer's node (`BlobOwner`, node-local), and reaches another node
//! only when the producer announces it. So the announce is what drives prefetch.
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
use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use calimero_context_client::client::ContextClient;
use calimero_governance_store::get_group_for_context;
use calimero_network_primitives::blob_types::{BlobAnnouncement, BlobRequest};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{MemberIdentity, PublicKey};
use futures_util::StreamExt;
use libp2p::PeerId;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::handlers::blob_protocol::is_signed_context_member;

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

const MEMBER_PREFETCH_BUDGET_BYTES: u64 = 4 * 1024 * 1024 * 1024; // per member per window
const MEMBER_PREFETCH_WINDOW: Duration = Duration::from_secs(24 * 60 * 60); // how long a charge counts
const MIN_PREFETCH_CHARGE_BYTES: u64 = 1024 * 1024; // so a failed or empty fetch is not free

/// Global prefetch budget, shared by every inbound announcement.
static PREFETCH_SLOTS: Semaphore = Semaphore::const_new(PREFETCH_CONCURRENCY);

/// Members with a prefetch running for one of their announcements.
static PREFETCHING_FOR: Mutex<BTreeSet<MemberIdentity>> = Mutex::new(BTreeSet::new());

/// One announcing member's single prefetch, released when the fetch ends, so
/// one member cannot take every global slot.
struct MemberPrefetch(MemberIdentity);

impl MemberPrefetch {
    fn claim(member: MemberIdentity) -> Option<Self> {
        let mut busy = PREFETCHING_FOR
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        busy.insert(member).then(|| Self(member))
    }
}

impl Drop for MemberPrefetch {
    fn drop(&mut self) {
        let _was_busy = PREFETCHING_FOR
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// How long an inbound announce stream may stay silent before it is dropped.
///
/// This handler runs on a detached task holding client handles and an 8 MiB
/// framed buffer, and nothing caps how many peers may open a stream, so a peer
/// that connects and never speaks must not be able to park one forever. The
/// budget only has to cover ONE small frame already on its way: the sender's
/// whole announce — open, send, close — is itself capped at 5s by
/// `send_blob_announcement`, so 10s is comfortably above any legitimate
/// exchange while still bounding a silent or stalled one.
const ANNOUNCE_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Read the single announcement frame and prefetch if this node is an
/// availability node for that context.
pub async fn handle_blob_announce_stream(
    node_client: NodeClient,
    context_client: ContextClient,
    peer_id: PeerId,
    stream: Box<Stream>,
) -> eyre::Result<()> {
    let Some(announcement) = read_announcement(peer_id, stream).await? else {
        return Ok(());
    };

    prefetch_announced_blob(&node_client, &context_client, peer_id, announcement).await
}

/// Read the one frame an announce stream carries, under
/// [`ANNOUNCE_READ_TIMEOUT`].
///
/// `Ok(None)` for "there is nothing to act on": the peer closed without
/// speaking, or went silent and ran out of time. Both are ordinary — an
/// announce is best-effort on the sender's side too — so neither is an error.
/// A frame that arrives but does not parse IS an error, because the peer spoke
/// the protocol badly.
///
/// Takes the stream by value and drops it on every path: nothing is read after
/// the announcement and nothing is written back, so the sender gets no signal
/// about what was decided, which is what keeps announcing cheap for them.
async fn read_announcement(
    peer_id: PeerId,
    mut stream: Box<Stream>,
) -> eyre::Result<Option<BlobAnnouncement>> {
    let Ok(first_message) = tokio::time::timeout(ANNOUNCE_READ_TIMEOUT, stream.next()).await else {
        debug!(
            %peer_id,
            timeout_secs = ANNOUNCE_READ_TIMEOUT.as_secs(),
            "blob announce stream went silent; dropping it"
        );
        return Ok(None);
    };

    let Some(first_message) = first_message else {
        debug!(%peer_id, "blob announce stream closed without a message");
        return Ok(None);
    };

    let announcement: BlobAnnouncement = serde_json::from_slice(&first_message?.data)
        .map_err(|err| eyre::eyre!("failed to parse blob announcement: {err}"))?;

    Ok(Some(announcement))
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
        auth,
    } = announcement;

    // The authorisation question, asked explicitly and first: fetching on a
    // stranger's say-so would let any peer make this node pull arbitrary bytes
    // for a context it has no business in. Only an availability node of THIS
    // context prefetches, and that fact is read from local governance state,
    // never from the announcement.
    if !is_availability_node_for(node_client, context_client, &context_id)? {
        debug!(
            %peer_id, %blob_id, %context_id,
            "ignoring blob announcement: this node is not a TEE member of that context"
        );
        return Ok(());
    }

    if !is_from_context_member(context_client.datastore(), peer_id, &announcement)? {
        debug!(
            %peer_id, %blob_id, %context_id, signer = %auth.public_key,
            "ignoring blob announcement: not signed for this peer by a member of the context"
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

    // Bytes held here for another context do not count: only a fetch from
    // this context's peers makes them this context's to serve.
    if node_client.is_blob_held_for_context(&context_id, &blob_id)? {
        debug!(%blob_id, %context_id, "blob already held for the context, nothing to prefetch");
        return Ok(());
    }

    let announcer = announcing_member(context_client.datastore(), &context_id, auth.public_key);
    let Some(_member_prefetch) = MemberPrefetch::claim(announcer) else {
        debug!(
            %peer_id, %blob_id, %context_id, signer = %auth.public_key,
            "a prefetch for this member is already running, skipping this announcement"
        );
        return Ok(());
    };

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
        node_client.fetch_blob_for_context(&blob_id, &context_id),
    )
    .await
    {
        Ok(Ok(Some(_))) => {
            info!(%blob_id, %context_id, size, "prefetched announced blob");
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

/// Whether `announcement` carries a valid membership proof for its context,
/// signed for `peer_id`: the check a signed blob read gets.
fn is_from_context_member(
    store: &calimero_store::Store,
    peer_id: PeerId,
    announcement: &BlobAnnouncement,
) -> eyre::Result<bool> {
    let request = BlobRequest {
        blob_id: announcement.blob_id,
        context_id: announcement.context_id,
        auth: Some(announcement.auth),
    };
    is_signed_context_member(store, &request, &peer_id)
}

/// The account behind `key` in the context's namespace, or the key itself when
/// it has none, so all of one account's devices share a single prefetch.
fn announcing_member(
    store: &calimero_store::Store,
    context_id: &ContextId,
    key: PublicKey,
) -> MemberIdentity {
    get_group_for_context(store, context_id)
        .ok()
        .flatten()
        .and_then(|group| {
            calimero_governance_store::member_account_in_namespace(store, &group, &key)
                .ok()
                .flatten()
        })
        .map_or_else(|| key.into(), MemberIdentity::from)
}

/// Whether this node holds a TEE membership (`ReadOnlyTee` or `RelayTee`)
/// covering `context_id`.
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

/// Whether `public_key` is a TEE member covering `context_id`.
///
/// Answered by [`crate::sync::is_availability_account`], which checks the
/// context's own group and every ancestor up to the namespace root — a TEE
/// admitted at the root has no direct row in an `Open` subgroup it follows by
/// inheritance, and is an availability node for that subgroup's contexts all
/// the same.
///
/// The SEND side (`crate::availability_peers`) resolves who to announce to from
/// [`crate::sync::availability_accounts_for_group`], and both read the one
/// ancestor walk in `crate::sync::availability_levels`. Sharing it is
/// deliberate: two walks for "is this an availability member of this context,
/// directly or by inheritance" would drift, and a send side that walked fewer
/// levels than the receive side would silently announce to nobody in exactly
/// the topology the fleet runs.
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

    Ok(crate::sync::is_availability_account(
        store, &group_id, &account,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        register_context_in_group, MembershipRepository, NamespaceRepository,
    };
    use calimero_network_primitives::blob_types::{BlobAuth, BlobAuthPayload};
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::stream::Message;
    use calimero_node_primitives::test_fixtures::{
        network_of_one_peer, network_of_peers, node_client_over,
    };
    use calimero_primitives::blobs::BlobId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use futures_util::SinkExt;
    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;

    const BLOB: [u8; 32] = [0xD0; 32];
    const TEE: [u8; 32] = [0x11; 32];
    const MEMBER: [u8; 32] = [0x21; 32];
    const OTHER_MEMBER: [u8; 32] = [0x22; 32];
    const HEAVY_UPLOADER: [u8; 32] = [0x23; 32];
    const DAILY_UPLOADER: [u8; 32] = [0x24; 32];
    const UNDERSTATING_UPLOADER: [u8; 32] = [0x25; 32];
    const EMPTY_ANNOUNCER: [u8; 32] = [0x26; 32];
    const SHED_UPLOADER: [u8; 32] = [0x27; 32];
    const MANY_HOLDERS_UPLOADER: [u8; 32] = [0x28; 32];
    const SMALL_UPLOADER: [u8; 32] = [0x29; 32];
    const UNREACHABLE_UPLOADER: [u8; 32] = [0x2A; 32];
    const STRANGER: [u8; 32] = [0x99; 32];
    /// Shorter than the node's own 1 s local-blob lookup, where a fetch first waits.
    const FETCH_WINDOW: Duration = Duration::from_millis(100);

    fn context() -> ContextId {
        ContextId::from([0xC0; 32])
    }

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// A namespace with the test context registered directly under it, and `key`
    /// enrolled with `role`.
    fn namespace_with(role: GroupMemberRole, key: &PublicKey) -> Store {
        namespace_with_members(&[(role, *key)])
    }

    fn namespace_with_members(members: &[(GroupMemberRole, PublicKey)]) -> Store {
        let store = test_store();
        let group = ContextGroupId::from([0xA0; 32]);
        for (role, key) in members {
            let account = calimero_context::test_support::enrol(&store, &group, key);
            MembershipRepository::new(&store)
                .add_member(&group, &account, role.clone())
                .expect("add member");
        }
        register_context_in_group(&store, &group, &context()).expect("register context");
        store
    }

    /// A peer that opens the stream and never speaks must not park this
    /// handler's task forever — it holds client handles and an 8 MiB framed
    /// buffer, and nothing caps how many peers may open a stream.
    ///
    /// `start_paused` advances the clock the moment nothing is runnable, so the
    /// 10s budget is enforced without the test taking 10s. The sending half is
    /// held (never dropped, never written) for the whole call, which is exactly
    /// the silent-peer case — dropping it would close the stream and hit the
    /// different, already-handled "closed without a message" path.
    #[tokio::test(start_paused = true)]
    async fn a_silent_peer_does_not_park_the_handler() {
        let (ours, _theirs) = Stream::test_pair();

        let read = read_announcement(PeerId::random(), Box::new(ours))
            .await
            .expect("a silent peer is not an error");

        assert!(
            read.is_none(),
            "nothing was announced, so nothing to act on"
        );
    }

    /// The other end closing without speaking is likewise ordinary, and is
    /// distinguished from the timeout above only by which branch reports it.
    #[tokio::test(start_paused = true)]
    async fn a_peer_that_closes_without_speaking_is_not_an_error() {
        let (ours, theirs) = Stream::test_pair();
        drop(theirs);

        let read = read_announcement(PeerId::random(), Box::new(ours))
            .await
            .expect("a closed stream is not an error");

        assert!(read.is_none());
    }

    /// A frame that arrives but is not an announcement IS an error: the peer
    /// negotiated this protocol and then spoke something else.
    #[tokio::test(start_paused = true)]
    async fn a_malformed_frame_is_an_error() {
        let (ours, mut theirs) = Stream::test_pair();
        theirs
            .send(calimero_network_primitives::stream::Message::new(
                b"not an announcement".to_vec(),
            ))
            .await
            .expect("send");

        let read = read_announcement(PeerId::random(), Box::new(ours)).await;
        assert!(read.is_err(), "a malformed frame must surface as an error");
    }

    /// An announcement of `BLOB` in the test context, signed by `signer` for `peer`.
    fn announcement_signed_by(
        signer: &PrivateKey,
        peer: PeerId,
        timestamp: u64,
    ) -> BlobAnnouncement {
        announcement_of(BlobId::from(BLOB), signer, peer, timestamp)
    }

    fn announcement_of(
        blob_id: BlobId,
        signer: &PrivateKey,
        peer: PeerId,
        timestamp: u64,
    ) -> BlobAnnouncement {
        let payload = BlobAuthPayload {
            blob_id: *blob_id,
            context_id: *context(),
            timestamp,
            requester: peer.to_bytes(),
        };
        let signature = signer
            .sign(&borsh::to_vec(&payload).expect("encode"))
            .expect("sign")
            .to_bytes();
        BlobAnnouncement {
            blob_id,
            context_id: context(),
            size: 1,
            auth: BlobAuth {
                public_key: signer.public_key(),
                signature,
                timestamp,
            },
        }
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs()
    }

    /// An availability node for the test context whose node manager and peers
    /// never answer, so a fetch it starts stays waiting. Tests share its slots.
    async fn availability_node() -> (NodeClient, ContextClient, TempDir, TempDir) {
        availability_node_over(NetworkClient::new(LazyRecipient::new())).await
    }

    async fn availability_node_over(
        network: NetworkClient,
    ) -> (NodeClient, ContextClient, TempDir, TempDir) {
        let tee = PrivateKey::from(TEE);
        let mut members = vec![(GroupMemberRole::ReadOnlyTee, tee.public_key())];
        for key in [
            MEMBER,
            OTHER_MEMBER,
            HEAVY_UPLOADER,
            DAILY_UPLOADER,
            UNDERSTATING_UPLOADER,
            EMPTY_ANNOUNCER,
            SHED_UPLOADER,
            MANY_HOLDERS_UPLOADER,
            SMALL_UPLOADER,
            UNREACHABLE_UPLOADER,
        ] {
            members.push((GroupMemberRole::Member, PrivateKey::from(key).public_key()));
        }
        let store = namespace_with_members(&members);
        store
            .handle()
            .put(
                &calimero_store::key::ContextIdentity::new(context(), tee.public_key()),
                &calimero_store::types::ContextIdentity {
                    private_key: Some(*tee.as_bytes()),
                },
            )
            .expect("store identity");
        let (node_client, data_dir, blob_dir) = node_client_over(store.clone(), network).await;
        let context_client = ContextClient::new(store, node_client.clone(), LazyRecipient::new());
        (node_client, context_client, data_dir, blob_dir)
    }

    /// Whether handling `announcement` from `peer` got past every check to the
    /// fetch, which does not return within the window here.
    async fn starts_a_fetch(
        node: &(NodeClient, ContextClient),
        peer: PeerId,
        announcement: BlobAnnouncement,
    ) -> bool {
        let handled = prefetch_announced_blob(&node.0, &node.1, peer, announcement);
        tokio::time::timeout(FETCH_WINDOW, handled).await.is_err()
    }

    /// `member` announces `BLOB` at `size`, signed for `peer`.
    fn announcement_sized(member: &PrivateKey, peer: PeerId, size: u64) -> BlobAnnouncement {
        BlobAnnouncement {
            size,
            ..announcement_signed_by(member, peer, now_secs())
        }
    }

    /// `member` announces `bytes` in blobs no larger than the size cap, and
    /// every one of them starts a fetch.
    async fn spend(
        node: &(NodeClient, ContextClient),
        peer: PeerId,
        member: &PrivateKey,
        bytes: u64,
    ) {
        let mut left = bytes;
        while left > 0 {
            let size = left.min(MAX_PREFETCH_SIZE_BYTES);
            let announcement = announcement_sized(member, peer, size);
            assert!(
                starts_a_fetch(node, peer, announcement).await,
                "{left} bytes left to spend"
            );
            left -= size;
        }
    }

    /// Announcements read off the wire make an availability node fetch only
    /// when signed, for the sending peer, by a member of the context.
    #[tokio::test(start_paused = true)]
    #[serial(blob_prefetch_slots)]
    async fn only_a_members_announcement_starts_a_fetch() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let node = (node_client, context_client);
        let peer = PeerId::random();
        let cases = [
            (
                "stranger",
                announcement_signed_by(&PrivateKey::from(STRANGER), peer, now_secs()),
                false,
            ),
            (
                "relayed",
                announcement_signed_by(&PrivateKey::from(MEMBER), PeerId::random(), now_secs()),
                false,
            ),
            (
                "member",
                announcement_signed_by(&PrivateKey::from(MEMBER), peer, now_secs()),
                true,
            ),
        ];

        for (case, sent, expected) in cases {
            let (ours, mut theirs) = Stream::test_pair();
            theirs
                .send(Message::new(serde_json::to_vec(&sent).expect("encode")))
                .await
                .expect("send");
            let read = read_announcement(peer, Box::new(ours))
                .await
                .expect("read")
                .expect("an announcement");

            assert_eq!(starts_a_fetch(&node, peer, read).await, expected, "{case}");
        }
    }

    /// One member's announcements start one fetch at a time, so a flood from
    /// one member cannot take every slot; another member still gets one.
    #[tokio::test(start_paused = true)]
    #[serial(blob_prefetch_slots)]
    async fn one_member_runs_one_fetch_at_a_time() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let node = (node_client, context_client);
        let peer = PeerId::random();
        let member = PrivateKey::from(MEMBER);

        let running = {
            let (node_client, context_client) = node.clone();
            let first = announcement_signed_by(&member, peer, now_secs());
            tokio::spawn(async move {
                prefetch_announced_blob(&node_client, &context_client, peer, first).await
            })
        };
        tokio::task::yield_now().await;

        let flood = announcement_signed_by(&member, peer, now_secs());
        assert!(
            !starts_a_fetch(&node, peer, flood).await,
            "the flood is capped"
        );
        let other = announcement_signed_by(&PrivateKey::from(OTHER_MEMBER), peer, now_secs());
        assert!(
            starts_a_fetch(&node, peer, other).await,
            "another member still fetches"
        );

        running.abort();
    }

    /// A member that has caused its whole budget of prefetch bytes gets no
    /// more prefetches, not even of one byte, while another member is served.
    #[tokio::test(start_paused = true)]
    #[serial(blob_prefetch_slots)]
    async fn a_member_over_its_byte_budget_gets_no_more_prefetches() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let node = (node_client, context_client);
        let peer = PeerId::random();
        let uploader = PrivateKey::from(HEAVY_UPLOADER);

        spend(&node, peer, &uploader, MEMBER_PREFETCH_BUDGET_BYTES).await;

        let over = announcement_sized(&uploader, peer, 1);
        assert!(
            !starts_a_fetch(&node, peer, over).await,
            "the budget is spent"
        );
        let other = announcement_sized(&PrivateKey::from(OTHER_MEMBER), peer, 1);
        assert!(
            starts_a_fetch(&node, peer, other).await,
            "another member still fetches"
        );
    }

    /// The budget is per window: once it has passed, the member is served again.
    #[tokio::test(start_paused = true)]
    #[serial(blob_prefetch_slots)]
    async fn a_member_is_served_again_once_its_window_has_passed() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let node = (node_client, context_client);
        let peer = PeerId::random();
        let uploader = PrivateKey::from(DAILY_UPLOADER);

        spend(&node, peer, &uploader, MEMBER_PREFETCH_BUDGET_BYTES).await;
        let refused = announcement_sized(&uploader, peer, 1);
        assert!(
            !starts_a_fetch(&node, peer, refused).await,
            "the budget is spent"
        );

        tokio::time::advance(MEMBER_PREFETCH_WINDOW).await;
        let next_day = announcement_sized(&uploader, peer, 1);
        assert!(starts_a_fetch(&node, peer, next_day).await, "a new window");
    }

    /// A peer serving more than the advertised size is cut off, not stored, and
    /// the same bytes announced at their size are. Real time: peers answer from another thread.
    #[tokio::test]
    #[serial(blob_prefetch_slots)]
    async fn an_announcement_fetches_no_more_than_it_advertised() {
        let served = vec![0x5A; 2 * MIN_PREFETCH_CHARGE_BYTES as usize];
        let (node_client, context_client, _data, _blobs) =
            availability_node_over(network_of_one_peer(Some(served.clone()))).await;
        let (blob, _size) = node_client
            .add_blob(served.as_slice(), None, None)
            .await
            .expect("hash the served bytes");
        let _deleted = node_client.delete_blob(blob).await.expect("forget them");
        let peer = PeerId::random();

        let understated = BlobAnnouncement {
            size: 1,
            ..announcement_of(
                blob,
                &PrivateKey::from(UNDERSTATING_UPLOADER),
                peer,
                now_secs(),
            )
        };
        prefetch_announced_blob(&node_client, &context_client, peer, understated)
            .await
            .expect("prefetch");
        assert!(
            !node_client.has_blob(&blob).expect("read"),
            "nothing past the advertised size is kept"
        );

        let honest = BlobAnnouncement {
            size: served.len() as u64,
            ..announcement_of(blob, &PrivateKey::from(OTHER_MEMBER), peer, now_secs())
        };
        prefetch_announced_blob(&node_client, &context_client, peer, honest)
            .await
            .expect("prefetch");
        assert!(
            node_client.has_blob(&blob).expect("read"),
            "the advertised size is fetched"
        );
    }

    /// Every holder tried for one announcement draws on the same charge, so
    /// peers sending bytes that do not match cannot multiply it.
    #[tokio::test]
    #[serial(blob_prefetch_slots)]
    async fn holders_of_one_announcement_share_its_charge() {
        let junk = vec![0x5A; 600 * 1024];
        let (network, sent) = network_of_peers(4, Some(junk.clone()));
        let (node_client, context_client, _data, _blobs) = availability_node_over(network).await;
        let peer = PeerId::random();

        let announced = announcement_sized(
            &PrivateKey::from(MANY_HOLDERS_UPLOADER),
            peer,
            MIN_PREFETCH_CHARGE_BYTES,
        );
        prefetch_announced_blob(&node_client, &context_client, peer, announced)
            .await
            .expect("prefetch");

        // The transfer that overdraws the charge has arrived by the time it is
        // refused, so one holder's bytes may pass it, and no holder after that.
        let sent = sent.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            sent <= MIN_PREFETCH_CHARGE_BYTES + junk.len() as u64,
            "{sent} bytes sent for one charge"
        );
    }

    /// A prefetch that finds no holder is charged the minimum, not the size it
    /// advertised, so an unreachable uploader does not spend the budget.
    #[tokio::test]
    #[serial(blob_prefetch_slots)]
    async fn a_failed_fetch_is_charged_the_minimum() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let waiting = (node_client, context_client);
        let (node_client, context_client, _empty_data, _empty_blobs) =
            availability_node_over(network_of_one_peer(None)).await;
        let peer = PeerId::random();
        let uploader = PrivateKey::from(UNREACHABLE_UPLOADER);

        let unreachable = announcement_sized(&uploader, peer, MAX_PREFETCH_SIZE_BYTES);
        prefetch_announced_blob(&node_client, &context_client, peer, unreachable)
            .await
            .expect("prefetch");

        spend(
            &waiting,
            peer,
            &uploader,
            MEMBER_PREFETCH_BUDGET_BYTES - MIN_PREFETCH_CHARGE_BYTES,
        )
        .await;
        let spent = announcement_sized(&uploader, peer, 1);
        assert!(
            !starts_a_fetch(&waiting, peer, spent).await,
            "the budget is spent"
        );
    }

    /// A fetch that succeeds is charged the bytes it took, not the minimum, so
    /// small blobs do not run the budget down by announcement count.
    #[tokio::test]
    #[serial(blob_prefetch_slots)]
    async fn a_fetched_blob_is_charged_what_it_took() {
        let small = vec![0x5B; 4096];
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let waiting = (node_client, context_client);
        let (node_client, context_client, _serving_data, _serving_blobs) =
            availability_node_over(network_of_one_peer(Some(small.clone()))).await;
        let (blob, _size) = node_client
            .add_blob(small.as_slice(), None, None)
            .await
            .expect("hash the served bytes");
        let _deleted = node_client.delete_blob(blob).await.expect("forget them");
        let peer = PeerId::random();
        let uploader = PrivateKey::from(SMALL_UPLOADER);

        spend(
            &waiting,
            peer,
            &uploader,
            MEMBER_PREFETCH_BUDGET_BYTES - 2 * MIN_PREFETCH_CHARGE_BYTES + 1,
        )
        .await;
        let fetched = BlobAnnouncement {
            size: 0,
            ..announcement_of(blob, &uploader, peer, now_secs())
        };
        prefetch_announced_blob(&node_client, &context_client, peer, fetched)
            .await
            .expect("prefetch");
        assert!(
            node_client.has_blob(&blob).expect("read"),
            "the small blob is fetched"
        );

        let settled = announcement_sized(&uploader, peer, 0);
        assert!(
            starts_a_fetch(&waiting, peer, settled).await,
            "only 4 KiB was charged"
        );
        let spent = announcement_sized(&uploader, peer, 0);
        assert!(
            !starts_a_fetch(&waiting, peer, spent).await,
            "the budget is spent"
        );
    }

    /// An announcement costs at least the minimum charge, even when it
    /// advertises nothing, so repeating failed fetches is not free.
    #[tokio::test(start_paused = true)]
    #[serial(blob_prefetch_slots)]
    async fn an_announcement_is_charged_at_least_the_minimum() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let node = (node_client, context_client);
        let peer = PeerId::random();
        let uploader = PrivateKey::from(EMPTY_ANNOUNCER);

        spend(
            &node,
            peer,
            &uploader,
            MEMBER_PREFETCH_BUDGET_BYTES - MIN_PREFETCH_CHARGE_BYTES + 1,
        )
        .await;

        let empty = announcement_sized(&uploader, peer, 0);
        assert!(
            !starts_a_fetch(&node, peer, empty).await,
            "less than the minimum is left"
        );
    }

    /// An announcement shed because every slot is busy costs its member nothing.
    #[tokio::test(start_paused = true)]
    #[serial(blob_prefetch_slots)]
    async fn an_announcement_shed_for_busy_slots_is_not_charged() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let node = (node_client, context_client);
        let peer = PeerId::random();
        let running: Vec<_> = [MEMBER, OTHER_MEMBER]
            .into_iter()
            .map(|key| {
                let (node_client, context_client) = node.clone();
                let busy = announcement_signed_by(&PrivateKey::from(key), peer, now_secs());
                tokio::spawn(async move {
                    prefetch_announced_blob(&node_client, &context_client, peer, busy).await
                })
            })
            .collect();
        tokio::task::yield_now().await;

        let uploader = PrivateKey::from(SHED_UPLOADER);
        let shed = announcement_sized(&uploader, peer, MAX_PREFETCH_SIZE_BYTES);
        assert!(
            !starts_a_fetch(&node, peer, shed).await,
            "every slot is busy"
        );
        for task in running {
            task.abort();
            let _aborted = task.await;
        }

        spend(&node, peer, &uploader, MEMBER_PREFETCH_BUDGET_BYTES).await;
    }

    /// Bytes this node holds only for another context are fetched for this
    /// one all the same; bytes already held for it are not.
    #[tokio::test(start_paused = true)]
    #[serial(blob_prefetch_slots)]
    async fn a_blob_held_only_for_another_context_is_prefetched_for_this_one() {
        let (node_client, context_client, _data, _blobs) = availability_node().await;
        let (blob, _size) = node_client
            .add_blob(&b"the same file in two contexts"[..], None, None)
            .await
            .expect("store bytes");
        node_client
            .record_blob_owner(&ContextId::from([0xC1; 32]), &blob)
            .expect("record");
        let node = (node_client, context_client);
        let peer = PeerId::random();

        let first = announcement_of(blob, &PrivateKey::from(MEMBER), peer, now_secs());
        assert!(starts_a_fetch(&node, peer, first).await, "held elsewhere");

        node.0.record_blob_owner(&context(), &blob).expect("record");
        let second = announcement_of(blob, &PrivateKey::from(OTHER_MEMBER), peer, now_secs());
        assert!(!starts_a_fetch(&node, peer, second).await, "held here");
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
    ///
    /// `root ── Open subgroup ── context`, TEE admitted at the root only.
    fn root_admitted_tee_over_a_subgroup_context() -> (Store, PublicKey) {
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

        (store, key)
    }

    #[test]
    fn a_root_admitted_tee_prefetches_for_a_subgroup_context() {
        let (store, key) = root_admitted_tee_over_a_subgroup_context();
        assert!(
            is_availability_member(&store, &context(), &key).expect("decide"),
            "a root-admitted ReadOnlyTee is an availability node for a subgroup context"
        );
    }

    /// The receive side's point check and the send side's set must agree for
    /// every role at every level: TEE and non-TEE, direct and inherited, member
    /// and stranger. `is_availability_account` replaced a `contains` on that set,
    /// so any account they disagree on is a behaviour change.
    #[test]
    fn the_point_check_agrees_with_the_availability_set() {
        let store = test_store();
        let root = ContextGroupId::from([0xA0; 32]);
        let subgroup = ContextGroupId::from([0xB0; 32]);
        NamespaceRepository::new(&store)
            .nest(&root, &subgroup)
            .expect("nest subgroup");

        let members = MembershipRepository::new(&store);
        let placed = [
            (root, GroupMemberRole::ReadOnlyTee, 0x11),
            (root, GroupMemberRole::Member, 0x12),
            (subgroup, GroupMemberRole::RelayTee, 0x13),
            (subgroup, GroupMemberRole::Member, 0x14),
            (subgroup, GroupMemberRole::Admin, 0x15),
        ];
        let mut accounts = vec![calimero_account::AccountId::from([0x99; 32])];
        for (group, role, seed) in placed {
            let account =
                calimero_context::test_support::enrol(&store, &root, &PublicKey::from([seed; 32]));
            members
                .add_member(&group, &account, role)
                .expect("add member");
            accounts.push(account);
        }

        for group in [root, subgroup] {
            let set = crate::sync::availability_accounts_for_group(&store, &group);
            for account in &accounts {
                assert_eq!(
                    crate::sync::is_availability_account(&store, &group, account),
                    set.contains(account),
                    "{account:?} in {group:?}"
                );
            }
        }
    }

    /// A blob announce asks about ONE announcer, so its cost must not grow with
    /// the namespace: it used to list every member of the context's group and
    /// each ancestor. One bound TEE and `n` other members; reading one member
    /// row per level grows ~1x from 250 to 2000 members, listing them ~8x.
    ///
    /// On RocksDB, because the in-memory test store clones a whole column per
    /// iterator and would charge the binding scan for the member rows.
    #[test]
    fn an_announce_check_does_not_grow_with_the_member_count() {
        use std::time::Instant;

        const SMALL: usize = 250;
        const LARGE: usize = 8 * SMALL;
        const MAX_GROWTH: f64 = 4.0;
        const CALLS: usize = 50;

        let measure = |n: usize| {
            let dir = TempDir::new().expect("tempdir");
            let path = dir.path().to_owned().try_into().expect("utf-8 path");
            let store = Store::open::<calimero_store_rocksdb::RocksDB>(
                &calimero_store::config::StoreConfig::new(path),
            )
            .expect("open rocksdb");
            let group = ContextGroupId::from([0xA0; 32]);
            let members = MembershipRepository::new(&store);
            for i in 0..n {
                let mut account = [0xA5; 32];
                account[..8].copy_from_slice(&(i as u64).to_le_bytes());
                members
                    .add_member(
                        &group,
                        &calimero_account::AccountId::from(account),
                        GroupMemberRole::Member,
                    )
                    .expect("add member");
            }
            let key = PublicKey::from(TEE);
            let account = calimero_context::test_support::enrol(&store, &group, &key);
            members
                .add_member(&group, &account, GroupMemberRole::ReadOnlyTee)
                .expect("add the tee");
            register_context_in_group(&store, &group, &context()).expect("register context");

            (0..3)
                .map(|_| {
                    let start = Instant::now();
                    for _ in 0..CALLS {
                        assert!(is_availability_member(&store, &context(), &key).expect("decide"));
                    }
                    start.elapsed()
                })
                .min()
                .expect("three runs")
        };

        let _warm = measure(SMALL);
        let small = measure(SMALL);
        let large = measure(LARGE);
        let growth = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
        assert!(
            growth <= MAX_GROWTH,
            "{SMALL} -> {LARGE} members took {small:?} -> {large:?} for {CALLS} checks, \
             {growth:.1}x; one member row per level is ~1x, listing them ~8x"
        );
    }

    /// THE agreement test: the send side and the receive side must answer the
    /// same question the same way, on the same fixture.
    ///
    /// This is the real fleet-HA shape — a root-admitted TEE following a
    /// subgroup's contexts. When only the receiver walked ancestors, the send
    /// side resolved no anchors here: nothing was announced, nothing was probed
    /// first, and the whole feature was inert in production while every
    /// one-sided unit test stayed green. Both directions are asserted together
    /// so that failure mode cannot come back unnoticed.
    #[test]
    fn send_and_receive_sides_agree_on_a_root_admitted_tee() {
        use calimero_node_primitives::client::MemberRoles;

        let (store, key) = root_admitted_tee_over_a_subgroup_context();

        // (a) receive side: this node would prefetch an announcement.
        assert!(
            is_availability_member(&store, &context(), &key).expect("decide"),
            "receive side must accept a root-admitted ReadOnlyTee"
        );

        // (b) send side: a producer would announce to the peer hosting it, and
        // probe that peer first.
        let peer = libp2p::PeerId::random();
        let node_state = crate::state::NodeState::new();
        let _replaced = node_state
            .peer_identities
            .insert(peer, [key].into_iter().collect());
        let resolver =
            crate::availability_peers::GovernanceAvailabilityPeers::new(store, node_state);

        assert_eq!(
            resolver.anchors_for_context(&context()),
            vec![peer],
            "send side must resolve the same node as an availability peer"
        );
    }
}
