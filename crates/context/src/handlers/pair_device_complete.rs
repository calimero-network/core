//! `PairDeviceCompleteRequest` handler — certify a device another node minted,
//! link it, and hand it the scope key.
//!
//! The second half of pairing, run on the device that already holds the account.
//! It is two published ops, and both are load-bearing:
//!
//! 1. **`AccountDeviceLinked`** — an encrypted `GroupOp` carrying the device's
//!    certificate, signed by the account root, plus a member endorsement. This
//!    is what confers authority.
//! 2. **`RootOp::KeyDelivery`** — the current scope key, wrapped to the device's
//!    agreement key. Without it the link lands and the new device still cannot
//!    read anything, which is the failure this half exists to prevent.
//!
//! **Delivery has to be a cleartext `RootOp`, and that is not incidental.** The
//! pairing device holds no scope key, so a device-addressed envelope carried
//! inside an *encrypted* `GroupOp` would be unreadable by its only recipient —
//! the same bootstrap deadlock that keeps the member-addressed envelope alive.
//! `KeyDelivery` being a root op is what breaks the cycle.
//!
//! **Only the current key is delivered.** Peers retain rotated-out keys, so
//! history *could* be handed back, but doing so would make every newly paired
//! device a full-history reader — a capability decision that deserves its own
//! change rather than riding in on pairing. The cost is that the paired device
//! converges on forward state and cannot decrypt ops sealed under retired
//! epochs.
//!
//! **The scope is chosen by application, not by namespace.** A namespace is an
//! implementation unit nobody outside this crate named, so asking a caller to
//! pick one left the rest of the outcome undecided; an application list settles
//! which namespaces receive the binding and which scope keys are delivered, and
//! it is a question the person doing the pairing can actually answer. Naming no
//! application means all of them, which is what the fan-out did unconditionally
//! before.
//!
//! **Two checks before anything is signed.** The pairing device's statement
//! proves the key material came from the device that minted it, and the
//! confirmation code the caller must supply proves the account holder is
//! certifying the keys they were actually read. The first is a signature over
//! the payload, so an attacker holding both keys can always make it agree with
//! itself; the second is the value it cannot produce, because it arrives from the
//! other device by a channel it does not control. Neither is a substitute for the
//! other, and if the code travels beside the keys it describes it proves nothing
//! — that part is the operator's channel, not this handler's.
//!
//! **Two keys, one use each.** The account root signs the certificate; the
//! namespace identity signs the endorsement, the ops, and the key wrap. Crossing
//! them is silent, which is why the publisher takes ONE signing key and mints the
//! endorsement itself - see `device_link::publish_link_and_key`.

use std::collections::BTreeSet;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::{AccountId, DeviceCert, PairingOffer};
use calimero_context_client::group::{
    PairDeviceCompleteRequest, PairDeviceCompleteResponse, PairingScope,
};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{GroupKeyring, NamespaceRepository, NodeDeviceRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::warn;

use crate::error::ContextError;
use crate::handlers::list_namespaces::namespace_rows_for_applications;
use crate::ContextManager;

/// Where the link is published: everywhere this node takes part, narrowed to the
/// namespaces targeting one of `applications`.
///
/// Participation is the base set rather than the application resolution, because
/// publishing needs this node's identity and scope key — a namespace it merely
/// knows the metadata of is one it cannot author in.
///
/// An empty list is every namespace, which is what a caller who names no
/// application asks for and what the fan-out did unconditionally before.
fn namespaces_in_scope(
    store: &Store,
    applications: &[ApplicationId],
) -> EyreResult<Vec<ContextGroupId>> {
    let participating = NamespaceRepository::new(store).participating_namespaces()?;
    if applications.is_empty() {
        return Ok(participating);
    }

    let scoped: BTreeSet<[u8; 32]> = namespace_rows_for_applications(store, applications)?
        .into_iter()
        .map(|(group_id, _meta)| group_id)
        .collect();

    Ok(participating
        .into_iter()
        .filter(|namespace| scoped.contains(&namespace.to_bytes()))
        .collect())
}

/// The namespaces the pairing is *gated on*, and the namespaces the link is
/// *published into*.
///
/// The two differ only on the deprecated namespace-scoped route, which checks
/// the one namespace it was handed and still fans out to every one. Left as it
/// was: narrowing that fan-out would silently change what an existing caller
/// gets, and the route is on its way out.
fn resolve_scope(
    store: &Store,
    scope: &PairingScope,
) -> EyreResult<(Vec<ContextGroupId>, Vec<ContextGroupId>)> {
    Ok(match scope {
        PairingScope::Namespace(namespace_id) => {
            (vec![*namespace_id], namespaces_in_scope(store, &[])?)
        }
        // No namespace to name, so the resolved set answers for both.
        PairingScope::Applications(applications) => {
            let targets = namespaces_in_scope(store, applications)?;
            (targets.clone(), targets)
        }
    })
}

/// The application scope the certificate cache keeps for this pairing.
///
/// The deprecated namespace-scoped route names no application and fans out
/// unconditionally, which is the same reach an empty list means - so it stores
/// one rather than a narrower guess that would leave the device out of
/// namespaces the pairing itself covered.
fn stored_scope(scope: &PairingScope) -> Vec<ApplicationId> {
    match scope {
        PairingScope::Namespace(_) => Vec::new(),
        PairingScope::Applications(applications) => applications.clone(),
    }
}

/// The key this node signs the endorsement, both ops and the key wrap with.
///
/// One key serves every namespace - it is node-level - so the first that
/// resolves is the same key as any other. What the scan actually decides is
/// whether this node takes part in the scope at all: an empty answer means
/// either it is a stranger to the namespaces named, or the applications resolved
/// to nothing here.
fn signing_identity(store: &Store, gated_on: &[ContextGroupId]) -> EyreResult<[u8; 32]> {
    let namespaces = NamespaceRepository::new(store);
    for group_id in gated_on {
        match namespaces.resolve_identity(group_id) {
            Ok(Some((_public, secret))) => return Ok(secret),
            Ok(None) => {}
            Err(err) => warn!(?group_id, %err, "failed to resolve namespace identity"),
        }
    }
    // Typed so the admin API surfaces this precondition as a 409, not a generic
    // 500 (see `parse_api_error`).
    Err(ContextError::PairingNoNamespaceIdentity {
        namespaces: format!("{gated_on:?}"),
    }
    .into())
}

/// Is `statement` the offering device's own signature over exactly these keys?
fn check_statement(offer: &PairingOffer, statement: &[u8; 64]) -> EyreResult<()> {
    offer.verify_statement(statement).map_err(|err| {
        ContextError::PairingStatementInvalid {
            device: offer.device.to_string(),
            cause: err.to_string(),
        }
        .into()
    })
}

/// Does the code the account holder was read describe the keys that arrived?
///
/// The refusal deliberately does not echo the expected code: an attacker that
/// can drive this endpoint would otherwise learn the value it needs.
fn check_confirmation_code(offer: &PairingOffer, supplied: &str) -> EyreResult<()> {
    if offer.code_matches(supplied) {
        return Ok(());
    }
    Err(ContextError::PairingCodeMismatch {
        device: offer.device.to_string(),
    }
    .into())
}

/// Is this node itself a device of `account`, and so able to certify a second?
///
/// A node that paired INTO somebody else's account holds a device whose account
/// its own root cannot name, so it has no standing to certify: it would mint a
/// certificate for an account it does not hold and the link would be refused
/// downstream.
fn require_this_node_holds(store: &Store, account: AccountId) -> EyreResult<()> {
    match NodeDeviceRepository::new(store).get()? {
        Some(enrolled) if enrolled.account == account => Ok(()),
        Some(enrolled) => Err(ContextError::PairingNotTheAccountHolder {
            enrolled: enrolled.account.to_string(),
            account: account.to_string(),
        }
        .into()),
        None => eyre::bail!(
            "this node has enrolled no device; joining a namespace enrols one, \
             and it has to happen before pairing a second"
        ),
    }
}

/// Does this node hold a current scope key in any of `namespaces`?
///
/// One is enough. Pairing publishes an encrypted group op and delivers that same
/// key, so it needs a key *somewhere* in the scope to do either; the namespaces
/// past the first are the fan-out's business, and it skips the ones it cannot
/// publish into.
fn require_a_scope_key(store: &Store, namespaces: &[ContextGroupId]) -> EyreResult<()> {
    for namespace_id in namespaces {
        if GroupKeyring::new(store, *namespace_id)
            .load_current_key()?
            .is_some()
        {
            return Ok(());
        }
    }
    Err(ContextError::PairingNoScopeKey {
        namespaces: format!("{namespaces:?}"),
    }
    .into())
}

impl Handler<PairDeviceCompleteRequest> for ContextManager {
    type Result = ActorResponse<Self, <PairDeviceCompleteRequest as Message>::Result>;

    fn handle(
        &mut self,
        PairDeviceCompleteRequest {
            scope,
            device,
            kem_pk,
            sign_pk,
            statement,
            confirmation_code,
        }: PairDeviceCompleteRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();

        // The scope the cert store records, so a namespace gained later can be
        // reached without re-pairing.
        let applications = stored_scope(&scope);

        // Resolved before any check, because the scope is what the checks are
        // about: which namespaces have to hold an identity and a key for this
        // pairing to be able to do anything.
        let (gated_on, targets) = match resolve_scope(&store, &scope) {
            Ok(resolved) => resolved,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // The namespace identity signs the endorsement, both ops, and the key
        // wrap. It must be a granted member: the endorsement is what carries the
        // link past the apply gate, and an endorsement from a non-member is
        // refused.
        let signer_sk_bytes = match signing_identity(&store, &gated_on) {
            Ok(identity) => identity,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);

        let device_repo = NodeDeviceRepository::new(&store);

        // The account root is what certifies the device, and it is also what
        // decides *which* account this node can pair into: the genesis is the
        // content address of this node's root key, so it can only ever certify
        // devices for the one account that root owns.
        let account_root = match device_repo.require_account_root() {
            Ok(root) => root,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let genesis = account_root.genesis();
        let account = genesis.account_id();

        // Check the key material before anything is signed over it. The
        // certificate minted below is what makes these keys a trusted device of
        // this account, and until this point they are three values a caller
        // supplied: an attacker who can alter the pairing payload substitutes its
        // own keys under a captured `DeviceId` and receives the scope-key
        // fan-out. The statement is the pairing device's own signature over
        // exactly what is being certified, so it can only be produced by
        // whoever holds the signing key it names.
        //
        // It does not cover a substitution that replaces both keys and re-signs
        // — nothing here has a prior commitment to the genuine ones, and binding
        // them into the `DeviceId` is ruled out because the id must survive key
        // rotation. The confirmation code returned below is what closes that,
        // out of band and by a person.
        let offer = PairingOffer::new(account, device, kem_pk, sign_pk);
        if let Err(err) = check_statement(&offer, &statement) {
            // Logged, not just returned: this is the security-relevant event the
            // check exists for, and the error otherwise reaches only whoever made
            // the request — possibly the attacker rather than an operator reading
            // logs. Ids only; no key material.
            warn!(
                namespaces = gated_on.len(),
                %account,
                %device,
                %err,
                "refusing to certify device: pairing statement invalid"
            );
            return ActorResponse::reply(Err(err));
        }

        // The statement proves the keys and the signature agree with each other,
        // which an attacker holding both can arrange. The code is the value it
        // cannot produce: the account holder was read it from the pairing
        // device's own output, so it describes the keys that device minted, and
        // here it is checked against the keys that actually arrived.
        if let Err(err) = check_confirmation_code(&offer, &confirmation_code) {
            // The warn carries no `err`, for the same reason the refusal carries
            // no expected code.
            warn!(
                namespaces = gated_on.len(),
                %account,
                %device,
                "refusing to certify device: confirmation code does not match the \
                 key material offered"
            );
            return ActorResponse::reply(Err(err));
        }

        if let Err(err) = require_this_node_holds(&store, account) {
            return ActorResponse::reply(Err(err));
        }

        // One precondition covers both ops: the link is an encrypted group op so
        // publishing it needs the current key, and the delivery is that same key
        // wrapped for the new device. Checking it here, before anything is
        // signed, beats failing deep inside the publisher.
        //
        // Only the namespaces this pairing is gated on are a precondition. The
        // fan-out below reaches the others too, but a missing key there is a
        // reason to skip that namespace rather than to refuse the pairing the
        // caller asked for.
        if let Err(err) = require_a_scope_key(&store, &gated_on) {
            return ActorResponse::reply(Err(err));
        }

        // Epoch 0 on both counts: the account root has not rotated (rotation is
        // not implemented yet), so there are no handoffs to carry and the
        // certifying key is the genesis key itself.
        let cert = match DeviceCert::sign(
            account_root.signing_key(),
            account,
            device,
            &sign_pk,
            &kem_pk,
            0,
            0,
        ) {
            Ok(cert) => cert,
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to sign the device certificate: {err}"
                )))
            }
        };

        // The device this certifies needs the proof to present itself, and cannot
        // read it off the DAG until it is a member somewhere - which for a thin
        // client is never. The same proof is what the publisher below builds each
        // link from, and what the cert store keeps, so a namespace this account
        // gains later can bind the device without a second pairing ceremony.
        let credential = Box::new(calimero_account::AccountProof {
            genesis,
            chain: vec![],
            statement: cert,
        });

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                // A device belongs to an account, not to a scope, so both
                // credentials the link carries are account-scoped: the certificate
                // is signed by the account root, and the endorsement by this node's
                // signing key, which is node-level. Neither says anything about a
                // namespace.
                //
                // What does vary per namespace is the scope key, so each one gets
                // its own delivery wrapped under the same device KEM key.
                //
                // The set is the caller's applications resolved, or every namespace
                // this node takes part in when they named none. It does not have to
                // agree with what the device subscribed to at pair-init: a binding
                // published somewhere the device is not listening still lands on
                // that namespace's DAG and is picked up whenever it does subscribe,
                // and a subscription this never reaches costs the device nothing.
                // Only prompt delivery depends on the two overlapping.
                let mut linked_in = Vec::new();
                let mut key_delivered_everywhere = true;

                for ns in targets {
                    // No key here means this node cannot publish an encrypted group
                    // op for the namespace, let alone deliver one. Skip rather than
                    // fail: the scope the caller asked about is checked above, and
                    // the others are a bonus this pairing is extending.
                    let Ok(Some((_key_id, ns_key))) =
                        GroupKeyring::new(&store, ns).load_current_key()
                    else {
                        continue;
                    };

                    match calimero_governance_store::device_link::publish_link_and_key(
                        &store,
                        &node_client,
                        &ack_router,
                        &ns,
                        &signer_sk,
                        &credential,
                        &ns_key,
                    )
                    .await
                    {
                        Ok(key_delivered) => {
                            key_delivered_everywhere &= key_delivered;
                            linked_in.push(ns);
                        }
                        // One namespace failing must not withhold the device from
                        // the rest. The caller sees which ones landed.
                        Err(err) => warn!(namespace_id = ?ns, %device, %err,
                                          "pairing: extending the device here failed; \
                                           others continue"),
                    }
                }

                if linked_in.is_empty() {
                    eyre::bail!(
                        "the link for {device} reached no namespace, so it is paired \
                         nowhere and holds no scope key"
                    );
                }

                // Kept only once the pairing reached somewhere, so a call that
                // failed leaves nothing behind. From here on a namespace this
                // account gains binds the device on its own, because the root
                // signature - which the replicated binding row drops - is written
                // down where it was made.
                if let Err(err) = NodeDeviceRepository::new(&store)
                    .remember_device_cert(&credential, &applications)
                {
                    warn!(%device, %err,
                          "paired, but this node could not remember the certificate; \
                           namespaces gained later will need an explicit relink");
                }

                let key_delivered = key_delivered_everywhere;

                Ok(PairDeviceCompleteResponse::new(
                    account,
                    device,
                    key_delivered,
                    confirmation_code,
                    credential,
                ))
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountGenesis;
    use calimero_governance_store::MetaRepository;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;

    use super::*;

    const NS_A: [u8; 32] = [0xA1; 32];
    const NS_B: [u8; 32] = [0xB2; 32];
    const NS_C: [u8; 32] = [0xC3; 32];
    /// Known to this node by its metadata row, but never joined.
    const NS_STRANGER: [u8; 32] = [0xD4; 32];
    /// Taken part in with no metadata row yet - what a freshly provisioned
    /// namespace looks like before its governance state has synced.
    const NS_UNSYNCED: [u8; 32] = [0xE5; 32];
    const APP_ONE: [u8; 32] = [0x11; 32];
    const APP_TWO: [u8; 32] = [0x22; 32];
    /// Served by no namespace here - somebody else's application.
    const APP_ELSEWHERE: [u8; 32] = [0x33; 32];

    fn app(id: [u8; 32]) -> ApplicationId {
        ApplicationId::from(id)
    }

    /// `NS_A` and `NS_C` serve `APP_ONE`, `NS_B` serves `APP_TWO`, `NS_STRANGER`
    /// serves `APP_ONE` but is not one this node takes part in, and `NS_UNSYNCED`
    /// is taken part in but names no application yet.
    fn namespaces_this_node_speaks_in() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let meta = MetaRepository::new(&store);
        let namespaces = NamespaceRepository::new(&store);
        for (id, application) in [
            (NS_A, Some(APP_ONE)),
            (NS_B, Some(APP_TWO)),
            (NS_C, Some(APP_ONE)),
            (NS_STRANGER, Some(APP_ONE)),
            (NS_UNSYNCED, None),
        ] {
            if let Some(application) = application {
                meta.save(
                    &id.into(),
                    &GroupMetaValue {
                        bytecode_id: [0xAA; 32],
                        target_application_id: app(application),
                        created_at: 1_700_000_000,
                        admin_identity: calimero_account::AccountId::from([0x01; 32]),
                        owner_identity: calimero_account::AccountId::from([0x01; 32]),
                        migration: None,
                        auto_join: true,
                    },
                )
                .expect("save the namespace metadata");
            }
            if id != NS_STRANGER {
                namespaces
                    .note_participation(&id.into())
                    .expect("take part in the namespace");
            }
        }
        store
    }

    fn sorted(namespaces: Vec<ContextGroupId>) -> Vec<[u8; 32]> {
        let mut ids: Vec<_> = namespaces.into_iter().map(|ns| ns.to_bytes()).collect();
        ids.sort_unstable();
        ids
    }

    /// What the certificate cache is handed, which decides every namespace this
    /// device reaches from here on. The deprecated route fanned out to everything,
    /// so storing anything narrower for it would shrink a pairing after the fact.
    #[test]
    fn the_stored_scope_is_what_the_pairing_actually_reached() {
        assert!(stored_scope(&PairingScope::Namespace(NS_A.into())).is_empty());
        assert!(stored_scope(&PairingScope::Applications(vec![])).is_empty());
        assert_eq!(
            stored_scope(&PairingScope::Applications(vec![app(APP_ONE)])),
            vec![app(APP_ONE)],
        );
    }

    #[test]
    fn an_application_scope_reaches_that_applications_namespaces_and_no_others() {
        let store = namespaces_this_node_speaks_in();

        let (gated_on, targets) =
            resolve_scope(&store, &PairingScope::Applications(vec![app(APP_ONE)]))
                .expect("resolve the scope");

        assert_eq!(sorted(targets), vec![NS_A, NS_C]);
        assert_eq!(
            sorted(gated_on),
            vec![NS_A, NS_C],
            "an application scope names no namespace, so the resolved set is both \
             what is checked and what is published into"
        );
    }

    /// `NS_UNSYNCED` is the case that makes this more than a shortcut: it is
    /// taken part in and names no application, so resolving an empty list through
    /// the application filter would drop it and pair the device into fewer
    /// namespaces than the unconditional fan-out reached.
    #[test]
    fn naming_no_application_reaches_every_participating_namespace() {
        let store = namespaces_this_node_speaks_in();

        let (_, targets) =
            resolve_scope(&store, &PairingScope::Applications(vec![])).expect("resolve the scope");

        assert_eq!(
            sorted(targets),
            vec![NS_A, NS_B, NS_C, NS_UNSYNCED],
            "an empty application list is the unconditional fan-out this replaces"
        );
    }

    /// Publishing needs this node's own identity and scope key, so a namespace it
    /// merely holds the metadata of is one it cannot author in - however well the
    /// application matches.
    #[test]
    fn a_namespace_this_node_only_knows_of_is_not_a_target() {
        let store = namespaces_this_node_speaks_in();

        for scope in [
            PairingScope::Applications(vec![app(APP_ONE)]),
            PairingScope::Applications(vec![]),
            PairingScope::Namespace(NS_A.into()),
        ] {
            let (_, targets) = resolve_scope(&store, &scope).expect("resolve the scope");
            assert!(!sorted(targets).contains(&NS_STRANGER));
        }
    }

    #[test]
    fn the_namespace_scoped_route_gates_on_one_and_still_fans_out_to_all() {
        let store = namespaces_this_node_speaks_in();

        let (gated_on, targets) = resolve_scope(&store, &PairingScope::Namespace(NS_B.into()))
            .expect("resolve the scope");

        assert_eq!(sorted(gated_on), vec![NS_B]);
        assert_eq!(sorted(targets), vec![NS_A, NS_B, NS_C, NS_UNSYNCED]);
    }

    /// The device subscribes to what it was told at pair-init and the holder
    /// publishes into what its own scope resolves to; nothing reconciles the two.
    /// A binding published where the device is not listening still lands on that
    /// namespace's DAG, and a subscription this never reaches costs it nothing -
    /// so the mismatch is tolerated rather than refused.
    #[test]
    fn a_scope_disjoint_from_what_the_device_subscribed_to_is_not_an_error() {
        let store = namespaces_this_node_speaks_in();
        let subscribed = [ContextGroupId::from(NS_A), ContextGroupId::from(NS_B)];
        let device = NodeDeviceRepository::new(&store)
            .ensure_enrolled_into(
                &subscribed,
                AccountGenesis::new(PrivateKey::from([0x07; 32]).public_key()),
            )
            .expect("mint the device against the namespaces it was told about");

        let (_, targets) = resolve_scope(&store, &PairingScope::Applications(vec![app(APP_TWO)]))
            .expect("resolve a scope that does not cover NS_A");

        assert_eq!(sorted(targets), vec![NS_B]);
        assert_eq!(
            NodeDeviceRepository::new(&store)
                .get()
                .expect("read")
                .expect("present")
                .device(),
            device.device(),
            "resolving a narrower scope consults no device row and re-mints nothing"
        );
    }

    /// An application this node serves nowhere leaves the scope empty, which is
    /// what the refusal in `handle` rests on: no namespace means no identity to
    /// sign the endorsement with and no key to deliver, so the pairing would be
    /// reported as done having reached nothing.
    #[test]
    fn an_application_this_node_serves_nowhere_resolves_to_no_namespace() {
        let store = namespaces_this_node_speaks_in();

        let (gated_on, targets) = resolve_scope(
            &store,
            &PairingScope::Applications(vec![app(APP_ELSEWHERE)]),
        )
        .expect("resolve the scope");

        assert!(gated_on.is_empty());
        assert!(targets.is_empty());
    }

    /// One key anywhere in the scope clears the precondition, and it has to be a
    /// key in *this* scope: the fan-out skips namespaces it cannot publish into,
    /// so a scope holding none would certify a device and deliver it nothing.
    #[test]
    fn one_scope_key_anywhere_in_the_set_clears_the_precondition() {
        let store = namespaces_this_node_speaks_in();
        let scope = [ContextGroupId::from(NS_A), ContextGroupId::from(NS_B)];

        let refused = require_a_scope_key(&store, &scope).expect_err("no key anywhere yet");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingNoScopeKey { .. })
        ));

        GroupKeyring::new(&store, NS_B.into())
            .store_key(&[0x42; 32])
            .expect("store a scope key in the second namespace only");

        assert!(require_a_scope_key(&store, &scope).is_ok());
        assert!(
            require_a_scope_key(&store, &[ContextGroupId::from(NS_A)]).is_err(),
            "a key held somewhere else is not a key held in this scope"
        );
    }

    /// A namespace this node merely knows of has no identity to sign with, so a
    /// scope that reaches only those is refused rather than certifying a device
    /// no endorsement can carry.
    #[test]
    fn a_scope_this_node_signs_nowhere_in_is_refused() {
        let store = namespaces_this_node_speaks_in();
        let scope = [ContextGroupId::from(NS_A), ContextGroupId::from(NS_B)];

        let refused = signing_identity(&store, &scope).expect_err("no key provisioned yet");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingNoNamespaceIdentity { .. })
        ));

        let node = PrivateKey::from([0x21; 32]);
        NamespaceRepository::new(&store)
            .store_identity(&NS_B.into(), &node.public_key(), node.as_bytes())
            .expect("provision this node's signing key");

        assert_eq!(
            PrivateKey::from(
                signing_identity(&store, &scope).expect("one identity anywhere is enough")
            )
            .public_key(),
            node.public_key(),
        );
    }

    /// The one thing an attacker holding both keys cannot arrange. It is checked
    /// against the keys that ARRIVED, so a substituted payload fails it even
    /// though its statement verifies.
    #[test]
    fn a_code_for_other_key_material_is_refused_without_echoing_the_expected_one() {
        let device_sk = PrivateKey::from([0x31; 32]);
        let account = AccountGenesis::new(PrivateKey::from([0x32; 32]).public_key()).account_id();
        let device = calimero_account::DeviceId::from([0x33; 32]);
        let kem_pk = calimero_account::KemPublicKey::from([0x34; 32]);
        let (offer, _) =
            PairingOffer::signed(&device_sk, account, device, kem_pk).expect("mint the offer");
        let substituted = PairingOffer::new(
            account,
            device,
            calimero_account::KemPublicKey::from([0xAA; 32]),
            device_sk.public_key(),
        );

        assert!(check_confirmation_code(&offer, &offer.confirmation_code()).is_ok());

        let refused = check_confirmation_code(&substituted, &offer.confirmation_code())
            .expect_err("the code describes keys that did not arrive");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingCodeMismatch { .. })
        ));
        assert!(
            !refused
                .to_string()
                .contains(&substituted.confirmation_code()),
            "echoing the expected code hands an attacker the one value it cannot compute"
        );
    }

    /// Re-signing the substituted material is what the statement does NOT close;
    /// altering it in transit is what it does.
    #[test]
    fn a_statement_over_other_key_material_is_refused() {
        let device_sk = PrivateKey::from([0x41; 32]);
        let account = AccountGenesis::new(PrivateKey::from([0x42; 32]).public_key()).account_id();
        let device = calimero_account::DeviceId::from([0x43; 32]);
        let (offer, statement) = PairingOffer::signed(
            &device_sk,
            account,
            device,
            calimero_account::KemPublicKey::from([0x44; 32]),
        )
        .expect("mint the offer");
        let altered = PairingOffer::new(
            account,
            device,
            calimero_account::KemPublicKey::from([0xBB; 32]),
            device_sk.public_key(),
        );

        assert!(check_statement(&offer, &statement).is_ok());

        let refused =
            check_statement(&altered, &statement).expect_err("the keys were altered in transit");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingStatementInvalid { .. })
        ));
    }

    /// A node that paired INTO somebody else's account cannot certify a third
    /// device: its root cannot name the account its own device row holds.
    #[test]
    fn a_paired_node_cannot_certify_for_the_account_its_own_root_owns() {
        let store = namespaces_this_node_speaks_in();
        let repo = NodeDeviceRepository::new(&store);
        let adopted = repo
            .ensure_enrolled_into(
                &[ContextGroupId::from(NS_A)],
                AccountGenesis::new(PrivateKey::from([0x51; 32]).public_key()),
            )
            .expect("adopt somebody else's account");

        assert!(require_this_node_holds(&store, adopted.account).is_ok());

        let own = repo.ensure_account_root().expect("generate").account();
        let refused = require_this_node_holds(&store, own)
            .expect_err("the adopted row does not name this node's own account");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingNotTheAccountHolder { .. })
        ));
    }
}
