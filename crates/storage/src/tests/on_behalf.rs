//! Entries a relay writes on an account's behalf.
//!
//! A relay executing a call for an account signs the result with its own key,
//! and says so: [`SignatureData::signer`] names the relay's key — the one the
//! signature verifies under — and [`SignatureData::on_behalf`] names the
//! account it wrote for. These tests play the receiving node through
//! `Interface::apply_action` and the snapshot verifiers.
//!
//! What storage decides, and what it does not:
//!
//! * The signature verifies under `signer`, exactly as for a direct write. The
//!   signed payload covers `on_behalf`, so the field cannot be changed in
//!   transit.
//! * Ownership and the writer-set check are asked of the AUTHOR account. The
//!   node hands it over as `ApplyContext::signer_account`, having decided
//!   whether the relay may author for that account — storage holds no roles,
//!   so it cannot. For an on-behalf entry storage insists the resolution names
//!   exactly the `on_behalf` account: anything else (the relay's own account, a
//!   third account, nothing) is refused.
//! * A direct entry (`on_behalf: None`) verifies exactly as before.

use ed25519_dalek::SigningKey;
use serial_test::serial;

use calimero_account::AccountId;

use crate::action::Action;
use crate::address::Id;
use crate::collections::{Authored, Frozen, LwwRegister, Root, UnorderedMap};
use crate::entities::{ChildInfo, Data, Metadata, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{ApplyContext, AuthorVerdict, Interface, StorageError};
use crate::store::{Key, MainStorage, MockedStorage, StorageAdaptor};
use crate::tests::common::{
    account_of_key, apply_ctx_for, build_signed_member_action, build_signed_shared_action, cell_at,
    create_signed_user_add_action, create_test_keypair, create_test_owner, owned_element,
    pubkey_of, sign_action, Page,
};

type MainInterface = Interface<MainStorage>;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn sig_data_mut(action: &mut Action) -> &mut SignatureData {
    let metadata = match action {
        Action::Add { metadata, .. }
        | Action::Update { metadata, .. }
        | Action::DeleteRef { metadata, .. } => metadata,
    };
    let nonce = *metadata.updated_at;
    match &mut metadata.storage_type {
        StorageType::User { signature_data, .. }
        | StorageType::Shared { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => {
            // A locally applied entry has no signature yet (the node signs it
            // after the run); start it from the placeholder the node would.
            signature_data.get_or_insert(SignatureData {
                signature: [0; 64],
                nonce,
                signer: None,
                on_behalf: None,
            })
        }
        other => panic!("not a signed storage type: {other:?}"),
    }
}

fn metadata_of(action: &Action) -> &Metadata {
    match action {
        Action::Add { metadata, .. }
        | Action::Update { metadata, .. }
        | Action::DeleteRef { metadata, .. } => metadata,
    }
}

/// Re-sign `action` as a relay would ship it: the relay's key named as the
/// signer, `account` as the author it wrote for, and a signature by the relay
/// over that exact payload.
fn relayed(mut action: Action, relay: &SigningKey, account: Option<AccountId>) -> Action {
    {
        let sd = sig_data_mut(&mut action);
        sd.signer = Some(pubkey_of(relay));
        sd.on_behalf = account;
    }
    let signature = sign_action(&action, relay);
    sig_data_mut(&mut action).signature = signature;
    action
}

/// Change `on_behalf` after signing, as a tampering peer would.
fn retargeted(mut action: Action, account: Option<AccountId>) -> Action {
    sig_data_mut(&mut action).on_behalf = account;
    action
}

fn ctx(signer_account: Option<AccountId>) -> ApplyContext {
    ApplyContext {
        signer_account,
        ..ApplyContext::empty()
    }
}

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

/// A `User` add for a fresh owned entry, signed by the owner's own device.
fn owned_add(device: &SigningKey, owner: AccountId) -> Action {
    let page = Page::new_from_element("Account profile", owned_element(owner));
    let data = borsh::to_vec(&page).expect("serialize");
    create_signed_user_add_action(device, owner, page.id(), data, env::time_now())
}

/// The delegated-write repro, in its fixed form: a relay runs a call for an
/// account and signs the `User` entry with its own key. It names that key as
/// the signer and the account as the author, and every receiver that resolves
/// the author to the owner applies it — where before it named the author's
/// DEVICE and was refused everywhere for a signature that did not verify.
#[test]
#[serial]
fn a_relayed_user_write_applies_when_its_author_resolves_to_the_owner() {
    env::reset_for_testing();
    let (author_device, owner) = create_test_owner();
    let (relay, _) = create_test_keypair();

    let shipped = relayed(owned_add(&author_device, owner), &relay, Some(owner));
    let sd = *sig_data_mut(&mut shipped.clone());
    assert_eq!(
        sd.signer,
        Some(pubkey_of(&relay)),
        "signer is the signing key"
    );
    assert_eq!(sd.on_behalf, Some(owner));

    MainInterface::apply_action(shipped, &apply_ctx_for(owner))
        .expect("an on-behalf entry whose author resolves to its owner applies");
}

#[test]
#[serial]
fn a_relayed_user_write_is_refused_once_its_on_behalf_is_changed() {
    env::reset_for_testing();
    let (author_device, owner) = create_test_owner();
    let (relay, _) = create_test_keypair();
    let (_, other) = create_test_owner();

    let shipped = relayed(owned_add(&author_device, owner), &relay, Some(owner));
    for (tampered, resolved) in [
        // Stripped: now reads as the relay's own write.
        (retargeted(shipped.clone(), None), owner),
        // Swapped for another account the relay serves.
        (retargeted(shipped.clone(), Some(other)), other),
    ] {
        assert!(matches!(
            MainInterface::apply_action(tampered, &apply_ctx_for(resolved)),
            Err(StorageError::InvalidSignature)
        ));
    }
}

#[test]
#[serial]
fn a_relayed_user_write_is_refused_unless_its_author_resolves_to_the_owner() {
    env::reset_for_testing();
    let (author_device, owner) = create_test_owner();
    let (relay, _) = create_test_keypair();
    let relay_account = account_of_key(&relay);
    let (_, stranger) = create_test_owner();

    let shipped = relayed(owned_add(&author_device, owner), &relay, Some(owner));
    for resolved in [None, Some(relay_account), Some(stranger)] {
        assert!(
            matches!(
                MainInterface::apply_action(shipped.clone(), &ctx(resolved)),
                Err(StorageError::InvalidSignature)
            ),
            "resolved author {resolved:?} must not own the entry"
        );
    }
}

/// The resolution must name the on-behalf account itself. A resolution that
/// names the owner while the entry says it was written for someone else is not
/// the author of THIS entry — refused, even though it matches `owner`.
#[test]
#[serial]
fn a_relayed_user_write_for_another_account_is_refused_even_resolved_to_the_owner() {
    env::reset_for_testing();
    let (author_device, owner) = create_test_owner();
    let (relay, _) = create_test_keypair();
    let (_, other) = create_test_owner();

    let shipped = relayed(owned_add(&author_device, owner), &relay, Some(other));
    assert!(matches!(
        MainInterface::apply_action(shipped, &apply_ctx_for(owner)),
        Err(StorageError::InvalidSignature)
    ));
}

#[test]
#[serial]
fn a_direct_user_write_verifies_exactly_as_before() {
    env::reset_for_testing();
    let (device, owner) = create_test_owner();
    let (_, stranger) = create_test_owner();

    let direct = owned_add(&device, owner);
    assert_eq!(sig_data_mut(&mut direct.clone()).on_behalf, None);
    for resolved in [None, Some(stranger)] {
        assert!(matches!(
            MainInterface::apply_action(direct.clone(), &ctx(resolved)),
            Err(StorageError::InvalidSignature)
        ));
    }
    MainInterface::apply_action(direct, &apply_ctx_for(owner)).expect("owner's own write");
}

/// Why a `User` write is refused, as the apply path's diagnostics name it.
#[test]
fn the_user_refusal_names_which_check_failed() {
    env::reset_for_testing();
    let (author_device, owner) = create_test_owner();
    let (relay, _) = create_test_keypair();
    let (_, other) = create_test_owner();

    let verdict = |action: &Action, resolved: Option<AccountId>| {
        let StorageType::User {
            owner,
            signature_data: Some(sd),
            ..
        } = &metadata_of(action).storage_type
        else {
            panic!("a signed user action");
        };
        MainInterface::user_action_verdict(
            sd,
            &action.payload_for_signing(),
            owner,
            resolved.as_ref(),
        )
    };

    let shipped = relayed(owned_add(&author_device, owner), &relay, Some(owner));
    assert_eq!(verdict(&shipped, Some(owner)), AuthorVerdict::Authorized);
    assert_eq!(
        verdict(&retargeted(shipped.clone(), Some(other)), Some(other)),
        AuthorVerdict::BadSignature
    );
    assert_eq!(verdict(&shipped, None), AuthorVerdict::AuthorUnresolved);
    assert_eq!(verdict(&shipped, Some(other)), AuthorVerdict::WrongAuthor);

    assert_eq!(AuthorVerdict::BadSignature.reason(), Some("bad-signature"));
    assert_eq!(AuthorVerdict::WrongAuthor.reason(), Some("wrong-author"));
    assert_eq!(
        AuthorVerdict::AuthorUnresolved.reason(),
        Some("author-unresolved")
    );
    assert_eq!(AuthorVerdict::Authorized.reason(), None);
}

type Notes = Authored<UnorderedMap<String, LwwRegister<String>>>;

/// A relay deletes an account's own entry for it: applied when the author
/// resolves to the owner, refused for a different on-behalf account.
#[test]
#[serial]
fn a_relayed_user_delete_is_judged_against_its_on_behalf_account() {
    env::reset_for_testing();
    let alice_device = key(0xA1);
    let alice = account_of_key(&alice_device);
    env::set_account_id(*alice.as_bytes());
    let mut notes = Root::new(Notes::new);
    notes
        .insert("n".to_owned(), LwwRegister::new("hi".to_owned()))
        .expect("insert");
    let id = notes.entry_id(&"n".to_owned());
    let relay = key(0xB7);
    let other = account_of_key(&key(0xC3));

    let at = env::time_now() + 1_000_000_000;
    let StorageType::User { rules, .. } = <Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .expect("present")
        .storage_type
    else {
        panic!("an owned entry");
    };
    let removal = |on_behalf| {
        let metadata = Metadata {
            created_at: at,
            updated_at: at.into(),
            storage_type: StorageType::User {
                owner: alice,
                rules,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce: at,
                    signer: None,
                    on_behalf: None,
                }),
            },
            ..Metadata::default()
        };
        relayed(
            Action::DeleteRef {
                id,
                deleted_at: at,
                metadata,
            },
            &relay,
            on_behalf,
        )
    };

    assert!(
        MainInterface::apply_action(removal(Some(other)), &apply_ctx_for(alice)).is_err(),
        "written for another account"
    );
    assert!(
        MainInterface::apply_action(removal(Some(alice)), &apply_ctx_for(account_of_key(&relay)))
            .is_err(),
        "the relay as itself owns nothing"
    );
    assert!(MainStorage::storage_read(Key::Entry(id)).is_some());
    MainInterface::apply_action(removal(Some(alice)), &apply_ctx_for(alice))
        .expect("the relay deletes alice's entry for her");
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

fn setup_root<A: StorageAdaptor>() -> ChildInfo {
    let root_meta = Metadata::default();
    Index::<A>::add_root(ChildInfo::new(Id::root(), [0; 32], root_meta.clone()))
        .expect("register root");
    let (full_hash, _) = Index::<A>::get_hashes_for(Id::root())
        .expect("root hashes")
        .expect("root present");
    ChildInfo::new(Id::root(), full_hash, root_meta)
}

/// A cell alice writes, bootstrapped on `A`, and an update to it written by
/// alice's own device — the action the relay variants are derived from.
fn shared_cell<A: StorageAdaptor>() -> (Id, SigningKey, AccountId, Action) {
    env::reset_for_testing();
    let alice_device = key(0xA1);
    let alice = account_of_key(&alice_device);
    let writers = [alice].into_iter().collect();
    let id = cell_at(0x5A, &writers);
    let root = setup_root::<A>();
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        writers.clone(),
        1_000,
        &alice_device,
        vec![root],
    );
    Interface::<A>::apply_action(bootstrap, &apply_ctx_for(alice)).expect("bootstrap");
    let update = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        writers,
        2_000,
        &alice_device,
        vec![],
    );
    (id, alice_device, alice, update)
}

#[test]
fn a_relayed_shared_write_applies_for_a_writer_it_resolves_to() {
    type A = MockedStorage<9500>;
    let (id, _, alice, update) = shared_cell::<A>();
    let relay = key(0xB7);
    Interface::<A>::apply_action(relayed(update, &relay, Some(alice)), &apply_ctx_for(alice))
        .expect("relayed write for a writer applies");
    assert_eq!(A::storage_read(Key::Entry(id)), Some(b"v1".to_vec()));
}

#[test]
fn a_relayed_shared_write_is_refused_unless_resolved_to_its_on_behalf_writer() {
    type A = MockedStorage<9501>;
    let (_, _, alice, update) = shared_cell::<A>();
    let relay = key(0xB7);
    let stranger = account_of_key(&key(0xC3));
    let shipped = relayed(update.clone(), &relay, Some(alice));

    for (action, resolved, why) in [
        (shipped.clone(), None, "unresolved"),
        (
            shipped.clone(),
            Some(account_of_key(&relay)),
            "the relay as itself is not a writer",
        ),
        (
            retargeted(shipped.clone(), None),
            Some(alice),
            "on_behalf stripped after signing",
        ),
        (
            relayed(update.clone(), &relay, Some(stranger)),
            Some(alice),
            "written for a non-writer, resolution names a writer",
        ),
    ] {
        assert!(
            matches!(
                Interface::<A>::apply_action(action, &ctx(resolved)),
                Err(StorageError::InvalidSignature)
            ),
            "{why}"
        );
    }
}

#[test]
fn a_direct_shared_write_verifies_exactly_as_before() {
    type A = MockedStorage<9502>;
    let (_, _, alice, update) = shared_cell::<A>();
    assert!(Interface::<A>::apply_action(update.clone(), &ctx(None)).is_err());
    Interface::<A>::apply_action(update, &apply_ctx_for(alice)).expect("alice's own write");
}

// ---------------------------------------------------------------------------
// SharedMember
// ---------------------------------------------------------------------------

/// The founder's `Frozen` value — a member of a cell the founder writes — and a
/// redelivery of its stored bytes, which every node accepts from the founder.
fn charter_redelivery() -> (Id, AccountId, Action) {
    env::reset_for_testing();
    let founder_device = key(0xF0);
    let founder = account_of_key(&founder_device);
    env::set_account_id(*founder.as_bytes());
    let charter = Root::new(|| Frozen::new("be kind".to_owned()));
    let (anchor, value_id) = charter.ids();
    let stored = MainStorage::storage_read(Key::Entry(value_id)).expect("stored");
    let action = build_signed_member_action(
        false,
        value_id,
        anchor,
        stored,
        env::time_now() + 1_000_000_000,
        &founder_device,
        vec![ChildInfo::new(anchor, [0; 32], Metadata::default())],
    );
    (value_id, founder, action)
}

#[test]
#[serial]
fn a_relayed_member_write_is_judged_against_its_on_behalf_writer() {
    let (_, founder, redelivery) = charter_redelivery();
    let relay = key(0xB7);
    let stranger = account_of_key(&key(0xC3));
    let shipped = relayed(redelivery.clone(), &relay, Some(founder));

    for (action, resolved, why) in [
        (shipped.clone(), None, "unresolved"),
        (
            shipped.clone(),
            Some(account_of_key(&relay)),
            "the relay as itself",
        ),
        (
            retargeted(shipped.clone(), Some(stranger)),
            Some(stranger),
            "on_behalf swapped after signing",
        ),
        (
            relayed(redelivery.clone(), &relay, Some(stranger)),
            Some(founder),
            "written for a non-writer, resolution names the writer",
        ),
    ] {
        assert!(
            matches!(
                MainInterface::apply_action(action, &ctx(resolved)),
                Err(StorageError::InvalidSignature)
            ),
            "{why}"
        );
    }
    MainInterface::apply_action(shipped, &apply_ctx_for(founder))
        .expect("relayed redelivery for the writer applies");
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// A snapshot leaf proves only that its signature verifies under the key it
/// names; for an on-behalf leaf that is the relay's key over a payload that
/// covers `on_behalf`. Authorship is the node's check (`snapshot_leaf_authorship`).
#[test]
#[serial]
fn the_snapshot_verifiers_cover_on_behalf_leaves() {
    // User.
    env::reset_for_testing();
    let (author_device, owner) = create_test_owner();
    let relay = key(0xB7);
    let user = relayed(owned_add(&author_device, owner), &relay, Some(owner));
    let leaf = |action: &Action| match action {
        Action::Add { id, data, .. } | Action::Update { id, data, .. } => (*id, data.clone()),
        Action::DeleteRef { .. } => unreachable!(),
    };
    let (id, data) = leaf(&user);
    MainInterface::verify_snapshot_entity_signature(id, None, &data, metadata_of(&user))
        .expect("an on-behalf user leaf verifies under the relay key");
    let tampered = retargeted(user, None);
    assert!(MainInterface::verify_snapshot_entity_signature(
        id,
        None,
        &data,
        metadata_of(&tampered)
    )
    .is_err());

    // Shared.
    type A = MockedStorage<9503>;
    let (_, _, alice, update) = shared_cell::<A>();
    let shared = relayed(update, &relay, Some(alice));
    let (id, data) = leaf(&shared);
    Interface::<A>::verify_snapshot_entity_signature(id, None, &data, metadata_of(&shared))
        .expect("an on-behalf shared leaf verifies under the relay key");
    let tampered = retargeted(shared, Some(account_of_key(&relay)));
    assert!(Interface::<A>::verify_snapshot_entity_signature(
        id,
        None,
        &data,
        metadata_of(&tampered)
    )
    .is_err());

    // SharedMember.
    let (value_id, founder, redelivery) = charter_redelivery();
    let member = relayed(redelivery, &relay, Some(founder));
    let (_, data) = leaf(&member);
    MainInterface::verify_snapshot_member_signature(value_id, &data, metadata_of(&member))
        .expect("an on-behalf member leaf verifies under the relay key");
    let tampered = retargeted(member, None);
    assert!(MainInterface::verify_snapshot_member_signature(
        value_id,
        &data,
        metadata_of(&tampered)
    )
    .is_err());
}

/// A keyed owned entry, whose key the snapshot verifier also checks against
/// its id, verifies with an on-behalf signature too.
#[test]
#[serial]
fn a_relayed_keyed_owned_entry_verifies_on_snapshot() {
    env::reset_for_testing();
    let alice_device = key(0xA1);
    let alice = account_of_key(&alice_device);
    env::set_account_id(*alice.as_bytes());
    let mut notes = Root::new(Notes::new);
    notes
        .insert("n".to_owned(), LwwRegister::new("hi".to_owned()))
        .expect("insert");
    let id = notes.entry_id(&"n".to_owned());
    let parent = (**notes).id();
    let data = MainStorage::storage_read(Key::Entry(id)).expect("stored");
    let metadata = <Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .expect("present");
    let shipped = relayed(
        Action::Update {
            id,
            data: data.clone(),
            ancestors: vec![],
            metadata,
        },
        &key(0xB7),
        Some(alice),
    );
    MainInterface::verify_snapshot_entity_signature(id, Some(parent), &data, metadata_of(&shipped))
        .expect("verifies");
}

// ---------------------------------------------------------------------------
// Delta path: the node's per-action resolution
// ---------------------------------------------------------------------------

/// The rig's failure, at the storage layer. A relay's entry for an account
/// rode a delta whose author was not that account: authored by the relay, or
/// applied with no author armed (a cascaded child, a persisted parent loaded
/// into the DAG), so the delta-wide `signer_account` is the relay's account or
/// nothing. Both are refused, as they must be on their own: the delta's author
/// is not the account the entry names.
///
/// The node judges each on-behalf entry itself (a `RelayTee` may write any
/// member's entries) and hands the verdict over per action in
/// `CausalActions::on_behalf_accounts`. With it the entry applies, whoever
/// authored the delta.
#[test]
#[serial]
fn a_relay_entry_applies_by_the_nodes_per_action_resolution_whoever_authored_the_delta() {
    use std::collections::BTreeMap;

    use crate::delta::StorageDelta;
    use crate::tests::common::EmptyData;

    let (author_device, owner) = create_test_owner();
    let (relay, _) = create_test_keypair();
    let relay_account = account_of_key(&relay);

    for delta_author in [None, Some(relay_account)] {
        env::reset_for_testing();
        let shipped = relayed(owned_add(&author_device, owner), &relay, Some(owner));
        let id = shipped.id();
        let delta = |on_behalf_accounts| {
            borsh::to_vec(&StorageDelta::CausalActions {
                actions: vec![shipped.clone()],
                delta_id: [0xD1; 32],
                delta_hlc: env::hlc_timestamp(),
                effective_writers: BTreeMap::new(),
                signer_account: delta_author,
                on_behalf_accounts,
            })
            .expect("encode")
        };

        // Control: the delta-wide author alone, as before the fix.
        drop(Root::<EmptyData>::sync(
            &delta(BTreeMap::new()),
            &ApplyContext::empty(),
        ));
        assert!(
            MainInterface::find_by_id_raw(id).is_none(),
            "{delta_author:?}: the delta's author is not the account the entry names"
        );

        Root::<EmptyData>::sync(
            &delta(BTreeMap::from([(id, owner)])),
            &ApplyContext::empty(),
        )
        .expect("sync");
        assert!(
            MainInterface::find_by_id_raw(id).is_some(),
            "{delta_author:?}: the node found the relay entitled, so the entry is the owner's"
        );
    }
}

/// The per-action resolution is still the AUTHOR the entry is checked against:
/// naming an account other than the entry's `on_behalf` does not let it in.
#[test]
#[serial]
fn a_per_action_resolution_naming_another_account_is_refused() {
    use std::collections::BTreeMap;

    use crate::delta::StorageDelta;
    use crate::tests::common::EmptyData;

    env::reset_for_testing();
    let (author_device, owner) = create_test_owner();
    let (relay, _) = create_test_keypair();
    let shipped = relayed(owned_add(&author_device, owner), &relay, Some(owner));
    let id = shipped.id();
    let delta = borsh::to_vec(&StorageDelta::CausalActions {
        actions: vec![shipped],
        delta_id: [0xD1; 32],
        delta_hlc: env::hlc_timestamp(),
        effective_writers: BTreeMap::new(),
        signer_account: None,
        on_behalf_accounts: BTreeMap::from([(id, AccountId::from([0x51; 32]))]),
    })
    .expect("encode");
    drop(Root::<EmptyData>::sync(&delta, &ApplyContext::empty()));
    assert!(MainInterface::find_by_id_raw(id).is_none());
}
