//! Which device scopes reach a scope: its target application, and the founder's
//! own account namespace.

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_projection::ScopeState;
use calimero_storage::logical_clock::HybridTimestamp;

const SALT: [u8; 32] = [0x5A; 32];

fn app(byte: u8) -> ApplicationId {
    ApplicationId::from([byte; 32])
}

/// An account root and one device of it, linked under a scope naming `applications`.
struct Linked {
    account: AccountId,
    device: DeviceId,
    device_key: PublicKey,
    payload: OpPayload,
}

fn linked(root_seed: u8, applications: &[ApplicationId]) -> Linked {
    let root = PrivateKey::from([root_seed; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [root_seed; 16]);
    let device_key = PrivateKey::from([root_seed ^ 0x80; 32]).public_key();
    let cert = DeviceCert::sign(
        &root,
        account,
        device,
        &device_key,
        &KemPublicKey::from([root_seed; 32]),
        0,
        0,
    )
    .expect("the root certifies its device");
    Linked {
        account,
        device,
        device_key,
        payload: OpPayload::DeviceLinked {
            genesis,
            chain: vec![],
            cert,
            scope_epoch: 1,
            applications: applications.to_vec(),
        },
    }
}

fn op(scope: ScopeId, id_byte: u8, parents: Vec<[u8; 32]>, payload: OpPayload) -> Op {
    let signer = PrivateKey::from([id_byte; 32]).public_key();
    signed_by(signer, scope, id_byte, parents, payload)
}

fn signed_by(
    signer: PublicKey,
    scope: ScopeId,
    id_byte: u8,
    parents: Vec<[u8; 32]>,
    payload: OpPayload,
) -> Op {
    Op::from_parts(
        [id_byte; 32],
        scope,
        parents,
        Authorship::unattributed(signer),
        HybridTimestamp::default(),
        payload,
        [0u8; 32],
        [0u8; 64],
    )
}

/// `founder`'s account-namespace genesis under `salt`, carrying its device's link.
fn founded(founder: &Linked, salt: [u8; 32]) -> OpPayload {
    OpPayload::AccountNamespaceFounded {
        founder: founder.account,
        salt,
        carried: Box::new(founder.payload.clone()),
    }
}

fn target(scope: ScopeId, application: ApplicationId) -> OpPayload {
    OpPayload::TargetApplicationSet {
        group: ContextGroupId::from(*scope.as_bytes()),
        application,
        bytecode: [0xB1; 32],
    }
}

fn live(ops: &[Op]) -> Vec<DeviceId> {
    let mut state = ScopeState::default();
    for op in ops {
        state.apply(op);
    }
    state.acl_view().devices.keys().copied().collect()
}

#[test]
fn a_device_is_live_only_where_its_scope_covers_the_target() {
    let scope = ScopeId::from([7u8; 32]);
    let covered = linked(0x11, &[app(0xC1)]);
    let elsewhere = linked(0x12, &[app(0xE1)]);
    let everywhere = linked(0x13, &[]);
    let ops = [
        op(scope, 1, vec![], target(scope, app(0xC1))),
        op(scope, 2, vec![[1; 32]], covered.payload),
        op(scope, 3, vec![[1; 32]], elsewhere.payload),
        op(scope, 4, vec![[1; 32]], everywhere.payload),
    ];

    let devices = live(&ops);

    assert!(devices.contains(&covered.device));
    assert!(devices.contains(&everywhere.device));
    assert!(
        !devices.contains(&elsewhere.device),
        "a device scoped away from the target is not live"
    );
}

/// The founder's device, scoped away from any target, and the scope its genesis
/// under `salt` derives.
fn founder_and_scope() -> (Linked, ScopeId) {
    let founder = linked(0x21, &[app(0xE1)]);
    let scope = ScopeId::from(calimero_account::account_namespace_id(
        &founder.account,
        &SALT,
    ));
    (founder, scope)
}

#[test]
fn an_account_namespace_exempts_only_its_founders_devices() {
    let (founder, scope) = founder_and_scope();
    let other = linked(0x22, &[app(0xE1)]);
    let ops = [
        signed_by(
            founder.device_key,
            scope,
            1,
            vec![],
            founded(&founder, SALT),
        ),
        op(scope, 3, vec![[1; 32]], other.payload),
    ];

    let devices = live(&ops);

    assert!(
        devices.contains(&founder.device),
        "the founder's scoped device reaches its own account namespace"
    );
    assert!(
        !devices.contains(&other.device),
        "another account's scoped device is still filtered in an account namespace"
    );
}

#[test]
fn a_genesis_whose_salt_does_not_derive_the_scope_marks_nothing() {
    let (founder, scope) = founder_and_scope();
    let ops = [signed_by(
        founder.device_key,
        scope,
        1,
        vec![],
        founded(&founder, [0x0B; 32]),
    )];

    assert!(!live(&ops).contains(&founder.device));
}

#[test]
fn a_parented_genesis_marks_nothing() {
    let (founder, scope) = founder_and_scope();
    let ops = [signed_by(
        founder.device_key,
        scope,
        1,
        vec![[9; 32]],
        founded(&founder, SALT),
    )];

    assert!(!live(&ops).contains(&founder.device));
}

/// The genesis' salt and credential are public, so a replay signed by any other
/// key must not mark the scope.
#[test]
fn a_genesis_signed_by_another_key_marks_nothing() {
    let (founder, scope) = founder_and_scope();
    let ops = [op(scope, 1, vec![], founded(&founder, SALT))];

    assert!(!live(&ops).contains(&founder.device));
}

#[test]
fn concurrent_targets_settle_the_same_way_in_either_order() {
    let scope = ScopeId::from([8u8; 32]);
    let device = linked(0x51, &[app(0xC1)]);
    let restated = op(scope, 0x10, vec![[1; 32]], target(scope, app(0xC1)));
    let rival = op(scope, 0x20, vec![[1; 32]], target(scope, app(0xE1)));
    let base = [
        op(scope, 1, vec![], target(scope, app(0xC1))),
        op(scope, 2, vec![[1; 32]], device.payload),
    ];

    let mut forward = base.to_vec();
    forward.extend([restated.clone(), rival.clone()]);
    let mut backward = base.to_vec();
    backward.extend([rival, restated]);

    assert_eq!(live(&forward), live(&backward));
    assert!(
        !live(&forward).contains(&device.device),
        "the rival that ranks first decides the target"
    );
}

/// A genesis whose carried credential names the founder and the signer's key but
/// does not verify marks nothing, even folded raw without the apply's check.
#[test]
fn a_genesis_carrying_a_credential_that_does_not_verify_marks_nothing() {
    let (founder, scope) = founder_and_scope();
    let forger = PrivateKey::from([0x66; 32]).public_key();
    let OpPayload::DeviceLinked {
        genesis,
        chain,
        mut cert,
        ..
    } = founder.payload.clone()
    else {
        unreachable!("linked builds a DeviceLinked")
    };
    cert.sign_pk = forger;
    let forged = OpPayload::AccountNamespaceFounded {
        founder: founder.account,
        salt: SALT,
        carried: Box::new(OpPayload::DeviceLinked {
            genesis,
            chain,
            cert,
            scope_epoch: 0,
            applications: vec![],
        }),
    };
    let ops = [
        signed_by(forger, scope, 1, vec![], forged),
        op(scope, 2, vec![[1; 32]], founder.payload),
    ];

    assert!(!live(&ops).contains(&founder.device));
}

/// An account-namespace genesis for a namespace founded as an ordinary one marks
/// nothing, even signed by the founder's own device.
#[test]
fn an_ordinary_namespace_cannot_be_marked_after_the_fact() {
    let founder = linked(0x71, &[app(0xE1)]);
    let ordinary = ScopeId::from(calimero_account::founded_namespace_id(
        &founder.account,
        &SALT,
    ));
    let ops = [signed_by(
        founder.device_key,
        ordinary,
        1,
        vec![],
        founded(&founder, SALT),
    )];

    assert!(!live(&ops).contains(&founder.device));
}

fn root(ops: &[Op]) -> [u8; 32] {
    let mut state = ScopeState::default();
    for op in ops {
        state.apply(op);
    }
    state.root()
}

#[test]
fn the_target_is_part_of_the_convergence_root() {
    let scope = ScopeId::from([9u8; 32]);

    assert_ne!(
        root(&[op(scope, 1, vec![], target(scope, app(0xC1)))]),
        root(&[op(scope, 1, vec![], target(scope, app(0xC2)))]),
    );
}

#[test]
fn the_account_namespace_mark_is_part_of_the_convergence_root() {
    // The founder's device unscoped, so the mark changes no live device.
    let (_, scope) = founder_and_scope();
    let founder = linked(0x21, &[]);
    let marked = [signed_by(
        founder.device_key,
        scope,
        1,
        vec![],
        founded(&founder, SALT),
    )];
    let unmarked = [signed_by(
        founder.device_key,
        scope,
        1,
        vec![],
        founder.payload.clone(),
    )];

    assert_ne!(root(&marked), root(&unmarked));
}

#[test]
fn a_device_scope_is_part_of_the_convergence_root() {
    let scope = ScopeId::from([10u8; 32]);
    let narrow = linked(0x81, &[app(0xC1)]);
    let wide = linked(0x81, &[app(0xC1), app(0xC2)]);
    let with = |device: Linked| {
        root(&[
            op(scope, 1, vec![], target(scope, app(0xC1))),
            op(scope, 2, vec![[1; 32]], device.payload),
        ])
    };

    assert_ne!(with(narrow), with(wide));
}

fn cascade(scope: ScopeId, from: u8, to: u8, application: ApplicationId) -> OpPayload {
    OpPayload::CascadeUpgraded {
        group: ContextGroupId::from(*scope.as_bytes()),
        from_bytecode: [from; 32],
        bytecode: [to; 32],
        application,
    }
}

/// Two cascades from the code the root runs: the earlier stamp moves it, the later one
/// no longer matches, and every arrival order agrees.
#[test]
fn concurrent_cascades_settle_the_same_way_in_either_order() {
    let scope = ScopeId::from([11u8; 32]);
    let device = linked(0x92, &[app(0xC2)]);
    let base = [
        op(scope, 1, vec![], target(scope, app(0xC1))),
        op(scope, 2, vec![[1; 32]], device.payload),
    ];
    let first = op(
        scope,
        0x10,
        vec![[1; 32]],
        cascade(scope, 0xB1, 0xB2, app(0xC2)),
    );
    let second = op(
        scope,
        0x20,
        vec![[1; 32]],
        cascade(scope, 0xB1, 0xB3, app(0xC3)),
    );

    let mut forward = base.to_vec();
    forward.extend([first.clone(), second.clone()]);
    let mut backward = base.to_vec();
    backward.extend([second, first]);

    assert_eq!(root(&forward), root(&backward));
    assert!(
        live(&forward).contains(&device.device),
        "the cascade with the earlier stamp decides the target"
    );
}
