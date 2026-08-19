//! The envelope, and the hash that is its identity.
//!
//! Everything here is about one question: what exactly is signed. [`Op`]'s `id`
//! is private and [`Op::compute_id`] is the only thing that produces it, so the
//! preimage — which fields are covered, in what order, with what framing — is
//! decided in this file alone. A field added to [`Op`] without being added to
//! the preimage is an unauthenticated field, and keeping the struct and the
//! hash within one screen of each other is what makes that omission visible.

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use calimero_account::{AccountId, DeviceId};
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;

use crate::authorship::Authorship;
use crate::payload::OpPayload;
use crate::scope::ScopeId;

/// One envelope for every kind of change in a scope.
///
/// `parents` are the op's causal predecessors **within its scope**, and MAY
/// also include a cross-scope governance head the op was authored under
/// (visibility-respecting: a subgroup op may reference its ancestor governance
/// scope's head, since subgroup members are ancestor members). It is one parent
/// set, one causal model, spanning data and governance.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Op {
    /// `compute_id(scope, parents, authorship, hlc, payload)` — content
    /// address. **Private** and computed by [`Op::new`] so a caller can't
    /// desync the id from the content it addresses; read it via [`Op::id`] and
    /// re-check it with [`Op::verify`].
    id: [u8; 32],
    /// The scope this op belongs to.
    pub scope: ScopeId,
    /// Causal predecessors (may cross scopes — see the struct docs).
    pub parents: Vec<[u8; 32]>,
    /// Who authored this: account, device, and signing key. See [`Authorship`]
    /// for why all three are carried rather than one key doing every job.
    pub authorship: Authorship,
    /// Hybrid logical clock at author time (causally monotonic).
    pub hlc: HybridTimestamp,
    /// The change itself. Once payload encryption lands, the data arms are
    /// ciphertext at rest under the scope key.
    pub payload: OpPayload,
    /// The author's expected `scope_root` after applying this op — a
    /// convergence **assertion**, not a trusted input. Deliberately NOT part of
    /// the [`compute_id`](Op::compute_id) preimage (so it is unsigned): peers
    /// **recompute** their own `scope_root` from their projection and compare,
    /// rather than trusting the author's number. A tampered value cannot grant
    /// authority — at worst it flags a divergence the recompute would catch
    /// anyway. Security never depends on this field.
    pub expected_scope_root: [u8; 32],
    /// Ed25519 signature by [`Authorship::device_key`] over the
    /// [`compute_id`](Op::compute_id) preimage (i.e. over `id`). The signature
    /// is intentionally NOT folded back into `id` (it signs the id, which would
    /// be circular).
    ///
    /// **Callers MUST verify this signature before trusting an `Op`.**
    /// `calimero-projection`/`calimero-authz` assume already-verified ops: they
    /// fold/authorize on content alone and perform no signature check. Feeding
    /// an unverified op into the projection bypasses authentication entirely.
    ///
    /// Note the two-stage split. Verifying this signature proves only that the
    /// holder of `device_key` authored the op — it says nothing about whether
    /// that key currently speaks for [`Authorship::account`]. That second
    /// question is answered at the causal cut by `calimero-authz`, because only
    /// the cut knows which links and revocations are in force.
    pub signature: [u8; 64],
}

impl Op {
    /// Build an op, computing its content-address [`id`](Op::id) from the
    /// content so the two can never disagree. `signature` is the author's
    /// Ed25519 signature over that id (see the [`signature`](Op::signature)
    /// field docs); callers sign `Op::compute_id(...)` with the author key.
    #[must_use]
    pub fn new(
        scope: ScopeId,
        parents: Vec<[u8; 32]>,
        authorship: Authorship,
        hlc: HybridTimestamp,
        payload: OpPayload,
        expected_scope_root: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        let id = Self::compute_id(scope, &parents, &authorship, &hlc, &payload);
        Self {
            id,
            scope,
            parents,
            authorship,
            hlc,
            payload,
            expected_scope_root,
            signature,
        }
    }

    /// Build an op from an **explicit** `id` rather than recomputing it from the
    /// content.
    ///
    /// This exists only for the unified-op *bridge*: a [`SignedNamespaceOp`] /
    /// rotation entry is already a node in the governance DAG with its own
    /// identity (`content_hash` / `delta_id`), and the unified `Op` mirrors that
    /// node verbatim — keyed in the op-store by that same id — rather than by
    /// `Op::compute_id` of the projected payload. These bridge ops are internal,
    /// unsigned projections of already-verified governance ops, so they are not
    /// passed through [`Op::verify`]. Fresh, independently-signed ops must use
    /// [`Op::new`] instead, so their id is a true content address.
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per Op field (incl. the explicit id); a builder \
                  would obscure the deliberate 1:1 field mapping for the bridge"
    )]
    #[must_use]
    pub fn from_parts(
        id: [u8; 32],
        scope: ScopeId,
        parents: Vec<[u8; 32]>,
        authorship: Authorship,
        hlc: HybridTimestamp,
        payload: OpPayload,
        expected_scope_root: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        Self {
            id,
            scope,
            parents,
            authorship,
            hlc,
            payload,
            expected_scope_root,
            signature,
        }
    }

    /// The account whose authority this op is judged against.
    #[must_use]
    pub const fn author(&self) -> AccountId {
        self.authorship.account
    }

    /// The CRDT replica that authored this op.
    #[must_use]
    pub const fn device(&self) -> DeviceId {
        self.authorship.device
    }

    /// The key that signed this op.
    #[must_use]
    pub const fn device_key(&self) -> &PublicKey {
        &self.authorship.device_key
    }

    /// Content address of this op.
    #[must_use]
    pub const fn id(&self) -> [u8; 32] {
        self.id
    }

    /// Verify this op end-to-end: the cached [`id`](Op::id) actually addresses
    /// the content, **and** the signature is a valid Ed25519 signature over
    /// that id by [`author`](Op::author).
    ///
    /// `calimero-projection`/`calimero-authz` assume already-verified ops, so
    /// every op crossing a trust boundary (deserialized, received from a peer)
    /// MUST pass this before being folded.
    #[must_use]
    pub fn verify(&self) -> bool {
        let recomputed = Self::compute_id(
            self.scope,
            &self.parents,
            &self.authorship,
            &self.hlc,
            &self.payload,
        );
        if recomputed != self.id {
            return false;
        }
        // Against the DEVICE key, not the account: an account has no key of its
        // own to sign with, and the whole point of the split is that the thing
        // which signs is per-device and revocable. Binding that key to
        // `authorship.account` is a separate, at-cut decision made by
        // `calimero-authz` — see the `signature` field docs.
        self.authorship
            .device_key
            .verify_raw_signature(&self.id, &self.signature)
            .is_ok()
    }

    /// Content address of an op: `Sha256(scope ‖ sorted(parents) ‖
    /// borsh(authorship) ‖ hlc ‖ borsh(payload))`. Parents are sorted so the id
    /// is independent of the order a builder happened to list them in.
    ///
    /// The whole [`Authorship`] triple is in the preimage, so the account, the
    /// replica id, and the signing key are all covered by the signature. Each
    /// omission would be exploitable on its own — see the [`Authorship`] docs.
    ///
    /// # Panics
    /// Never in practice — borsh-serializing these field types into an
    /// in-memory buffer is infallible; the `expect` documents that invariant.
    #[must_use]
    pub fn compute_id(
        scope: ScopeId,
        parents: &[[u8; 32]],
        authorship: &Authorship,
        hlc: &HybridTimestamp,
        payload: &OpPayload,
    ) -> [u8; 32] {
        let mut sorted = parents.to_vec();
        sorted.sort_unstable();

        let mut hasher = Sha256::new();
        hasher.update(scope.as_bytes());
        // Length-prefix the parent list so the boundary between the (variable
        // count of) parents and the authorship that follows is unambiguous —
        // i.e. `parents=[A,B], account=C` can never hash-collide with
        // `parents=[A,B,C], account=…`. All other fields are fixed-size or
        // borsh-length-prefixed.
        hasher.update((sorted.len() as u64).to_le_bytes());
        for parent in &sorted {
            hasher.update(parent);
        }
        hasher.update(borsh::to_vec(authorship).expect("Authorship borsh is infallible in-memory"));
        hasher.update(borsh::to_vec(hlc).expect("HybridTimestamp borsh is infallible in-memory"));
        hasher.update(borsh::to_vec(payload).expect("OpPayload borsh is infallible in-memory"));
        hasher.finalize().into()
    }
}
