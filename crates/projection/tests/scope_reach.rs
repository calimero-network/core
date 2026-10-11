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
    let scope = ScopeId::from(calimero_account::founded_namespace_id(
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
