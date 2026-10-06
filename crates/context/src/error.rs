//! Typed error enums for the context crate.
//!
//! This module provides structured error types that replace string-based errors,
//! making error handling more consistent and allowing programmatic matching on errors.

use calimero_primitives::context::ContextId;
use thiserror::Error;

/// Errors that can occur during context operations.
///
/// This enum provides typed variants for various error conditions that may arise
/// when performing context-related operations, replacing string-based error messages
/// with structured, matchable error types.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContextError {
    /// The context was deleted before the operation could complete.
    #[error("context '{context_id}' was deleted before operation could complete")]
    ContextDeleted {
        /// The ID of the context that was deleted.
        context_id: ContextId,
    },

    /// A state inconsistency was detected during execution.
    ///
    /// This occurs when the context state changes but no actions were generated,
    /// which could indicate a potential state synchronization issue.
    #[error(
        "context state changed but no actions were generated, \
         discarding execution outcome to mitigate potential state inconsistency"
    )]
    StateInconsistency,

    /// An error occurred while accessing storage.
    #[error("storage error: {message}")]
    StorageError {
        /// A description of the storage error.
        message: String,
    },

    /// The application's `init` did not complete, so no context was created.
    ///
    /// Carries the guest's own message, which is the only thing that says what
    /// was actually wrong — almost always that the `initializationParams` do
    /// not match `init`'s signature. `#[app::init]` cannot return a `Result`
    /// (the macro rejects it), so the only way it fails is a guest panic, and
    /// the SDK's panic hook routes every one of those through `panic_utf8`;
    /// the message is therefore never empty.
    ///
    /// Typed for the same reason as [`Self::NotAGroupMember`]: it is a caller
    /// precondition, not a server fault, and callers map it to a `400` rather
    /// than letting it fall through to a generic `500` that says nothing.
    #[error("application initialization failed: {message}")]
    InitFailed {
        /// The guest's panic message, verbatim.
        message: String,
    },

    /// This node is not a member of the group the operation targets.
    ///
    /// A legitimate client-side precondition (the node hasn't joined the
    /// group, or isn't in it), not a server fault — callers map this to a
    /// `403`, never a generic `500`.
    /// The node cannot yet decide whether the op's signer was authorized,
    /// because the causal cut it must judge against cites history this node has
    /// not folded — a missing ancestor, or one encrypted under a key it does not
    /// hold yet.
    ///
    /// **Not a refusal, and the distinction is the whole point.** The apply path
    /// burns nothing on this outcome — the DAG head does not advance and the
    /// nonce is not consumed — so the identical call succeeds once sync or a key
    /// pull delivers what is missing. Typed for the same reason as
    /// [`Self::NotAGroupMember`], but mapping to a RETRYABLE status: a caller that
    /// sees a generic `500` cannot tell "wait, and this will work" from "no, and
    /// it never will", and those want opposite client behaviour.
    #[error(
        "authority for '{group_id}' cannot be resolved at this op's causal cut yet          — this node is missing history or a key it is entitled to; retry once it          has synced"
    )]
    AuthorityNotYetResolvable {
        /// The group whose authority could not be resolved.
        group_id: String,
    },

    /// A join found no admitter to endorse it, so it cannot be completed.
    ///
    /// Two causes, told apart because they read differently to a person: no peer
    /// of the namespace could be reached at all (usually the inviting node is
    /// offline), or one answered that the invitation does not name as an
    /// admitter. Both are retryable once an admitter is reachable, which is why
    /// neither is the generic `500` it used to be.
    #[error(
        "join could not be endorsed: {}",
        if *reached_a_peer {
            "the peer that answered is not an admitter named by this invitation; retry \
             once one of the invitation's admitters is online"
        } else {
            "could not reach any member of this namespace to complete the join; make sure \
             the inviting node is online and retry"
        }
    )]
    JoinNotEndorsed {
        /// Whether any peer answered the join request.
        reached_a_peer: bool,
    },

    #[error("node is not a member of group '{group_id}'")]
    NotAGroupMember {
        /// Hex rendering of the target group id (for the message only).
        group_id: String,
    },

    /// The node is not a member of the namespace the request names.
    ///
    /// The namespace-scoped sibling of [`Self::NotAGroupMember`]; separate so
    /// the message names a namespace rather than a group, which is what the
    /// operator is holding when they hit this.
    #[error("node is not a member of namespace '{namespace_id}'")]
    NotANamespaceMember {
        /// Hex rendering of the target namespace id (for the message only).
        namespace_id: String,
    },

    /// A caller-supplied identity is not a member of the group it named.
    ///
    /// Distinct from [`Self::NotAGroupMember`], which is about THIS NODE's own
    /// standing. `create_context` takes an `identity_secret` straight from the
    /// request body, so the identity it checks is frequently not the node's --
    /// and a message saying "node is not a member" would name the wrong
    /// principal entirely.
    ///
    /// `403` rather than `404`: the caller holds this key and is acting AS this
    /// identity, so the refusal is about standing, not about a thing being
    /// absent. It also keeps company with the capability check immediately
    /// after it, which refuses the same call for the same shape of reason.
    #[error("identity '{identity}' is not a member of group '{group_id}'")]
    IdentityNotAGroupMember {
        /// Hex rendering of the target group id (for the message only).
        group_id: String,
        /// Rendering of the identity that was checked (for the message only).
        identity: String,
    },

    /// A delegated creation pins an application the group does not target.
    ///
    /// A self-signed creation silently runs the group's target instead of the
    /// requested application; a delegated one must not, because that would run
    /// code the member did not sign. Refused so the member can re-sign against
    /// the group's current target.
    #[error(
        "the creation warrant pins application {signed}, but group '{group_id}' targets \
         {target}; re-sign the warrant against the group's application"
    )]
    DelegatedApplicationNotTargeted {
        /// Hex rendering of the group (for the message only).
        group_id: String,
        /// The application the warrant pins.
        signed: String,
        /// The application the group targets.
        target: String,
    },

    /// A creation named no service, but the application is a multi-service
    /// bundle, so there is no wasm to run without one.
    ///
    /// Caught against the application row before `init`, where it used to
    /// surface from the bundle reader as an untyped "declares no top-level
    /// wasm" and reach the caller as a `500`. The caller's request is what is
    /// incomplete, so it is a `400`, and the message carries the names the
    /// caller may pick from — the one thing it needs to fix the request.
    #[error(
        "application {application_id} is a multi-service bundle; name one of its services \
         in `service_name`: {}",
        services.join(", ")
    )]
    ServiceNameRequired {
        /// Rendering of the application (for the message only).
        application_id: String,
        /// Every service the bundle declares, in manifest order.
        services: Vec<String>,
    },

    /// A creation named a service the application does not declare. A `400`
    /// for the same reason as [`Self::ServiceNameRequired`]; `services` is
    /// empty for a single-service application, which takes no name at all.
    #[error(
        "application {application_id} declares no service '{service}'; {}",
        if services.is_empty() {
            "it is single-service, so leave `service_name` unset".to_owned()
        } else {
            format!("its services are: {}", services.join(", "))
        }
    )]
    ServiceNotInBundle {
        /// Rendering of the application (for the message only).
        application_id: String,
        /// The service the caller named.
        service: String,
        /// Every service the bundle declares, in manifest order.
        services: Vec<String>,
    },

    /// A `403`: the identity creating a context for itself is a member of the
    /// group but neither an admin nor granted `CAN_CREATE_CONTEXT`. About
    /// standing, like [`Self::IdentityNotAGroupMember`]; being granted the
    /// capability is the only thing that helps. As an untyped `bail!` it was a
    /// `500`, which told the caller to retry a refusal that will never change.
    #[error(
        "identity '{identity}' may not create a context in group '{group_id}': \
         not an admin and CAN_CREATE_CONTEXT is not set"
    )]
    CreateContextNotPermitted {
        /// Hex rendering of the group (for the message only).
        group_id: String,
        /// Rendering of the identity that was checked (for the message only).
        identity: String,
    },

    /// The named group has no meta row on this node.
    ///
    /// Typed so it can answer `404`. As an untyped `bail!` it fell through to
    /// the generic `500`, and a caller cannot tell "what you asked about is
    /// not here" from "this node fell over" — one means stop, the other means
    /// retry. A control-plane script read exactly that 500 as "already left"
    /// and carried on past a real failure.
    #[error("group '{group_id}' not found")]
    GroupNotFound {
        /// Hex rendering of the absent group id (for the message only).
        group_id: String,
    },

    /// The named namespace has no meta row on this node. See
    /// [`Self::GroupNotFound`] for why this is typed.
    #[error("namespace '{namespace_id}' not found")]
    NamespaceNotFound {
        /// Hex rendering of the absent namespace id (for the message only).
        namespace_id: String,
    },

    /// The named application is not installed on this node. See
    /// [`Self::GroupNotFound`] for why this is typed.
    #[error("application '{application_id}' not found")]
    ApplicationNotFound {
        /// Rendering of the absent application id (for the message only).
        application_id: String,
    },

    /// The named context does not exist on this node. See
    /// [`Self::GroupNotFound`] for why this is typed.
    #[error("context '{context_id}' not found")]
    ContextNotFound {
        /// Rendering of the absent context id (for the message only).
        context_id: String,
    },

    /// The key material offered for pairing carries no valid signature from the
    /// device that minted it.
    ///
    /// Typed for the same reason as [`Self::InitFailed`]: it is the caller's
    /// payload that is wrong, so it maps to a `400` rather than a generic `500`
    /// that reads as "the node broke".
    #[error(
        "refusing to certify device {device}: {cause}. The key material does not \
         come with a valid signature from the device that minted it — re-run \
         `account pair-init` and carry its statement across unaltered"
    )]
    PairingStatementInvalid {
        /// The device being certified (for the message only).
        device: String,
        /// The verification failure, verbatim.
        cause: String,
    },

    /// The device id offered for pairing was not minted for the account, so no
    /// peer would accept a certificate for it. A `400`: the payload is wrong.
    #[error(
        "refusing to certify device {device}: its id was not minted for account \
         {account}, so no peer would accept the certificate. Re-run \
         `account pair-init` with a client that mints the current device id"
    )]
    PairingDeviceNotMinted {
        /// The device being certified (for the message only).
        device: String,
        /// The account the pairing certifies into.
        account: String,
    },

    /// The confirmation code does not describe the key material that arrived.
    ///
    /// Never carries the expected code: an attacker able to drive the endpoint
    /// would otherwise learn the one value it cannot produce.
    #[error(
        "refusing to certify device {device}: the confirmation code does not \
         match the key material in this request. Either it was mistyped, or \
         the payload was altered between `account pair-init` and here — in \
         which case do not retry with the code this side computes, get it \
         from the pairing device again"
    )]
    PairingCodeMismatch {
        /// The device being certified (for the message only).
        device: String,
    },

    /// This node takes part in none of the namespaces a pairing covers, so it
    /// holds no identity to sign the endorsement with.
    ///
    /// A precondition on this node's state rather than on the request, so it maps
    /// to a `409`: the identical call works once the node takes part there.
    #[error(
        "this node takes part in none of the namespaces this pairing covers \
         ({namespaces}); it has no identity to sign with and cannot certify a \
         device there"
    )]
    PairingNoNamespaceIdentity {
        /// Debug rendering of the namespaces the pairing is gated on.
        namespaces: String,
    },

    /// No current scope key anywhere in the namespaces a pairing is gated on.
    ///
    /// A `409` for the same reason as [`Self::PairingNoNamespaceIdentity`]: the
    /// request is fine and the node is not ready to serve it yet.
    #[error(
        "this node holds no current scope key in any of {namespaces}; pairing \
         both publishes an encrypted group op and delivers that key, so neither \
         is possible yet"
    )]
    PairingNoScopeKey {
        /// Debug rendering of the namespaces the pairing is gated on.
        namespaces: String,
    },

    /// This node's own device row belongs to another account, so its root cannot
    /// certify a second device for the account named here.
    ///
    /// The right request sent to the wrong node, which is a `403`: no retry and
    /// no state change here will make it work.
    #[error(
        "this node's device belongs to account {enrolled}, not to {account} which its \
         own root owns; a paired device cannot certify further devices — run \
         this on the node that holds the account"
    )]
    PairingNotTheAccountHolder {
        /// The account this node's device row actually names.
        enrolled: String,
        /// The account its own root owns, which the pairing would certify into.
        account: String,
    },

    /// This node holds no certificate for the device a relink names.
    ///
    /// A `404`: only a device this account's registry names can be relinked.
    #[error(
        "this node holds no certificate for device {device}, so it cannot extend it \
         anywhere. Only a device this account's registry names can be relinked"
    )]
    PairingUnknownDevice {
        /// The device the caller named (for the message only).
        device: String,
    },

    /// The device a relink names has been revoked.
    ///
    /// A `403`, and permanently so: a revocation is terminal, and re-enrolling the
    /// machine mints a FRESH device id - so there is no sequence of calls that
    /// makes this id work again.
    #[error(
        "device {device} is revoked in {namespaces}; a revocation is terminal, so this \
         id can never be linked again in any account. Enrol the machine afresh - that \
         mints a new device id - and pair that"
    )]
    PairingDeviceRevoked {
        /// The device the caller named (for the message only).
        device: String,
        /// Debug rendering of the namespaces holding a tombstone for it.
        namespaces: String,
    },

    /// A `400`: an empty list means every application on the wire, so accepting
    /// it would turn the narrowest-looking request into the widest one.
    #[error(
        "a scope replacement must name at least one application; ask for every \
         application explicitly instead"
    )]
    ScopeReplacementEmpty,

    /// A `400`: the list is longer than a scope statement may carry, so it could
    /// never be signed into one.
    #[error("a scope replacement may name at most {limit} applications")]
    ScopeReplacementTooLarge {
        /// The cap the wire format puts on a scope statement.
        limit: usize,
    },

    /// A `400`: the list names an application no namespace of this account
    /// targets, so the replacement would reach nothing and descope everywhere.
    #[error(
        "this account takes part in no namespace targeting application \
         {application}, so naming it in a scope replacement would leave the \
         device reaching nothing"
    )]
    ScopeReplacementUnknownApplication {
        /// The application the caller named (for the message only).
        application: String,
    },

    /// A `409`: scope epochs only ever rise, and this device's has reached the
    /// last one there is, so no further statement can supersede what it holds.
    #[error(
        "device {device} is at the last scope epoch there is, so its scope can no \
         longer be replaced; revoke it and pair the machine afresh"
    )]
    ScopeEpochExhausted {
        /// The device the caller named (for the message only).
        device: String,
    },

    /// A `403`: the named device is the one holding the account root, which signs
    /// every scope statement - including any that would narrow itself.
    #[error(
        "device {device} holds the account root, so it always acts for every \
         application and there is nothing to replace; name one of the account's \
         paired devices instead"
    )]
    ScopeReplacementHoldsTheRoot {
        /// The device the caller named (for the message only).
        device: String,
    },

    /// A `400`: the name is empty, untrimmed, too long, or carries control
    /// characters, any of which would render as something other than a name.
    #[error(
        "a device name must be trimmed, non-empty, at most {limit} bytes and free \
         of control characters"
    )]
    DeviceLabelInvalid {
        /// The cap the wire format puts on a device label.
        limit: usize,
    },

    /// A `403`: a paired device holds no account root, so it can sign a
    /// statement about no device but its own.
    #[error(
        "this node may name only its own device ({own}), not {device}; run the \
         rename on the node holding the account root"
    )]
    DeviceLabelNotOwn {
        /// The device the caller named (for the message only).
        device: String,
        /// The device this node presents (for the message only).
        own: String,
    },

    /// A `429`: renaming is cheap to ask for and costs a published op each time,
    /// so this node spaces out its own.
    #[error("device {device} was renamed less than {cooldown_secs}s ago; try again shortly")]
    DeviceRenamedTooRecently {
        /// The device the caller named (for the message only).
        device: String,
        /// How long this node makes a caller wait between renames.
        cooldown_secs: u64,
    },

    /// A `404`: the namespace holds no binding for the device a revocation names,
    /// so there is no account to name in the op. Either it was never linked
    /// there, or its link has not synced to this node yet.
    #[error(
        "{namespace} holds no binding for device {device}, so there is no account to \
         name in the revocation. Either it was never linked here, or its link has \
         not synced to this node yet"
    )]
    RevocationUnknownDevice {
        /// Hex rendering of the namespace the caller named (for the message only).
        namespace: String,
        /// The device the caller named (for the message only).
        device: String,
    },

    /// A `403`: the device a revocation names is the one this node runs as.
    ///
    /// Revoking it from here would withdraw the identity this node signs the
    /// revocation with, part way through publishing it: the local apply lands and
    /// the publish that should carry it to peers can no longer be signed. Refused
    /// before anything is applied, because no retry on this node can succeed.
    #[error(
        "device {device} is the device this node runs as, so it cannot revoke it: the \
         revocation would withdraw the identity it is signed with before it reached any \
         peer. Revoke it from another device of the account, or leave the namespace to \
         retire this node"
    )]
    RevocationOfOwnDevice {
        /// The device the caller named (for the message only).
        device: String,
    },

    /// A `400`: a device link carried for another account does not verify, or its
    /// scope does not reach the namespace. Fixed by re-signing, not by retrying.
    #[error("cannot carry this device link: {reason}")]
    DeviceLinkInvalid {
        /// Why, from the governance store's refusal.
        reason: String,
    },

    /// A `403`: a device link this node will never carry — the device was
    /// revoked here, or the account is a member of nothing in the namespace, so
    /// this node's endorsement would vouch for a stranger.
    #[error("refusing to carry this device link: {reason}")]
    DeviceLinkRefused {
        /// Why, from the governance store's refusal.
        reason: String,
    },

    /// A `403`: the account replaced this device's scope with one that no longer
    /// reaches the namespace, so it may read on but must not author there.
    #[error(
        "this device's account narrowed its application scope out of the namespace \
         owning group '{group_id}', so it may no longer write there; widen the scope \
         with `PUT /admin-api/account/devices/{{id}}/scope` from the account holder"
    )]
    DeviceOutOfScope {
        /// Hex rendering of the target group id (for the message only).
        group_id: String,
    },

    /// A `409`: the group already has an upgrade in flight, so a second one (or a
    /// retry while its propagator still runs) would race it for the same status.
    #[error("an upgrade is already in progress for group {group_id}; wait for it to finish")]
    UpgradeInProgress {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `409`: the group already runs the target application, bytecode and all,
    /// so there is nothing to upgrade to until a new version is installed.
    #[error(
        "group {group_id} is already targeting this application; install a new version \
         of it to upgrade"
    )]
    UpgradeAlreadyTargeting {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `409`: the group holds no contexts, so a non-cascading upgrade has
    /// nothing to swap.
    #[error("group {group_id} has no contexts to upgrade")]
    UpgradeNoContexts {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `404`: a retry names a group that has never been upgraded.
    #[error("no upgrade found for group {group_id}")]
    UpgradeNotFound {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `409`: the group's upgrade is in a state a retry cannot act on, because
    /// no context of it failed.
    #[error("the upgrade of group {group_id} {reason}; nothing to retry")]
    UpgradeNotRetryable {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
        /// Why, as a clause: "is in progress with no failures", "is already completed".
        reason: &'static str,
    },

    /// A `400`: the group a leave names is a namespace root. Leaving it here
    /// would apply the cascade without unsubscribing from the namespace topic,
    /// which only the namespace leave does.
    #[error(
        "{group_id} is a namespace (root group); leave it with \
         POST /admin-api/namespaces/{{namespace_id}}/leave, which also unsubscribes \
         from the namespace gossipsub topic"
    )]
    LeaveGroupIsNamespace {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `409`: this node reaches the group only through a parent group, so it
    /// holds no membership row here to leave. The leave belongs where the
    /// membership anchor lives.
    #[error(
        "this node is not a direct member of {group_id}; leave the parent group \
         where the membership anchor lives instead"
    )]
    LeaveGroupNotDirectMember {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `409`: the upgrade gate refused the target, because moving the group
    /// or context onto it could not be proven safe (an identity downgrade, a
    /// schema downgrade or gap, no migration evidence, a signer that does not
    /// continue the running one). The request is understood; the caller has to
    /// pick, build or install a different target. The message is the gate's
    /// own, unchanged, since it says which.
    #[error("{reason}")]
    UpgradeRefused {
        /// What the gate refused and what to do instead, as a sentence.
        reason: String,
    },

    /// A `409`: the invitation's expiry has passed. Like a consumed invitation,
    /// nothing about this request can succeed; a fresh invitation can.
    #[error("invitation for group {group_id} expired at {expired_at} (unix seconds)")]
    InvitationExpired {
        /// Hex rendering of the group the invitation is for (for the message only).
        group_id: String,
        /// The invitation's `expiration_timestamp`.
        expired_at: u64,
    },

    /// A `400`: the invitation is missing something a join cannot proceed
    /// without, so it was minted or re-serialized by something that dropped it.
    #[error("invitation for group {group_id} is invalid: {reason}")]
    InvitationInvalid {
        /// Hex rendering of the group the invitation is for (for the message only).
        group_id: String,
        /// What is missing, and why the join refuses to default it.
        reason: &'static str,
    },

    /// A `503`: the join was published but no group key arrived in time. The
    /// node is catching up, not broken, and the identical join succeeds once
    /// an admitter delivers the key.
    #[error(
        "KeyDelivery timed out for group {group_id}: no group key arrived within \
         {waited_secs}s via the gossip fallback path; join cannot proceed without a \
         usable group key"
    )]
    JoinKeyDeliveryTimedOut {
        /// Hex rendering of the group being joined (for the message only).
        group_id: String,
        /// How long the join waited, in seconds.
        waited_secs: u64,
    },

    /// A `400`: a TEE admission or authoring policy the caller sent cannot be
    /// stored as given (set on a subgroup, or missing a measurement the
    /// admission gate requires, so it would refuse every node later instead of
    /// this request now). The message is the check's own, unchanged: it names
    /// the field and where to take its value from.
    #[error("{reason}")]
    TeePolicyInvalid {
        /// What is wrong with the policy and how to fix it, as a sentence.
        reason: String,
    },

    /// A `403`: an owner-level op needs a proof signed by the account's root
    /// key, none was supplied, and this node holds no root for the signing
    /// account to sign one itself. Only the root holder can help: sign the proof
    /// offline and pass it, or run the call on a node that holds the root.
    #[error("{reason}")]
    RootProofRequired {
        /// What is needed and how to supply it, as a sentence.
        reason: String,
    },

    /// A `403`: the operation needs this node to be a direct admin of the
    /// group, and it is not. About standing, like `NotAGroupMember`: only
    /// being granted the role helps.
    #[error("node is not a direct admin of group '{group_id}'")]
    NotAGroupAdmin {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `400`: an ownership-proof request the node will not sign as given (a
    /// field empty, over-long or carrying control characters, a short nonce,
    /// an expiry not in the future, a non-root namespace, a context outside
    /// the namespace). The message is the check's own and names the field.
    #[error("{reason}")]
    OwnershipProofInvalid {
        /// What is wrong with the request, naming the field.
        reason: String,
    },

    /// A `409`: a group with the id the caller chose is already held here.
    #[error("group '{group_id}' already exists")]
    GroupAlreadyExists {
        /// Hex rendering of the group the caller named (for the message only).
        group_id: String,
    },

    /// A `403`: a subgroup below the namespace root needs a namespace admin to
    /// create it; `CAN_CREATE_SUBGROUP` only reaches the root level.
    #[error(
        "creating a subgroup under non-root parent '{parent_id}' requires namespace admin \
         (delegated nested-subgroup creation is not yet supported)"
    )]
    SubgroupCreationNeedsNamespaceAdmin {
        /// Hex rendering of the parent the caller named (for the message only).
        parent_id: String,
    },

    /// A `400`: the `bytecode_id` a caller chose cannot be bound (zero, not an
    /// application bundle, or another package's bundle).
    #[error("{reason}")]
    BytecodeIdInvalid {
        /// What is wrong with the id, naming it.
        reason: String,
    },

    /// A `404`: the `bytecode_id` a caller chose is not a blob this node holds.
    /// Installing that version first makes the identical call succeed.
    #[error("bytecode_id blob '{blob_id}' is not present locally; install that version first")]
    BytecodeNotInstalled {
        /// The blob the caller named.
        blob_id: String,
    },

    /// A `403`: the caller is not an identity this node can act as in the
    /// context. The message names neither the caller nor the context, and is
    /// the same whether the key is unknown or a remote member's, so it cannot
    /// be used to learn who is a member.
    #[error("unauthorized: caller is not a permitted identity for this context")]
    CallerNotPermitted,

    /// A `409`: this node's state leaves it no way to name a device right now
    /// (it holds no usable device, follows no account namespace, or the
    /// device's name epoch is exhausted). The message says which.
    #[error("{reason}")]
    DeviceLabelUnavailable {
        /// Why the name cannot be minted, as a sentence.
        reason: String,
    },

    /// A `409`: a context seeded by the caller derives an id that is already
    /// held here, so the identical seed can never create a new one.
    #[error("seed resulted in an already existing context")]
    ContextSeedCollision,

    /// A `409`: an operator resync the context's state does not admit (it is
    /// not in a group, or holds local DAG heads a resync would discard and the
    /// caller did not pass `force`). The message says which, and how to proceed.
    #[error("context {context_id}: {reason}")]
    ResyncRefused {
        /// The context the caller named.
        context_id: String,
        /// Why the resync is refused.
        reason: String,
    },
}
