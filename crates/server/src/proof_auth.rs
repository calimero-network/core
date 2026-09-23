//! Admitting a caller that proves who it is on the request itself.
//!
//! # Where this sits
//!
//! [`calimero_account::CallerProof`] answers "which account signed this",
//! cryptographically and with no state. This module answers the question after
//! it: **does this node serve that caller at all.** The two are separate
//! because the first is a property of the bytes and the second is a decision
//! the operator made, and collapsing them would put a deployment's policy
//! inside a signature check.
//!
//! It answers neither of the questions after *that*. Whether the account is a
//! member, whether its device has been revoked, and what subset of anything it
//! may see are at-cut questions against live governance, resolved per request
//! by the handler — never here, and never cached.
//!
//! # The posture
//!
//! ```text
//! flag off (default)   proofs from devices of THIS NODE'S OWN account
//! flag on              proofs from anyone
//! ```
//!
//! Off by default because an operator did not ask for callers they have no
//! relationship with. The own-account exemption is what stops that default
//! being useless: a second device of your own account — a phone paired against
//! your desktop node — works without anyone turning anything on, and the flag
//! then means *serve other people's keyholders* rather than *accept proofs*,
//! which is a much easier thing to reason about before flipping it.

use axum::http::HeaderName;
use calimero_account::{AccountId, CallerProof};
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tracing::{info, warn};

use crate::admin::handlers::identity::get_node_identity::node_identity;

/// Where a caller's chain travels.
pub(crate) static PROOF_HEADER: HeaderName = HeaderName::from_static("x-calimero-proof");

/// The largest body this path will hash.
///
/// A proof commits to the body, so admitting one means reading the body before
/// the handler does. Every route reachable this way has a small body or none;
/// streaming uploads stay on the token path precisely so this cap can exist and
/// be small. Refusing past it is deliberate — the alternative is buffering
/// whatever arrives, which turns "prove who you are" into a way to spend a
/// node's memory.
pub(crate) const MAX_PROVEN_BODY: usize = 1024 * 1024;

/// Tolerance applied to both windows in a caller's chain.
///
/// Neither bound is a security property: a client whose clock runs fast mints
/// proofs not yet valid and one running slow mints proofs already expired, and
/// both read to a user as "the login is broken". The window itself is what
/// bounds replay; this only stops a correct client being refused for owning a
/// bad clock.
pub(crate) const CLOCK_SKEW_SECS: u64 = 30;

/// Why a proof was not admitted.
///
/// Every variant is a caller-side condition, and they are kept apart because
/// they send whoever reads the log somewhere different: a malformed header is a
/// client encoding bug, a failed chain is a forgery or a stale credential, and
/// a refused account is a node that was never asked to serve this caller.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Refusal {
    /// The header was not hex, or not a `CallerProof`.
    Malformed,
    /// The chain did not check out against this request.
    Unverified,
    /// The chain is sound and this node does not serve this caller.
    NotServed,
}

/// What this node will admit, and for whom.
#[derive(Clone, Debug)]
pub(crate) struct ProofPolicy {
    /// This node's own device signing key.
    ///
    /// A session statement names the node it was minted for, and this is what
    /// that name is checked against. Without it a hostile relay could take a
    /// statement a user signed for it and present it here as that user.
    pub(crate) node_key: PublicKey,
    /// The account this node itself speaks for.
    pub(crate) node_account: AccountId,
    /// Whether callers outside [`Self::node_account`] are served.
    pub(crate) delegated_access: bool,
}

impl ProofPolicy {
    /// Read this node's own identity, or report that it has none yet.
    ///
    /// `None` is not a failure. A node mints its signing key the first time it
    /// takes part in a namespace, so a fresh one legitimately has none — and a
    /// node in no namespace has nothing a caller could be a member of, so
    /// serving proofs would be answering a question that cannot have a useful
    /// answer. Read once at startup, which matches the value being
    /// configuration: what clients pin should not change under them mid-session.
    pub(crate) fn resolve(store: &Store, delegated_access: bool) -> Option<Self> {
        match node_identity(store) {
            Ok(Some((node_account, node_key, ..))) => Some(Self {
                node_key,
                node_account,
                delegated_access,
            }),
            Ok(None) => {
                info!("this node has no signing key yet, so it serves no request proofs");
                None
            }
            Err(err) => {
                // Fail closed. An unreadable identity must not become a policy
                // that admits anyone, and it must not become one that silently
                // admits the node's own account either.
                warn!(%err, "could not read this node's identity; serving no request proofs");
                None
            }
        }
    }

    /// Decode and check a chain, then decide whether this node serves it.
    ///
    /// `now` is supplied rather than read, so the one time-dependent rule here
    /// is testable without moving a clock — the same reason the intent handler
    /// separates its own.
    pub(crate) fn admit(
        &self,
        header: &[u8],
        method: &str,
        path: &str,
        body: &[u8],
        now: u64,
    ) -> Result<AccountId, Refusal> {
        let bytes = hex::decode(header).map_err(|_ignored| Refusal::Malformed)?;
        let proof: CallerProof =
            borsh::from_slice(&bytes).map_err(|_ignored| Refusal::Malformed)?;

        let caller = proof
            .verify(&self.node_key, method, path, body, now, CLOCK_SKEW_SECS)
            .map_err(|_ignored| Refusal::Unverified)?;

        // The policy decision, and the only one made here. A sound chain from an
        // account this node was never asked to serve is refused for a reason
        // that has nothing to do with the cryptography, which is why it is its
        // own variant rather than folded into the verification failure.
        if !self.delegated_access && caller.account != self.node_account {
            return Err(Refusal::NotServed);
        }

        Ok(caller.account)
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{
        AccountGenesis, AccountProof, Audience, CallerProof, DeviceCert, KemPublicKey,
        LoginStatement, RequestSig,
    };
    use calimero_primitives::identity::{DeviceId, PrivateKey};

    use super::{ProofPolicy, Refusal};

    const METHOD: &str = "GET";
    const PATH: &str = "/admin-api/namespaces";
    const NOW: u64 = 1_700_000_000;

    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    /// A full chain for the account rooted at `root_seed`.
    fn chain_for(root_seed: u8, node: &PrivateKey) -> (CallerProof, calimero_account::AccountId) {
        let root = key(root_seed);
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);
        let device_sk = key(root_seed.wrapping_add(50));
        let session_sk = key(root_seed.wrapping_add(100));

        let cert = DeviceCert::sign(
            &root,
            account,
            device,
            &device_sk.public_key(),
            &KemPublicKey::from([9_u8; 32]),
            0,
            0,
        )
        .expect("cert");

        let session = LoginStatement::sign(
            &device_sk,
            node.public_key(),
            Audience::Cli,
            [0x77; 32],
            session_sk.public_key(),
            NOW,
            NOW + 3600,
        )
        .expect("statement");

        let proof = CallerProof {
            account_proof: AccountProof {
                genesis,
                chain: vec![],
                statement: cert,
            },
            session: Some(session),
            request: RequestSig::sign(&session_sk, METHOD, PATH, b"", NOW, NOW + 300)
                .expect("request"),
        };
        (proof, account)
    }

    fn encoded(proof: &CallerProof) -> String {
        hex::encode(borsh::to_vec(proof).expect("borsh"))
    }

    fn policy(node: &PrivateKey, own: calimero_account::AccountId, open: bool) -> ProofPolicy {
        ProofPolicy {
            node_key: node.public_key(),
            node_account: own,
            delegated_access: open,
        }
    }

    /// The default posture has to be useful, or an operator turns the flag on
    /// for the wrong reason — to make their own phone work — and opens the node
    /// to everyone as a side effect.
    #[test]
    fn a_device_of_this_nodes_own_account_is_served_with_the_flag_off() {
        let node = key(4);
        let (proof, account) = chain_for(1, &node);

        let admitted = policy(&node, account, false)
            .admit(encoded(&proof).as_bytes(), METHOD, PATH, b"", NOW)
            .expect("own account is served");
        assert_eq!(admitted, account);
    }

    /// And a stranger is not, by the same default.
    #[test]
    fn another_account_is_refused_with_the_flag_off() {
        let node = key(4);
        let (proof, _stranger) = chain_for(1, &node);
        let (_mine, own_account) = chain_for(2, &node);

        assert_eq!(
            policy(&node, own_account, false).admit(
                encoded(&proof).as_bytes(),
                METHOD,
                PATH,
                b"",
                NOW
            ),
            Err(Refusal::NotServed),
            "a sound chain from an unserved account is refused for policy, not for cryptography",
        );
    }

    /// Turning the flag on is what serves other people's keyholders — and the
    /// chain itself is unchanged, so the flag is provably what decided it.
    #[test]
    fn another_account_is_served_with_the_flag_on() {
        let node = key(4);
        let (proof, stranger) = chain_for(1, &node);
        let (_mine, own_account) = chain_for(2, &node);

        let admitted = policy(&node, own_account, true)
            .admit(encoded(&proof).as_bytes(), METHOD, PATH, b"", NOW)
            .expect("the flag serves strangers");
        assert_eq!(admitted, stranger);
    }

    /// A chain minted for another node must not be usable here whatever the
    /// flag says: the policy widens who is served, never what a proof means.
    #[test]
    fn a_chain_for_another_node_is_refused_even_with_the_flag_on() {
        let other_node = key(9);
        let (proof, account) = chain_for(1, &other_node);

        assert_eq!(
            policy(&key(4), account, true).admit(
                encoded(&proof).as_bytes(),
                METHOD,
                PATH,
                b"",
                NOW
            ),
            Err(Refusal::Unverified),
        );
    }

    /// A proof for one request does not admit another.
    #[test]
    fn a_proof_does_not_admit_a_different_request() {
        let node = key(4);
        let (proof, account) = chain_for(1, &node);

        assert_eq!(
            policy(&node, account, true).admit(encoded(&proof).as_bytes(), "POST", PATH, b"", NOW),
            Err(Refusal::Unverified),
        );
    }

    #[test]
    fn a_header_that_is_not_a_proof_is_refused_before_any_key_is_touched() {
        let node = key(4);
        let (_proof, account) = chain_for(1, &node);
        let p = policy(&node, account, true);

        assert_eq!(
            p.admit(b"not hex", METHOD, PATH, b"", NOW),
            Err(Refusal::Malformed),
        );
        assert_eq!(
            p.admit(b"abcdef", METHOD, PATH, b"", NOW),
            Err(Refusal::Malformed),
        );
    }
}
