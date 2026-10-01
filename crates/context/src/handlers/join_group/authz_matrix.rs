//! Authorization matrix rows for the join handler: whose key a joiner keeps.

use calimero_governance_store::authz_matrix::{
    assert_covered, assert_matches, observe, Actor, ActorState, GatedOp, Home, OpTable, Outcome,
    Row, World,
};
use calimero_governance_store::test_fixtures::signed_invitation_for;
use calimero_primitives::identity::PrivateKey;

use super::{install_join_key, settle_join_key, GroupKeyring, JoinKey};

const TABLES: &[OpTable] = &[OpTable {
    op: GatedOp::AcceptNamespaceJoinKey,
    // The inviter, an admitter it names and the namespace's anchors are all the
    // owner here: the invitation names only its inviter, as one minted by default does.
    allow: &[ActorState::Owner],
    gap: &[],
}];

const ROWS: &[Row] = &[(GatedOp::AcceptNamespaceJoinKey, accept_namespace_join_key)];

/// A newcomer joining the namespace on the owner's invitation is answered by the
/// actor, then settles the key once the response's ops have applied.
fn accept_namespace_join_key(world: &World, actor: &Actor) -> Outcome {
    let store = world.fork();
    let namespace = world.namespace;
    let keyring = GroupKeyring::new(&store, namespace);
    // The joiner holds no namespace key before its join.
    while let Some((key_id, _key)) = keyring.load_current_key().expect("read the keyring") {
        keyring.delete_key_by_id(&key_id).expect("drop a key");
    }

    let joiner = PrivateKey::from([0xE1; 32]);
    let offered = [0x99; 32];
    let envelope = GroupKeyring::wrap_for_member(
        &actor.sign_sk,
        &joiner.public_key(),
        &namespace.to_bytes(),
        &offered,
    )
    .expect("wrap the key");
    let invitation = signed_invitation_for(&world.owner_sk, namespace, [0xE2; 32]);

    let install = |state: Option<JoinKey>| match state {
        None => install_join_key(
            &store,
            namespace.to_bytes(),
            namespace,
            &joiner,
            &envelope,
            &invitation,
        ),
        Some(state) => settle_join_key(
            &store,
            namespace.to_bytes(),
            namespace,
            &joiner,
            &envelope,
            &invitation,
            state,
        ),
    };
    let installed = install(None).expect("the install runs");
    let settled = install(Some(installed)).expect("the settle runs");
    let held = keyring
        .load_current_key()
        .expect("read the keyring")
        .map(|(_id, key)| key);
    assert_eq!(
        settled == JoinKey::Held,
        held == Some(offered),
        "the settled state and the keyring disagree"
    );
    (held == Some(offered)).into()
}

#[test]
fn every_operation_homed_here_has_a_row_and_a_table() {
    let ops: Vec<GatedOp> = ROWS.iter().map(|(op, _)| *op).collect();
    assert_covered(Home::Context, &ops, TABLES);
}

#[test]
fn authorization_matrix() {
    assert_matches("context", TABLES, &observe(ROWS));
}
