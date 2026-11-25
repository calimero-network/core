//! The system group: fleet, app directory, identity and organizations.
//!
//! Commands are applied in log order on every node. Each command that acts for
//! someone carries that `actor`, and `apply` checks their authority against
//! the state the command applies to, so the log alone decides every outcome.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use calimero_bundle::UpgradeAuthority;
use calimero_identity::Credential;
use calimero_log::Machine;
use calimero_pipeline::PriceTable;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::error;

use crate::job_node::RUNS_PER_JOB_NODE;
use crate::types::{
    Access, Account, AccountId, ApiKeyRecord, AppId, AppRecord, Audience, Components, Grant,
    Invite, NodeRecord, NodeRole, Org, OrgId, OrgRole, RefreshRecord, Team, TeamId, TeamJoin,
    TeamRole,
};

mod accounts;
mod approvals;
mod apps;
pub mod cluster_version;
mod compute;
mod deployments;
mod fleet;
mod orgs;
mod packages;
mod recovery;
mod rejection;
mod routing;
mod sign_up;
mod updates;

pub use accounts::SessionsRevoked;
pub use approvals::{
    egress_args, ApprovedReleases, Deadlines, EgressSet, OutboundApproved, VersionDeadline,
};
pub use deployments::{
    AppOffered, AppWithdrawn, ClusterOwnerNamed, DeployerGranted, DeployerRevoked,
    DeploymentDeleted, DeploymentMade, DeploymentSecretCleared, DeploymentSecretSet, GovernanceSet,
};
pub use orgs::{
    InviteRedeemed, OrgCreated, OrgDeleted, OrgRenamed, OrgSettingsSet, TeamCreated, TeamDeleted,
};
pub use packages::{PublisherBound, Uninstalled};
pub use recovery::{RecoveryFinished, RecoveryStarted};

/// What `ScopeApps` answers: the organization's mode after, and each package's app team.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppTeamsScoped {
    pub mode: crate::types::OrgMode,
    pub teams: Vec<ScopedTeam>,
}

/// One package's app team in [`AppTeamsScoped`]: whether it was made, the instances moved onto
/// it, the rows added, how it is joined now and how many lost it by a close.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedTeam {
    pub package: String,
    pub team_id: TeamId,
    pub created: bool,
    pub apps: Vec<AppId>,
    pub added: usize,
    pub join: Option<TeamJoin>,
    pub dropped: usize,
}
pub use rejection::{Rejection, RejectionKind};
pub use sign_up::InviteKeyAnswer;
pub use updates::PolicyView;

/// Per-account limits, replicated so every node enforces the same ones at apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Quotas {
    pub max_apps_per_account: u32,
    pub max_bundle_bytes_per_account: u64,
    pub max_api_keys_per_account: u32,
    pub max_orgs_per_account: u32,
    /// Accounts the cluster takes in all: where sign-up is open
    /// ([`Governance::sign_up`]), this is what keeps it from growing without
    /// bound (Xil's review, issue 12).
    pub max_accounts: u32,
    /// Plaintext bytes of the files an organization's apps hold (a personal
    /// organization's too).
    pub max_file_bytes_per_org: u64,
    /// Compute runs an organization (a personal one included) may have granted to job
    /// nodes at once; the rest wait their turn.
    pub max_concurrent_runs_per_org: u32,
    /// The declared fuel of an organization's granted runs, summed.
    pub compute_fuel_per_org: u64,
    /// Fuel a developer's apps may burn per UTC day, summed over its component apps
    /// (Q20, D76, D78); 0 (the default) is no limit. The developer is the one usage
    /// is billed to (`usage::developer`, #4839): the owner of the deployment an app
    /// was created from, else the app's organization or owner. Over it, a client's
    /// call or an xcall delivery to any of the developer's apps is refused before it
    /// runs (`429 QUOTA_EXCEEDED`); `init` and timer ticks still run, and still
    /// count. Checked where the call runs, against the apps' usage books, not at
    /// apply: those live in the app groups. A decimal string in JSON (D20); a number
    /// is refused. Written only once every node runs a release that knows it
    /// (`UPGRADE_PENDING`, `Node::nodes_behind_release`).
    #[serde(with = "decimal_u64")]
    pub max_fuel_per_developer_per_day: u64,
}

/// A `u64` as a decimal string in JSON (D20): digits only, no sign, no leading zero.
/// A JSON number is refused, so a client never sends a cap a JavaScript number has
/// already rounded.
mod decimal_u64 {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = String::deserialize(d)?;
        let digits = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits || (s.len() > 1 && s.starts_with('0')) {
            return Err(D::Error::custom(format!(
                "{s:?} is not a decimal string (D20)"
            )));
        }
        s.parse().map_err(D::Error::custom)
    }
}

mod decimal_u64_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Deserialize)]
    struct Decimal(#[serde(with = "super::decimal_u64")] u64);

    pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        v.map(|n| n.to_string()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        Ok(Option::<Decimal>::deserialize(d)?.map(|Decimal(n)| n))
    }
}

/// One payer's compute and file limits in place of the cluster's; a field left out keeps the
/// cluster's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OrgQuotas {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_runs: Option<u32>,
    #[serde(with = "decimal_u64_option", skip_serializing_if = "Option::is_none")]
    pub compute_fuel: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
}

/// The runs, fuel and file bytes `payer` may hold: its override, else the cluster's.
fn limits_of(
    quotas: &Quotas,
    overrides: &BTreeMap<String, OrgQuotas>,
    payer: &str,
) -> (u32, u64, u64) {
    let o = overrides.get(payer);
    (
        o.and_then(|o| o.max_concurrent_runs)
            .unwrap_or(quotas.max_concurrent_runs_per_org),
        o.and_then(|o| o.compute_fuel)
            .unwrap_or(quotas.compute_fuel_per_org),
        o.and_then(|o| o.max_file_bytes)
            .unwrap_or(quotas.max_file_bytes_per_org),
    )
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            max_apps_per_account: 100,
            max_bundle_bytes_per_account: 1024 * 1024 * 1024,
            max_api_keys_per_account: 50,
            max_orgs_per_account: 20,
            max_accounts: 100_000,
            max_file_bytes_per_org: 50_000_000_000,
            max_concurrent_runs_per_org: 4,
            compute_fuel_per_org: 10 * calimero_app_host::MAX_COMPUTE_FUEL,
            max_fuel_per_developer_per_day: 0,
        }
    }
}

/// The codes `AcquireRun` answers with when it refuses; a leader matches these, never text.
pub const RUN_NO_SUCH_APP: &str = "NO_SUCH_APP";
pub const RUN_NOT_PAYER: &str = "NOT_PAYER";
pub const RUN_FUEL_OVER_MAX: &str = "FUEL_OVER_RUN_MAX";
pub const RUN_FUEL_OVER_QUOTA: &str = "FUEL_OVER_QUOTA";
pub const RUN_RELEASED: &str = "RELEASED";
pub const RUN_APP_QUEUE_FULL: &str = "APP_QUEUE_FULL";
pub const RUN_NO_POOL: &str = "RUN_NO_POOL";
/// The apply error that refuses removing a pool that still has job nodes or runs.
pub const POOL_IN_USE: &str = "POOL_IN_USE: the pool still has job nodes or runs";
/// The pool a job node with no pool, and a run on a cluster with no pools, belongs to.
pub const DEFAULT_POOL: &str = "standard";
pub const MAX_ORG_QUOTAS: usize = 10_000; // payers with their own quota override
pub const MAX_PAYER_BYTES: usize = 128; // a payer id an override names
pub const MAX_POOLS: usize = 32; // pools one cluster defines
pub const MAX_POOL_NAME_BYTES: usize = 64; // a pool's name, also on a job node's record
/// The apply error for a pool set past `MAX_POOLS` or with an empty or too long name.
pub const BAD_POOLS: &str = "BAD_POOLS: at most 32 pools, each named with 1 to 64 bytes";
/// The apply error for an update policy whose hold is outside
/// `MIN_HOLD_HOURS..=MAX_HOLD_HOURS` ([`crate::types::UpgradePolicy`]).
pub const BAD_UPGRADE_POLICY: Rejection = Rejection::new(
    RejectionKind::BadUpgradePolicy,
    "BAD_UPGRADE_POLICY: holdHours is 24 (a day) to 168 (a week)",
);
/// The apply error for a red or unclassified update whose policy asks for a second
/// approver, without another owner's or admin's live first approval of it
/// ([`SystemState::update_gate`]).
pub const SECOND_APPROVAL_REQUIRED: Rejection =
    Rejection::new(RejectionKind::SecondApprovalRequired, "SECOND_APPROVAL_REQUIRED: this update is red and the app's policy asks for a second approver: another owner or admin approves it first (POST /apps/{app}/upgrade-approvals)");
/// The apply error for an update's class that leaves out a reason the log shows, or is
/// not one the classifier answers (red without a reason, green with one, too many).
pub const UPGRADE_CLASS_MISMATCH: Rejection = Rejection::new(
    RejectionKind::UpgradeClassMismatch,
    "UPGRADE_CLASS_MISMATCH: the update's class does not match the releases the log holds",
);
/// The apply error for a first approval where one approval applies the update: no second
/// approver in the app's policy, or a green update.
pub const ONE_APPROVAL: Rejection = Rejection::new(
    RejectionKind::OneApproval,
    "ONE_APPROVAL: this update needs one approval: upgrade the app (POST /apps/{app}/upgrade)",
);
/// The apply error for a first approval of an update another owner or admin approved
/// first: that one waits for its second, which is an upgrade.
pub const APPROVAL_PENDING: Rejection =
    Rejection::new(RejectionKind::ApprovalPending, "APPROVAL_PENDING: another owner or admin approved this update already: apply it (POST /apps/{app}/upgrade)");
/// Who an update that applied on its own names as its actor (`UpgradeApp::auto`): its
/// app's update policy, never an account (no account id has this shape).
pub const AUTO_UPDATE: &str = "policy:auto-update";
/// The apply error for an update proposed to apply on its own that the app's policy does
/// not let through: no `noDestinations` policy, a class other than green, a unit with a
/// destination before or after, an older release, a unit's member alone, or another
/// release than its deployment offers.
pub const AUTO_UPDATE_REFUSED: Rejection =
    Rejection::new(RejectionKind::AutoUpdateRefused, "AUTO_UPDATE_REFUSED: the app's update policy does not apply this update on its own; an owner or admin approves it");
/// The apply error for an update proposed to apply on its own before its hold passed,
/// judged on the release's logged install or offer time and the upgrade's own `at`.
pub const HOLD_NOT_OVER: Rejection = Rejection::new(
    RejectionKind::HoldNotOver,
    "HOLD_NOT_OVER: the update's hold has not passed since the release was logged",
);
/// The apply error for an update proposed to apply on its own without a restore point.
pub const RESTORE_POINT_REQUIRED: Rejection = Rejection::new(
    RejectionKind::RestorePointRequired,
    "RESTORE_POINT_REQUIRED: an update applies on its own only after a restore point",
);
/// The apply error for an undo of an app whose last upgrade left no restore point.
pub const NO_RESTORE_POINT: Rejection = Rejection::new(
    RejectionKind::NoRestorePoint,
    "NO_RESTORE_POINT: this app has no upgrade to undo",
);
/// The apply error for an undo logged more than `UNDO_WINDOW_SECS` after its upgrade.
pub const UNDO_WINDOW_PASSED: Rejection = Rejection::new(
    RejectionKind::UndoWindowPassed,
    "UNDO_WINDOW_PASSED: an upgrade is undone within 7 days of it, and this one is older",
);
/// The apply error for an undo of an app that no longer runs what its upgrade moved it
/// to, or is being restored from another backup.
pub const UNDO_STALE: Rejection = Rejection::new(
    RejectionKind::UndoStale,
    "UNDO_STALE: the app no longer runs the release its last upgrade moved it to",
);
/// The apply error for an undo whose apps or new groups are not those its instance's
/// restore points name (the instance and the members its unit's upgrade moved).
pub const UNDO_NOT_AS_NAMED: Rejection =
    Rejection::new(RejectionKind::UndoNotAsNamed, "UNDO_NOT_AS_NAMED: an undo reverts the instance and the members its upgrade moved, each seeded by the node that holds its restore point");
/// The apply error for an undo to a release that is no longer installed (a bundle another
/// release ships may remain, but the release itself and its frontend are gone).
pub const UNDO_RELEASE_GONE: Rejection = Rejection::new(
    RejectionKind::UndoReleaseGone,
    "UNDO_RELEASE_GONE: the release the app ran before is no longer installed",
);
/// The apply error for a price table whose version is not the current one plus one.
pub const PRICE_VERSION: &str = "PRICE_VERSION: the version must be the current one plus one";
/// The apply error for a price table with a zero fuel rate.
pub const BAD_PRICES: &str = "BAD_PRICES: the fuel rate per vCPU-second must be above zero";
/// Released run ids kept per app, so a late `AcquireRun` for one is refused.
pub const RELEASED_RUNS_KEPT: usize = 256;

type WaitingRun = (u64, String, String, u64, String); // seq, app, run, fuel, pool

/// A pool of job nodes: how many runs each takes and the largest run memory it admits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolConfig {
    pub runs_per_node: u32,
    pub max_memory_mib: u32,
}

/// A compute run waiting for, or holding, a place on a job node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSlot {
    pub payer: String,
    /// The fuel the run declares, reserved while it is granted.
    pub fuel: u64,
    /// Arrival order across the cluster; the longest-waiting organization is served first.
    pub seq: u64,
    pub granted: bool,
    /// The payer's fuel quota fell below the run's fuel: it will never be granted, and the
    /// leader ends it.
    #[serde(default)]
    pub refused: bool,
    /// The pool the run was admitted to; empty on a cluster with no pools.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pool: String,
}

/// How a quota refusal's message starts ([`Rejection::quota`]). Only the
/// wording: its status comes from its kind ([`RejectionKind::QuotaExceeded`]).
pub const QUOTA_PREFIX: &str = "quota exceeded";

/// The refusal when an app would run a compute component.
const COMPUTE_NOT_AN_APP: &str = "a compute component is run by job nodes, never as an app";

/// The refusal when an app names a compute component its bundle's release does not ship.
/// The refusal of an uninstall of a release another release's pipelines pin as a module.
pub const MODULE_IN_USE: Rejection = Rejection::new(
    RejectionKind::ModuleInUse,
    "an installed release's pipelines use that release as a module; uninstall them first",
);
const COMPUTE_NOT_SHIPPED: &str = "that compute component is not shipped with the app's bundle";

/// The refusal when an app names a release that does not ship its bundle.
const RELEASE_NOT_SHIPPED: &str = "that release does not ship the app's bundle";

/// The refusal when a release ships a WASM a release of another package,
/// signed by another key, ships.
pub const SHIPS_IN_OTHER_PACKAGE: &str =
    "a WASM of this release ships in another signer's package; a WASM belongs to one signer";

/// The refusal when an upgrade would add or drop an app's app component.
const COMPONENTS_CHANGED: &str = "an app keeps the components it was made with";

/// The refusal when an app's components differ from its release's.
const COMPONENTS_NOT_RELEASES: &str = "the app's components are not its release's";

/// The actor the log records for what the operator does with its ops token
/// (installs, upgrades): never an account id, so it names no account.
pub const OPERATOR: &str = "operator";

/// `SetBackupKey` naming a key that is not 32 bytes of lowercase hex.
pub const BAD_BACKUP_KEY: &str = "a backup key is 64 lowercase hex characters, not all zeros";

/// `SetBackupKey` by someone who does not upgrade the holder's apps.
pub const NOT_BACKUP_HOLDER: &str =
    "only whoever upgrades these apps sets the key their backups open with";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleRecord {
    pub hash: String,
    /// The WASM itself lives in each node's blob store, under `hash`.
    pub size: u64,
    pub uploaded_by: AccountId,
    /// The package release this WASM came out of; `None` for a raw upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageRef>,
}

/// One service of one package release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRef {
    pub package: String,
    pub version: String,
    /// The service's name; `None` for a single-service bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// The bundle is the service's compute component, not the service itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compute: bool,
}

/// Where a release's `.mpk` came from, so a node that needs it knows where to get it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BundleSource {
    /// The operator's registry (`--registry-url`): every node fetches it from there.
    Registry,
    /// Uploaded as a file: held by a majority of nodes, fetched from a peer.
    Upload,
}

/// Who may use a release an account installed ([`SystemState::sees_release`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Visibility {
    /// The registry serves it to anyone: everyone here may use it.
    Public,
    /// A private registry package, or an upload: members of the organizations it was
    /// installed for ([`PackageVersion::orgs`]).
    Private,
}

/// A package: one publisher key, any number of releases.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRecord {
    pub name: String,
    /// The publisher's key (`did:key`), pinned by the first release. Every
    /// later release must be signed by it (the `(package, signer)` identity).
    pub signer_id: String,
    /// Who installed the first release; may uninstall releases.
    pub owner: AccountId,
    pub versions: BTreeMap<String, PackageVersion>,
}

/// A pipeline a release registers: the SHA-256 of its canonical JSON, a content-addressed
/// part of the release, and the most fuel one run of it can burn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRef {
    pub hash: String,
    /// Decimal `u128`: JSON numbers do not hold one.
    pub worst_case_fuel: String,
    /// The other packages' modules the pipeline names, by the reference as written (`self`
    /// excluded), each pinned to the release it was installed against.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, ModulePin>,
}

/// The release of another package a pipeline's module reference resolved to at install.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModulePin {
    pub package: String,
    pub version: String,
    /// The release's compute component, as a bundle hash.
    pub compute: String,
}

/// One app a release references in its signed manifest (`BundleManifest::uses`), as the
/// system state keeps it: what judges the secrets a deployment offering the release may hold
/// for the apps it uses ([`SystemState::used_releases`]). Unknown fields are read past, so a
/// later release may add to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseUse {
    /// The referenced package.
    pub package: String,
    /// The semver requirement its releases meet (`^0.1`).
    pub range: String,
    /// The release the app was built against (`calimero.lock`), inside `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl From<&calimero_bundle::BundleUse> for ReleaseUse {
    fn from(u: &calimero_bundle::BundleUse) -> Self {
        Self {
            package: u.package.clone(),
            range: u.range.clone(),
            version: u.version.clone(),
        }
    }
}

/// One release of a package, as its signed manifest describes it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageVersion {
    pub version: String,
    /// `hex(SHA-256)` of the `.mpk`: what a node that fetches it checks first.
    pub mpk: String,
    pub mpk_size: u64,
    pub source: BundleSource,
    /// Service name (`""` for a single-service bundle) -> the WASM's bundle hash.
    pub services: BTreeMap<String, String>,
    /// Which components the release has. `ComputeOnly`: `services` is empty and `compute`
    /// holds the single-service bundle's component under `""`.
    #[serde(default, skip_serializing_if = "Components::is_app")]
    pub components: Components,
    /// Service name -> its compute component's bundle hash, for the services that ship one.
    /// Never a service: no app runs it, and only job nodes do.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub compute: BTreeMap<String, String>,
    /// Each part's WASM size by bundle hash, which is what it counts against the bundle quota.
    /// Absent in releases from before: every part then counts the whole `.mpk`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub part_sizes: BTreeMap<String, u64>,
    /// Pipeline name -> its registered form. Left out when empty, so a record of a release
    /// without pipelines reads and writes as it did; an older node ignores the field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pipelines: BTreeMap<String, PipelineRef>,
    /// The manifest's display metadata (name, description, icon, ...).
    #[serde(default)]
    pub metadata: Value,
    /// URL prefixes the app may call out to (the manifest's signed `outbound`).
    #[serde(default)]
    pub outbound: Vec<String>,
    /// The credentials the release binds (`BundleManifest::secrets`): the node attaches
    /// each to its host's requests, in its header or body field, and no other. Empty for a
    /// release that binds none, whose requests carry no secret. Its shape changed in
    /// 1.0.0-rc.36 (one host and one place, from a list of hosts), a hard cutover: an
    /// `InstallPackage` an earlier release recorded is not read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, calimero_bundle::BundleSecret>,
    /// The apps the release references (`BundleManifest::uses`), by reference name: a
    /// deployment offering it may hold the secrets their releases declare
    /// ([`SystemState::deployment_uses_declare`]). Recorded only once every node applies it
    /// (`Feature::DeploymentUsedSecrets`); empty in a release recorded before, which an
    /// install of the same release again fills in. Left out when empty, so such a record
    /// reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub uses: BTreeMap<String, ReleaseUse>,
    pub installed_by: AccountId,
    pub installed_at: u64,
    /// Who installed this same release again after `installed_by` (an
    /// account, or [`OPERATOR`]): it is theirs to see and use too
    /// ([`SystemState::sees_release`]). Absent in records from before, and
    /// left out when empty, so such a record reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub installers: BTreeSet<AccountId>,
    /// Who upgrades the instances running this release, from its signed
    /// manifest ([`UpgradeAuthority`]); left out when `owner`.
    #[serde(default, skip_serializing_if = "UpgradeAuthority::is_owner")]
    pub upgrades: UpgradeAuthority,
    /// The oldest runtime its signed manifest says it runs on
    /// (`minRuntimeVersion`), kept so placement and the reports judge it
    /// without the `.mpk` ([`crate::runtime`]). Absent in releases recorded
    /// before, and dropped by a node on an earlier release: then it is read
    /// from the `.mpk` where it decides something (`Node::min_runtime_of`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_runtime_version: Option<String>,
    /// Which organization an instance of it is created in, from its signed manifest
    /// (`instanceOrg`, [`calimero_bundle::InstanceOrg`]). Recorded only once every node
    /// keeps it (`Feature::OrdinaryOrgs`), and filled in by an install of the same release
    /// after that; absent in a release recorded before, whose mode the API then reads from
    /// its `.mpk` (`api::apps::instance_org_of`). Left out when absent, so such a record
    /// reads and writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_org: Option<calimero_bundle::InstanceOrg>,
    /// Whether an instance of it takes guests, from its signed manifest (`guests`,
    /// [`calimero_bundle::GuestPolicy`]). Recorded only once every node keeps it
    /// (`Feature::Guests`), filled in by an install of the same release after that; absent
    /// in a release recorded before, whose switch the API reads from its `.mpk`
    /// (`Node::release_guests`) and apply takes as `forbidden`. Left out when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guests: Option<calimero_bundle::GuestPolicy>,
    /// Public or private, as the registry served it (an upload is private). Recorded only
    /// once every node keeps it (`Feature::PrivateReleases`); absent in a release recorded
    /// before, which keeps the rule from before ([`SystemState::sees_release`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    /// The organizations it was installed for, one more with each install for another
    /// (`Feature::PrivateReleases`). Left out when empty.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub orgs: BTreeSet<OrgId>,
}

impl PackageVersion {
    /// Service name -> the bundle an app of this release runs: the app component, or the
    /// compute component when that is all the release has.
    #[must_use]
    pub fn app_parts(&self) -> &BTreeMap<String, String> {
        match self.components {
            Components::App => &self.services,
            Components::ComputeOnly => &self.compute,
        }
    }

    /// Every WASM the release records under `package`, with the part of the release it is:
    /// each service, then each service's compute component.
    fn parts<'a>(&'a self, package: &'a str) -> impl Iterator<Item = (&'a str, PackageRef)> {
        let at = move |service: &str, compute: bool| PackageRef {
            package: package.to_owned(),
            version: self.version.clone(),
            service: (!service.is_empty()).then(|| service.to_owned()),
            compute,
        };
        let services = self
            .services
            .iter()
            .map(move |(s, h)| (h.as_str(), at(s, false)));
        let computes = self
            .compute
            .iter()
            .map(move |(s, h)| (h.as_str(), at(s, true)));
        services.chain(computes)
    }

    /// What the part `hash` counts against the bundle quota.
    fn part_size(&self, hash: &str) -> u64 {
        self.part_sizes.get(hash).copied().unwrap_or(self.mpk_size)
    }

    /// Whether `who` installed this release, first or again.
    #[must_use]
    pub fn installed_by_any(&self, who: &str) -> bool {
        self.installed_by == who || self.installers.contains(who)
    }

    /// Its kind as a reader is told: a release the operator installed (the catalog) is
    /// everyone's, so public; else as recorded (`None`: recorded before it was kept).
    #[must_use]
    pub fn visibility_shown(&self) -> Option<Visibility> {
        if self.installed_by_any(OPERATOR) {
            return Some(Visibility::Public);
        }
        self.visibility
    }

    /// The accounts that installed it (the operator left out).
    fn installing_accounts(&self) -> impl Iterator<Item = &AccountId> {
        std::iter::once(&self.installed_by)
            .chain(&self.installers)
            .filter(|a| *a != OPERATOR)
    }
}

impl PackageRecord {
    /// Whether `v` keeps the package's upgrade declarations in order: in
    /// version order, `publisher` releases first, then `owner` ones, so a
    /// release never takes upgrades back from owners.
    #[must_use]
    pub fn upgrades_in_order(&self, version: &str, upgrades: UpgradeAuthority) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        self.versions
            .values()
            .all(|u| match compare_versions(&u.version, version) {
                Less => !(u.upgrades.is_owner() && !upgrades.is_owner()),
                Greater => !(!u.upgrades.is_owner() && upgrades.is_owner()),
                Equal => true,
            })
    }

    /// The highest release by semver (a version that does not parse sorts first).
    #[must_use]
    pub fn latest(&self) -> Option<&PackageVersion> {
        self.latest_where(|_| true)
    }

    /// The highest release by semver among those `keep` holds of.
    pub fn latest_where(&self, keep: impl Fn(&PackageVersion) -> bool) -> Option<&PackageVersion> {
        self.versions
            .values()
            .filter(|v| keep(v))
            .max_by(|a, b| compare_versions(&a.version, &b.version))
    }
}

/// The rows a `MoveApp` gives, by the flags its entry carries (an entry an
/// earlier release logged leaves them out, and replays without those rows).
#[derive(Clone, Copy, Debug)]
struct MoveRows {
    /// A new app team gives its members who get every app added later a row.
    all_apps_rows: bool,
    /// An instance only named people reach gives them a row on its closed app team.
    accounts_in_app_team: bool,
    /// The target's quotas are its override where it has one.
    org_quotas: bool,
}

/// Semver order, falling back to the strings for a version that does not parse.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
        (Err(_), Ok(_)) => std::cmp::Ordering::Less,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// Make `s`, a system state restored from a backup, that of the cluster
/// founded again on `founders` (`node::restore`, `--restore-from`): the
/// founders are the replica nodes now and every other replica node of the
/// cluster backed up is removed (job nodes, which hold nothing, stay), each
/// app keeps the founders among its replicas and is filled up to `replicas`
/// from them, and every app waits for its own backup
/// ([`crate::types::PendingRestore::AWAITING_BACKUP`]): its group is hosted
/// nowhere until the operator restores it. Accounts, organizations, teams,
/// packages, governance and the operator key are kept as they were.
pub fn refound(s: &mut SystemState, founders: &[NodeRecord], replicas: usize) {
    let new: BTreeSet<u64> = founders.iter().map(|n| n.id).collect();
    let gone: Vec<u64> = s
        .replica_nodes()
        .filter(|n| !new.contains(&n.id))
        .map(|n| n.id)
        .collect();
    for id in gone {
        let _gone = s.nodes.remove(&id);
        let _new = s.removed_nodes.insert(id);
        *s.node_epoch.entry(id).or_default() += 1;
    }
    for node in founders {
        let _was = s.removed_nodes.remove(&node.id);
        *s.node_epoch.entry(node.id).or_default() += 1;
        let _prev = s.nodes.insert(node.id, node.clone());
    }
    let fleet: Vec<u64> = s.replica_nodes().map(|n| n.id).collect();
    let want = replicas.min(fleet.len()).max(1);
    for app in s.apps.values_mut() {
        app.replicas.retain(|id| fleet.contains(id));
        for id in &fleet {
            if app.replicas.len() >= want {
                break;
            }
            if !app.replicas.contains(id) {
                app.replicas.push(*id);
            }
        }
        app.restore = Some(crate::types::PendingRestore::AWAITING_BACKUP);
    }
}

/// Everything the system group replicates.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemState {
    /// Per-payer quota overrides; left out when empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub org_quotas: BTreeMap<String, OrgQuotas>,
    /// Job node pools by name; left out when empty (one legacy pool).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pools: BTreeMap<String, PoolConfig>,
    /// The operator's prices; left out until set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_table: Option<PriceTable>,
    pub nodes: BTreeMap<u64, NodeRecord>,
    /// Nodes the operator removed (`RemoveNode`): registered again only by
    /// the operator (`/ops/nodes`).
    #[serde(default)]
    pub removed_nodes: std::collections::BTreeSet<u64>,
    /// Moves with every registration and removal of a node id, so what a
    /// node kept about an id (an attestation) is dropped when the id changes.
    #[serde(default)]
    pub node_epoch: BTreeMap<u64, u64>,
    pub apps: BTreeMap<AppId, AppRecord>,
    pub next_group: u64,
    pub bundles: BTreeMap<String, BundleRecord>,
    pub accounts: BTreeMap<AccountId, Account>,
    /// What each account's passkeys did, oldest first: each sign-in and each
    /// change one approved, the last [`crate::types::ACTIVITY_KEPT`]. Shown
    /// to the account holder alone (`GET /auth/activity`). Written at apply
    /// from entries that carry their time; an earlier release's entries carry
    /// none and add nothing. Kept beside the accounts, not in them, so the
    /// account a sign-up's passkey signs (`account_args`) is unchanged; left
    /// out when empty, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub activity: BTreeMap<AccountId, VecDeque<crate::types::ActivityEntry>>,
    /// credential id -> account
    pub credentials: BTreeMap<String, AccountId>,
    /// API key hash -> record
    pub api_keys: BTreeMap<String, ApiKeyRecord>,
    /// Recovery codes: the code's public key ([`crate::proof::recovery_key`],
    /// hex) -> the account it recovers. At most one per account; the code
    /// itself is never logged. Left out when empty, so state from before
    /// reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recovery_keys: BTreeMap<String, AccountId>,
    /// What each person lets an app do as them (`SetDelegation`): account -> source app ->
    /// target app -> the target's methods the source's runs for that person call as them.
    /// Left out when empty, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub delegations: BTreeMap<AccountId, BTreeMap<AppId, BTreeMap<AppId, Vec<String>>>>,
    /// Account recoveries under way, by account: the passkey the start
    /// logged and when it may finish (`docs/specs/account-recovery.md`). At
    /// most one per account. Left out when empty, as `recovery_keys`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub account_recoveries: BTreeMap<AccountId, crate::types::PendingAccountRecovery>,
    /// refresh token hash -> record
    pub refresh: BTreeMap<String, RefreshRecord>,
    pub revoked_sessions: BTreeSet<String>,
    pub orgs: BTreeMap<OrgId, Org>,
    /// invite key (the token's Ed25519 public key, hex) -> invite
    pub invites: BTreeMap<String, Invite>,
    /// Instance invites ([`crate::types::AppInvite`]), by the token's Ed25519 public key:
    /// whoever redeems one becomes a guest of its instance. Left out when empty, so a
    /// state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_invites: BTreeMap<String, crate::types::AppInvite>,
    #[serde(default)]
    pub quotas: Quotas,
    /// Installed packages by name.
    #[serde(default)]
    pub packages: BTreeMap<String, PackageRecord>,
    /// The rules the cluster was deployed with, written once at genesis
    /// (`SystemCmd::Genesis`) and attested with every receipt key. `None` is
    /// state from before governance, which reads as [`Governance::default`].
    #[serde(default)]
    pub governance: Option<Governance>,
    /// When the operator holds release approvals ([`Governance::releases`]):
    /// the mero-tee releases it approved to hold every app's data.
    #[serde(default)]
    pub operator_releases: BTreeSet<String>,
    /// When the operator holds outbound approvals ([`Governance::outbound`]):
    /// what it approved, by package signer, for every app.
    #[serde(default)]
    pub operator_outbound: BTreeMap<String, crate::types::OutboundApproval>,
    /// The operator's end-of-new-work deadlines (`SetReleaseDeadline`): mero-tee
    /// release -> Unix seconds. From a release's deadline on, an organization
    /// or account whose newest approved release is older creates, installs
    /// and upgrades nothing until it approves that release or a newer one
    /// ([`SystemState::stalled`]); its apps keep running, and no data moves.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub release_deadlines: BTreeMap<String, u64>,
    /// Publishers (`BindPublisher`): a package signing key (`did:key`) -> the
    /// organization it is bound to, and how many bindings it has had. Where
    /// owners hold release approvals, that organization's approved releases govern every instance
    /// of every package the key signs, whoever owns the instance
    /// ([`SystemState::approved_for_release`]); where a package declares
    /// `upgrades: publisher`, its `MANAGE_APPLICATION` holders upgrade them
    /// ([`SystemState::may_upgrade`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub publishers: BTreeMap<String, PublisherBinding>,
    /// Package signing keys whose organization was deleted (`DeleteOrg`), and
    /// how many bindings each has had: the generation its next binding signs,
    /// so a signature that bound it once never binds it again. Left out when
    /// empty, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unbound_publishers: BTreeMap<String, u64>,
    /// When each session in `revoked_sessions` was revoked (Unix seconds), so
    /// it can be forgotten once no token of it can still be valid.
    #[serde(default)]
    pub revoked_at: BTreeMap<String, u64>,
    /// Refresh tokens already exchanged (hash -> whose session, until when): a
    /// second use is a stolen token, and revokes the session.
    #[serde(default)]
    pub rotated_refresh: BTreeMap<String, RefreshRecord>,
    /// The proofs decisions were logged with ([`crate::proof`]), each usable
    /// once: a node cannot log a decision again after its maker reversed it.
    #[serde(default)]
    pub used_proofs: BTreeSet<String>,
    /// The operator's verify key, recorded at genesis: what the operator's
    /// decisions are signed with.
    #[serde(default)]
    pub operator_key: Option<crate::proof::OperatorKey>,
    /// The log index from which sign-ups, sign-ins, refreshes, counters and
    /// API key revocations carry their proofs ([`SystemCmd::is_session_command`]):
    /// genesis's, or that of the [`SystemCmd::RequireSessionProofs`] the first
    /// leader on a release that makes them wrote. Entries before it, logged by
    /// releases that made none, apply as they did when a restart replays them.
    /// `None` (state from before): none required yet. Left out when `None`,
    /// so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_proofs_from: Option<u64>,
    /// Whether an app's upgrades, publisher and release approvals are its
    /// release's ([`SystemState::governing_release`]), and a package's WASM
    /// ships in no other package: from the [`SystemCmd::AttributeByRelease`]
    /// the first leader on a release that judges so wrote. Before it, logged
    /// entries replay judging the release each bundle was first recorded
    /// from, as the releases that logged them did. Left out when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub attributes_by_release: bool,
    /// The log index from which every entry applies under this release's
    /// rules ([`SystemCmd::hold_to_current_rules`]): the
    /// [`SystemCmd::RequireCurrentRules`] the first system leader on a release
    /// that makes it wrote. Entries before it, logged by releases whose
    /// commands named an older rule with a flag left `false`, apply as they
    /// did when a restart replays them. `None` (state from before): none
    /// required yet. Left out when `None`, so state from before reads and
    /// writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_rules_from: Option<u64>,
    /// Whether the sign-in proofs in [`Self::used_proofs`] expire: from the
    /// [`SystemCmd::ExpireProofs`] the first system leader on a release that
    /// makes it wrote. From then on a sign-in's or sign-up's proof is
    /// recorded with the expiry of the session it opened, which its passkey
    /// signed ([`Self::proof_expiry`]), and forgotten once that expiry passes.
    /// Before it, proofs are kept for good, as the releases that logged them
    /// kept them. Left out when `false`, so state from before reads and
    /// writes as it did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expires_proofs: bool,
    /// The proofs in [`Self::used_proofs`] that are forgotten at a time
    /// (Unix seconds): a sign-in's or sign-up's, by the expiry of the
    /// session it opened. Left out when empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub proof_expiry: BTreeMap<u64, BTreeSet<String>>,
    /// The latest time (an entry's own `now`, never a clock) up to which
    /// expired proofs were forgotten: a sign-in whose session expires by
    /// then is refused, since its proof may have been forgotten. 0: none
    /// forgotten yet. Left out when 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub proofs_forgotten_through: u64,
    /// The active cluster version ([`cluster_version`]): the newest core
    /// release every node supports, logged by the system leader
    /// (`SetClusterVersion`), only ever raised. `None` until the first.
    /// Left out when `None`, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_version: Option<String>,
    /// Who may install apps where deployments are granted
    /// ([`Governance::deployments`]), by [`Deployer::key`]
    /// (`account:<id>`, `org:<id>`); [`SystemState::may_deploy`] judges them.
    /// Left out when empty, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deployers: BTreeMap<String, DeployerGrant>,
    /// The cluster's owner (`SetClusterOwner`), named once by the operator
    /// where governance says `deployers: owner`; from then on it grants
    /// deployers, not the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_owner: Option<Deployer>,
    /// Deployments by id: each an owner's list of the apps it offers, every
    /// entry naming one installed release ([`Deployment`]). Left out when
    /// empty, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deployments: BTreeMap<String, Deployment>,
    /// The values a deployment's managers set for secrets its offered releases
    /// declare (`SetDeploymentSecret`), by deployment id, then name: every
    /// instance created from it uses one unless it set its own. Never in an
    /// answer, a report or a log line: only the outbound gate reads a value
    /// (`Node::outbound_for`, [`SystemState::deployment_secrets_of`]); the
    /// routes answer each one's fingerprint. Left out when empty, so state from
    /// before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deployment_secrets: BTreeMap<String, crate::component::InstanceSecrets>,
    /// Web origins packages' managers registered for their frontends
    /// (`AddPackageOrigin`), by origin: a passkey ceremony there opens a
    /// session bound to the package ([`crate::origins`]). Left out when
    /// empty, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub package_origins: BTreeMap<String, crate::origins::PackageOrigin>,
    /// Plaintext bytes of the files each app holds (`ChargeFileBytes`).
    #[serde(default)]
    pub app_file_bytes: BTreeMap<AppId, u64>,
    /// `app_file_bytes` summed per payer, kept in step by `set_app_file_bytes`.
    #[serde(default)]
    pub payer_file_bytes: BTreeMap<String, u64>,
    /// Cluster sign-up invites (`CreateSignUpInvite`), by the token's Ed25519
    /// public key ([`crate::proof::invite_key`]): what lets someone create an
    /// account where sign-up is `invite` or `closed`. Left out when empty, so
    /// state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sign_up_invites: BTreeMap<String, SignUpInvite>,
    /// The compute runs queued or granted, by app and run id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub compute: BTreeMap<AppId, BTreeMap<String, RunSlot>>,
    /// The `seq` the next queued run gets.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_run_seq: u64,
    /// When each payer last got a place, as a count of grants: free places go to the payer
    /// served longest ago. Only payers that still have runs are kept.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub run_served: BTreeMap<String, u64>,
    /// Places granted so far, the clock of `run_served`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub run_grants: u64,
    /// The last run ids released per app, oldest first.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub released_runs: BTreeMap<AppId, VecDeque<String>>,
    /// The public keys the recovery disk encrypts backups to, by
    /// [`BackupHolder::key`] (`SetBackupKey`): whoever upgrades an app holds
    /// the key that opens its backups ([`SystemState::backup_holder`]). Left
    /// out when empty, so state from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub backup_keys: BTreeMap<String, BackupKey>,
}

/// Who holds the key that opens an app's recovery-disk backups: the party
/// that upgrades it ([`SystemState::backup_holder`]); shared with clients,
/// which read the files.
pub use calimero_client::recovery::BackupHolder;

/// A holder's backup key: the X25519 public key (hex) the recovery disk
/// encrypts its backups to. The private half never reaches a node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupKey {
    pub holder: BackupHolder,
    pub public_key: String,
    /// Unix seconds: the command's own time.
    pub set_at: u64,
    /// The account that set it, or [`OPERATOR`].
    pub set_by: AccountId,
}

/// What a holder's recovery files carry of the system state beside its
/// apps' own: enough to bring them back on a cluster founded from nothing
/// ([`SystemState::holder_slice`]). Never another holder's apps, and of the
/// accounts only what signing in takes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HolderSlice {
    pub holder: BackupHolder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<Org>,
    pub accounts: Vec<SliceAccount>,
    pub apps: BTreeMap<AppId, AppRecord>,
    /// The bundles those apps run, and the releases that ship them.
    pub bundles: BTreeMap<String, BundleRecord>,
    pub packages: BTreeMap<String, PackageRecord>,
}

/// An account as a [`HolderSlice`] carries it: who it is and the passkeys it
/// signs in with, nothing it holds elsewhere.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceAccount {
    pub account_id: AccountId,
    pub name: String,
    pub created_at: u64,
    pub passkeys: Vec<Credential>,
}

/// One app an `ImportHolder` brings back: the replicas its new group gets
/// (the seed first) and the recovery file it is restored from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedApp {
    pub app_id: AppId,
    pub replicas: Vec<u64>,
    pub source: crate::types::RestoreSource,
}

/// What the operator's signature on an `ImportHolder` states: the holder,
/// the slice file's hash and each app's file and hash.
#[must_use]
pub fn import_holder_args(
    holder: &BackupHolder,
    slice_sha256: &str,
    apps: &[ImportedApp],
) -> Value {
    json!({
        "holder": holder.key(),
        "slice": slice_sha256,
        "apps": apps
            .iter()
            .map(|a| json!({ "app": a.app_id, "file": a.source.name, "sha256": a.source.sha256 }))
            .collect::<Vec<_>>(),
    })
}

/// `ImportHolder` where the operator upgrades apps, or for the operator.
pub const NOT_IMPORTABLE: &str =
    "only an organization's or account's files are imported, where owners upgrade apps";

/// `ImportHolder` of something this cluster holds already.
pub const ALREADY_HERE: &str =
    "this cluster holds that organization, account, app or passkey already";

/// The largest a backup key's hex is: 32 bytes.
pub const BACKUP_KEY_HEX_LEN: usize = 64;

/// A backup key as the log takes it: 64 lowercase hex, not all zeros (an
/// X25519 key that agrees on nothing).
#[must_use]
pub fn backup_key_fits(public_key: &str) -> bool {
    public_key.len() == BACKUP_KEY_HEX_LEN
        && public_key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && public_key.bytes().any(|b| b != b'0')
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A cluster sign-up invite: issued by the operator (its signature) or the
/// cluster's owner (a passkey), it lets `uses_left` people create an account
/// before `expires_at`, whatever the sign-up policy. Where sign-up is
/// `closed` it is the only way in: a pre-authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignUpInvite {
    /// Unix seconds; judged against the sign-up's own time.
    pub expires_at: u64,
    pub uses_left: u32,
    /// [`OPERATOR`], or the owner-side account.
    pub created_by: AccountId,
    /// Unix seconds.
    pub created_at: u64,
}

/// An invite presented at sign-up (`CreateAccount::invite`): the token's
/// public key and its key's signature over the new account and the time
/// ([`crate::proof::sign_invite_redeem`]), made by the node the person gave
/// the token to. An organization invite or a cluster sign-up invite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignUpRedemption {
    pub invite_key: String,
    /// Unix seconds, signed: the invite's expiry is judged against it.
    pub now: u64,
    pub signature: String,
    /// An organization invite's `all_apps` grant stands only while its
    /// creator is still an admin, as for `RedeemInvite { clamp_grants }`.
    /// Absent (`false`) in an earlier release's entry, which replays
    /// granting as minted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clamp_grants: bool,
}

/// How the sign-up policy let an account in ([`SystemState::admits`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// No invite, on an `open` cluster.
    Open,
    /// An organization invite: the account joins its organization.
    OrgInvite,
    /// A cluster sign-up invite.
    SignUpInvite,
}

/// How a refusal of a sign-up under the cluster's policy starts
/// ([`Rejection::sign_up`]). Only the wording: the API answers it `403
/// SIGN_UP_REFUSED` for its kind ([`RejectionKind::SignUpRefused`]).
pub const SIGN_UP_PREFIX: &str = "sign-up refused";

/// A refusal: no organization invite is kept under that token. The last use
/// of an invite removes it, as does deleting its organization, so a used-up
/// invite reads the same as one that never existed.
pub const INVITE_GONE: &str = "this invite was used up, revoked or never existed";

/// What `DeleteTeam` answers when its team's subtree holds apps and the
/// command carries no passkey consent over them.
pub const DELETE_TEAM_NEEDS_CONSENT: &str =
    "deleting a team that holds apps needs the deleter's passkey";
/// What `TransferApp` answers to someone who neither owns the app (as a
/// member of its organization, for an organization's app) nor administers
/// its organization.
pub const ONLY_MANAGERS_TRANSFER: &str =
    "only the app's owner, while a member of its organization, or an organization admin may transfer it";
/// A refusal: the organization invite is past its expiry.
pub const INVITE_EXPIRED: &str = "this invite has expired";
/// A refusal: renaming an organization is its owner's or an admin's.
pub const ONLY_ADMINS_RENAME: &str = "only the organization's owner and admins rename it";
/// A refusal: making, changing or dropping an app team, scoping apps, or
/// creating the first instance of a package without one in an organization
/// whose apps have app teams, is installing an app there: an admin's.
/// Refusal of a `DeleteApp` under the whole-unit rule naming a unit's member: it goes with
/// the instance whose release created it.
pub const UNIT_MEMBER: Rejection = Rejection::new(
    RejectionKind::UnitMember,
    "this app is a member of an instance's unit: it is deleted with that instance, not alone",
);
/// Refusal of a `DeleteApp` whose `members` are not the instance's unit as it stands.
pub const UNIT_NOT_AS_NAMED: &str = "the unit's members are not the ones the delete names";

pub const ONLY_ADMINS_INSTALL: &str =
    "only the organization's owner and admins install apps in it and change its app teams";
/// The personal organization a sign-up makes ([`SystemCmd::CreateAccount`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalOrgSeed {
    /// Its id, the node's choice as the account's is.
    pub org_id: OrgId,
    /// The release of the node the account signed up through, approved to
    /// hold its apps from the start.
    #[serde(default)]
    pub release: Option<String>,
}

/// A refusal: an app in a personal organization goes to its new owner's
/// personal organization, and they have none yet (the API makes it first).
pub const NO_PERSONAL_ORG: &str = "the new owner has no personal organization to take the app";
/// A refusal: an account owns at most one personal organization
/// ([`crate::types::Org::personal`]).
pub const PERSONAL_ORG_EXISTS: Rejection = Rejection::new(
    RejectionKind::PersonalOrgExists,
    "the account has a personal organization already",
);
/// A refusal: from the cluster version that makes organizations ordinary
/// ([`SystemState::ordinary_orgs`]) no personal organization is made; an account's own
/// organization is an ordinary one it creates.
pub const NO_PERSONAL_ORGS: Rejection = Rejection::new(
    RejectionKind::PersonalOrgsEnded,
    "personal organizations are no longer made: create an ordinary organization",
);
/// How far up a unit [`SystemState::guest_holder`] walks: a unit nests at most
/// `api::apps::MAX_UNIT_DEPTH` deep, so this leaves room and bounds a cycle.
pub const MAX_UNIT_LINKS_UP: usize = 8;

/// A refusal: the organization turned egress off, so it approves no outbound host
/// until it is on again; the API answers it `409 EGRESS_OFF`.
pub const EGRESS_OFF: Rejection = Rejection::new(
    RejectionKind::EgressOff,
    "egress is off for this organization: turn it on before approving outbound hosts",
);

/// Who turned egress off for an app ([`SystemState::egress_off`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressOff {
    /// Its organization, for every app of it.
    Org,
    /// The instance itself.
    Instance,
}

impl EgressOff {
    /// Who, as a refusal and a page name it.
    #[must_use]
    pub const fn who(self) -> &'static str {
        match self {
            Self::Org => "its organization",
            Self::Instance => "the instance",
        }
    }

    /// Its name on the wire (`offBy`).
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Org => "organization",
            Self::Instance => "instance",
        }
    }
}

/// An organization's settings, as `SetOrgSettings` changes them and its consent states them
/// (camelCase, a setting left out kept as it is).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrgSettings {
    /// Whether its instances take guests ([`crate::types::Org::allow_guests`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_guests: Option<bool>,
    /// Whether its instances' public links admit visitors
    /// ([`crate::types::Org::allow_public_links`], `Feature::PublicLinks`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_public_links: Option<bool>,
}

impl OrgSettings {
    /// Whether it changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allow_guests.is_none() && self.allow_public_links.is_none()
    }
}

/// Public links one instance holds at most ([`crate::types::PublicLink`]).
pub const MAX_PUBLIC_LINKS_PER_APP: usize = 16;
/// Public links the cluster holds at most: the system state is one checkpoint.
pub const MAX_PUBLIC_LINKS: usize = 10_000;
/// A public link's label, at most this many characters.
pub const MAX_PUBLIC_LINK_LABEL_CHARS: usize = 64;
/// The most calls a minute a public link may give one visitor (its `ratePerMinute`).
pub const MAX_PUBLIC_LINK_RATE: u32 = 600;
/// A public link's `ratePerMinute` when its request names none.
pub const DEFAULT_PUBLIC_LINK_RATE: u32 = 60;
/// A refusal: the instance's organization turned public links off.
pub const PUBLIC_LINKS_OFF: &str = "the instance's organization does not allow public links";
/// A refusal: a public link gone, revoked or never made.
pub const PUBLIC_LINK_GONE: &str = "this public link was revoked or never existed";

/// The arguments a public link's consent states: its key and terms.
#[must_use]
pub fn public_link_args(link_key: &str, link: &crate::types::PublicLink) -> Value {
    json!({
        "linkKey": link_key,
        "label": link.label,
        "expiresAt": link.expires_at,
        "ratePerMinute": link.rate_per_minute,
    })
}

/// The arguments an instance invite's consent states: its key and terms, and its scope when
/// it carries one.
#[must_use]
pub fn app_invite_args(invite_key: &str, invite: &crate::types::AppInvite) -> Value {
    let mut args = json!({
        "inviteKey": invite_key,
        "expiresAt": invite.expires_at,
        "uses": invite.uses_left,
    });
    if let Some(scope) = &invite.scope {
        args["scope"] = scope.clone();
    }
    args
}

/// Pending invites one instance holds at most ([`crate::types::AppInvite`]).
pub const MAX_APP_INVITES_PER_APP: usize = 64;
/// Pending instance invites the cluster holds at most: the system state is one checkpoint.
pub const MAX_APP_INVITES: usize = 10_000;
/// Uses one instance invite admits at most.
pub const MAX_APP_INVITE_USES: u32 = 1_000;
/// How long an instance invite lasts at most, from when it is made.
pub const MAX_APP_INVITE_TTL_SECS: u64 = 90 * 24 * 3600;
/// A refusal: the instance's release or its organization does not allow guests
/// ([`SystemState::guests_allowed`]).
pub const GUESTS_NOT_ALLOWED: Rejection = Rejection::new(
    RejectionKind::GuestsNotAllowed,
    "this instance takes no guests: its release or its organization does not allow them",
);
/// A refusal: an instance invite gone, used up or never made.
pub const APP_INVITE_GONE: &str = "this invite was used up, revoked or never existed";

/// A refusal: an organization's app is handed only to a member of it.
pub const NEW_OWNER_NOT_A_MEMBER: &str = "the new owner is not a member of the app's organization";
/// The name a personal organization is made with when the request names
/// none: never the account's name, which the deployment report would show.
pub const PERSONAL_ORG_NAME: &str = "Personal";
/// A refusal: a standing grant of every app added later ([`Org::all_apps`]),
/// by invite or set on a member, is the owner's or an admin's.
pub const ONLY_ADMINS_ALL_APPS: &str =
    "only the organization's owner and admins give someone every app added later";
/// A refusal: below admin, a team's rows, capabilities and default are set
/// only on a team the actor sees ([`cluster_version::Feature::HeldTeamGrants`]).
pub const TEAM_NOT_SEEN: Rejection = Rejection::new(
    RejectionKind::TeamNotSeen,
    "below admin, only a team you are in or lead has its rows, capabilities or default set",
);
/// A refusal: an organization invite whose creator could no longer make it
/// ([`cluster_version::Feature::InviteCreatorStanding`]).
pub const INVITE_CREATOR_LACKS: Rejection = Rejection::new(
    RejectionKind::InviteCreatorLacks,
    "this invite's creator can no longer give what it grants; ask for a new invite",
);
/// A refusal: capability bits that no capability has.
pub const UNKNOWN_CAPABILITIES: Rejection = Rejection::new(
    RejectionKind::UnknownCapabilities,
    "capabilities name a bit no capability has",
);
/// A refusal: an invite grants no capability its creator does not hold.
pub const CANNOT_GRANT_UNHELD: Rejection = Rejection::new(
    RejectionKind::CannotGrantUnheld,
    "an invite or a team row grants only capabilities its maker holds; only the owner and admins grant any",
);
/// A refusal: the package has an app team in this organization already.
pub const APP_TEAM_EXISTS: &str = "this package has an app team in this organization already";

/// What `LinkApp` answers to someone who does not manage both apps.
pub const ONLY_MANAGERS_LINK: &str = "only who manages both apps attaches one to the other";

/// What `MoveApp` answers to someone who does not manage the app where it is
/// now ([`SystemState::manages_app`]).
pub const ONLY_MANAGERS_MOVE: &str =
    "only the app's owner, while a member of its organization, or an organization admin may move it";
/// What `MoveApp` answers to someone who is not an admin or the owner of the
/// organization the app would move to.
pub const ONLY_ADMINS_MOVE_IN: &str =
    "only an admin or the owner of the target organization may move an app into it";
/// What `MoveApp` answers when the app's access is a team other than its
/// package's app team: that team's leads, read-only rows and capabilities are
/// grants of the organization it is in, which the target's admins never saw.
pub const MOVE_NEEDS_APP_TEAM: &str = "the app's access is a team other than its package's app team; give it organization or app-team access first";
/// What `MoveApp` answers when the app's owner would not be a member of the
/// target organization: an organization's app is owned by one of its members.
pub const MOVE_OWNER_NOT_IN_TARGET: &str = "the app's owner is not a member of the target organization; invite them, transfer the app to a member, or bring the members along";
/// What `MoveApp` answers when the target approves none of the releases that
/// hold the app now: its replicas would be sent none of its data.
pub const MOVE_RELEASES_DISJOINT: &str =
    "the target organization approves none of the releases that hold this app; approve one first";

/// How many cluster sign-up invites a cluster keeps at once.
pub const MAX_SIGN_UP_INVITES: usize = 10_000;

/// How many accounts one cluster sign-up invite may create.
pub const MAX_SIGN_UP_INVITE_USES: u32 = 10_000;

/// What a cluster sign-up invite's issuer signs (`cluster /
/// __create_sign_up_invite / ...`): its public key and its terms.
#[must_use]
pub fn sign_up_invite_args(invite_key: &str, invite: &SignUpInvite) -> Value {
    json!({
        "inviteKey": invite_key,
        "uses": invite.uses_left,
        "expiresAt": invite.expires_at,
    })
}

/// An owner's list of the apps it offers on this cluster. Customers create
/// instances from its offers ([`AppRecord::deployment`]); whoever
/// [acts for](SystemState::acts_for) its owner changes it, each with their
/// own passkey.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub id: String,
    /// Shown to customers ("acme-labs").
    pub name: String,
    /// An account, or an organization whose org-wide `MANAGE_APPLICATION`
    /// holders manage it.
    pub owner: Deployer,
    /// App name -> the release it offers.
    pub apps: BTreeMap<String, Offer>,
    pub created_by: AccountId,
    pub created_at: u64,
}

/// One app a deployment offers: the release new instances of it run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub package: String,
    pub version: String,
    pub added_by: AccountId,
    pub added_at: u64,
}

/// How many deployments one owner keeps.
pub const MAX_DEPLOYMENTS_PER_OWNER: usize = 16;

/// How many deployments a cluster keeps.
pub const MAX_DEPLOYMENTS: usize = 10_000;

/// How many apps one deployment offers.
pub const MAX_APPS_PER_DEPLOYMENT: usize = 64;

/// The longest deployment name, in characters, and the longest app name or
/// deployment id, in bytes.
pub const MAX_DEPLOYMENT_NAME_CHARS: usize = 64;

/// How many secret values one deployment keeps
/// ([`SystemState::deployment_secrets`]), each at most
/// [`crate::component::MAX_SECRET_VALUE_BYTES`]: as many as one release declares
/// ([`calimero_bundle::MAX_SECRETS`]).
pub const MAX_SECRETS_PER_DEPLOYMENT: usize = calimero_bundle::MAX_SECRETS;

/// How many deployment secret values a cluster keeps in all: the system state is
/// one checkpoint, so they stay under 16 MiB of it together.
pub const MAX_DEPLOYMENT_SECRETS: usize = 4096;

/// Why a deployment's secret is refused: no release it offers declares the name, nor (for
/// an entry judged by `SetDeploymentSecret::used`) one an offered release references.
pub const UNDECLARED_SECRET: Rejection =
    Rejection::new(RejectionKind::UndeclaredSecret, "no release this deployment offers, or one they use, declares that secret; offer one that does first");

/// The names of a secret a deployment reaches ([`SystemState::deployment_declarers`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretDeclarers {
    /// The deployment's apps whose offered release declares it, or references one that does.
    pub apps: BTreeSet<String>,
    /// The releases that declare it, as (`package`, `version`).
    pub declared_by: BTreeSet<(String, String)>,
}

/// Why an upgrade of an instance from a deployment is refused: it names a
/// release other than the one the deployment offers for the instance's package
/// (`SystemState::within_deployment`).
pub const OUTSIDE_DEPLOYMENT: Rejection =
    Rejection::new(RejectionKind::OutsideDeployment, "an instance from a deployment runs the release its deployment offers: its developer decides which, and its owner accepts it or not");

/// Why clearing a deployment's secret is refused: it holds no value of it.
pub const NO_DEPLOYMENT_SECRET: Rejection = Rejection::new(
    RejectionKind::NotFound,
    "this deployment holds no value of that secret",
);

/// A deployment's id: derived from the nonce its creator's consent signed,
/// so the consent binds it and no node picks another (32 lowercase hex
/// characters: URL- and command-line-safe).
#[must_use]
pub fn deployment_id(nonce: &str) -> String {
    let digest = calimero_identity::sha256(format!("calimero.deployment.v1\0{nonce}").as_bytes());
    hex::encode(&digest[..16])
}

/// A deployment's name: what customers see, one line of at most
/// [`MAX_DEPLOYMENT_NAME_CHARS`] characters.
///
/// # Errors
/// Empty, too long, padded or holding a control character.
pub fn deployment_name_fits(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty()
        || name.trim() != name
        || name.chars().count() > MAX_DEPLOYMENT_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err("a deployment's name is 1 to 64 characters on one line");
    }
    Ok(())
}

/// The longest organization name, in characters.
pub const MAX_ORG_NAME_CHARS: usize = 64;

/// An organization's name, as it is created ([`SystemCmd::CreateOrg`]) and
/// renamed ([`SystemCmd::RenameOrg`]): what its members and invitees read,
/// one line of at most [`MAX_ORG_NAME_CHARS`] characters.
///
/// # Errors
/// Empty, too long, padded or holding a control character.
pub fn org_name_fits(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty()
        || name.trim() != name
        || name.chars().count() > MAX_ORG_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err("an organization's name is 1 to 64 characters on one line");
    }
    Ok(())
}

/// An app's name in a deployment, or a deployment's id: a path segment
/// (`/deployments/{id}/apps/{app}`, `/d/{id}/`), so ASCII letters, digits,
/// `.`, `_` and `-`, starting with a letter or digit, at most
/// [`MAX_DEPLOYMENT_NAME_CHARS`] bytes.
///
/// # Errors
/// Anything else.
pub fn deployment_segment_fits(s: &str) -> Result<(), &'static str> {
    let fits = !s.is_empty()
        && s.len() <= MAX_DEPLOYMENT_NAME_CHARS
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !fits {
        return Err("an app name is 1 to 64 letters, digits, '.', '_' or '-', starting with a letter or digit");
    }
    Ok(())
}

pub use calimero_client::attest::{Authority, Deployers, Deployments, Governance, SignUp};

/// An account, or an organization, as a deployer or the cluster's owner.
/// Its JSON names the kind: `{"account":"<id>"}`, `{"org":"<id>"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Deployer {
    Account(AccountId),
    Org(OrgId),
}

impl Deployer {
    /// Its key in [`SystemState::deployers`].
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Account(a) => format!("account:{a}"),
            Self::Org(o) => format!("org:{o}"),
        }
    }

    /// The deployer [`Self::key`] names.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        if let Some(a) = key.strip_prefix("account:") {
            return Some(Self::Account(a.to_owned()));
        }
        key.strip_prefix("org:").map(|o| Self::Org(o.to_owned()))
    }
}

/// A grant to install apps where deployments are granted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployerGrant {
    /// When it ends (Unix seconds, judged against an install's own time);
    /// `None`: it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    /// When it was granted (Unix seconds).
    pub granted_at: u64,
    /// Who granted it: [`OPERATOR`], or the owner-side account.
    pub granted_by: AccountId,
}

impl DeployerGrant {
    /// Whether it still lets its holder install at `now`.
    #[must_use]
    pub fn live(&self, now: u64) -> bool {
        self.until.is_none_or(|until| now < until)
    }
}

/// What an install is refused with where deployments are granted and the
/// installer holds no live grant; the API answers it `403
/// DEPLOY_NOT_GRANTED` (`node::rejected`).
pub const NOT_A_DEPLOYER: Rejection =
    Rejection::new(RejectionKind::DeployNotGranted, "This cluster only lets Calimero Cloud deployers install apps; ask the cluster's owner or operator for a deployer grant.");

/// A refusal: a release is installed for an organization its installer is not a member of,
/// or an account installs a private release for none.
pub const NOT_AN_ORG_MEMBER: &str =
    "an account installs a private release for an organization it is a member of";

/// How many deployer grants a cluster keeps.
pub const MAX_DEPLOYERS: usize = 10_000;

/// Whose release approvals a change is to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ReleaseScope {
    Org {
        org: OrgId,
    },
    /// Every app, by the operator: only where the deployment gave it this
    /// decision ([`Governance`]).
    Cluster,
}

/// How a refusal of new work past a release deadline starts
/// ([`Stall::rejection`]). Only the wording: the API answers it `409
/// RELEASE_DEADLINE` for its kind ([`RejectionKind::ReleaseDeadline`]).
pub const DEADLINE_PREFIX: &str = "new work stopped";

/// How many releases may carry a deadline at once.
pub const MAX_RELEASE_DEADLINES: usize = 64;

/// The longest release version a deadline names.
pub const MAX_RELEASE_VERSION_BYTES: usize = 64;

/// Why a scope (an organization, or an account's own apps) may start no new
/// work: the newest release whose deadline has passed and that the scope has
/// not approved (nor anything newer), and that deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stall {
    pub release: String,
    /// Unix seconds.
    pub after: u64,
}

impl Stall {
    /// The refusal ([`RejectionKind::ReleaseDeadline`]), naming the release
    /// to approve and its deadline.
    #[must_use]
    pub fn rejection(&self) -> Rejection {
        Rejection::with(RejectionKind::ReleaseDeadline, self.message())
    }

    /// The refusal's message.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "{DEADLINE_PREFIX}: approve mero-tee release {} (its deadline, {}, has passed); \
             apps already running keep running",
            self.release,
            rfc3339(self.after)
        )
    }
}

/// Unix seconds as RFC 3339 (UTC), or the number itself past what that writes.
#[must_use]
pub fn rfc3339(secs: u64) -> String {
    i64::try_from(secs)
        .ok()
        .and_then(|s| time::OffsetDateTime::from_unix_timestamp(s).ok())
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| secs.to_string())
}

#[cfg(test)]
mod time_format_tests {
    /// The strings chrono wrote before the node moved to `time` (RUSTSEC-2026-0543): the API
    /// keeps answering them byte for byte.
    #[test]
    fn times_read_as_they_did() {
        assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::rfc3339(1_763_164_800), "2025-11-15T00:00:00Z");
        assert_eq!(super::rfc3339(1_763_208_245), "2025-11-15T12:04:05Z");
        assert_eq!(super::rfc3339(u64::MAX), u64::MAX.to_string());
    }
}

/// What a package signing key signs to bind it to an organization on a
/// cluster: the cluster's name, the organization, the key itself, and the
/// binding's generation (how many bindings the key has had there), so a
/// signature binds one organization once and is never replayed, on this
/// cluster or another, after the key moved on.
#[must_use]
pub fn publisher_binding_message(
    cluster: &str,
    org: &str,
    signer_id: &str,
    generation: u64,
) -> Vec<u8> {
    format!("calimero-publisher-binding/v1\n{cluster}\n{org}\n{signer_id}\n{generation}")
        .into_bytes()
}

/// The longest signer id or cluster name a binding names.
pub const MAX_BINDING_FIELD_BYTES: usize = 128;

/// Which organization publishes the packages a signing key signs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublisherBinding {
    /// The organization the key is bound to.
    pub org: OrgId,
    /// How many bindings the key has had; the next one signs this number.
    pub generation: u64,
}

/// A package signing key's signature binding it to an organization: the key
/// (base64url of its 32 Ed25519 bytes) and its signature over
/// [`publisher_binding_message`] (base64url of 64 bytes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublisherProof {
    pub public_key: String,
    pub signature: String,
}

impl PublisherProof {
    /// Whether this is `signer_id`'s key signing `message`.
    #[must_use]
    pub fn proves(&self, signer_id: &str, message: &[u8]) -> bool {
        let (Ok(key), Ok(sig)) = (
            calimero_identity::from_b64url(&self.public_key),
            calimero_identity::from_b64url(&self.signature),
        ) else {
            return false;
        };
        let (Ok(key), Ok(sig)) = (<[u8; 32]>::try_from(key), <[u8; 64]>::try_from(sig)) else {
            return false;
        };
        if calimero_bundle::derive_signer_id_did_key(&key) != signer_id {
            return false;
        }
        let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&key) else {
            return false;
        };
        key.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&sig))
            .is_ok()
    }
}

/// How long a revoked session is remembered: past every token it could have
/// minted (sessions live 15 minutes), with room for clock differences.
pub const REVOKED_SESSION_KEEP_SECS: u64 = 60 * 60;

/// `addr` as the same node's other spellings share it: scheme and host
/// lowercased, the default port dropped, trailing slashes trimmed.
#[must_use]
pub fn normalize_addr(addr: &str) -> String {
    match url::Url::parse(addr) {
        Ok(u) if u.origin().is_tuple() => format!(
            "{}{}",
            u.origin().ascii_serialization(),
            u.path().trim_end_matches('/')
        ),
        _ => addr.trim_end_matches('/').to_ascii_lowercase(),
    }
}

impl SystemState {
    /// The nodes that hold apps and the cluster key: every site that picks nodes for the
    /// cluster's own work goes through this, never `nodes` directly.
    pub fn replica_nodes(&self) -> impl Iterator<Item = &NodeRecord> {
        self.nodes.values().filter(|n| n.role == NodeRole::Replica)
    }

    /// The replica node `id`, if it is one.
    #[must_use]
    pub fn replica_node(&self, id: &u64) -> Option<&NodeRecord> {
        self.nodes.get(id).filter(|n| n.role == NodeRole::Replica)
    }

    /// The nodes that only run compute.
    pub fn job_nodes(&self) -> impl Iterator<Item = &NodeRecord> {
        self.nodes.values().filter(|n| n.role == NodeRole::Job)
    }

    /// The `(runs, fuel, file bytes)` `payer` ([`crate::usage::payer`]) may hold.
    #[must_use]
    pub fn payer_quotas(&self, payer: &str) -> (u32, u64, u64) {
        limits_of(&self.quotas, &self.org_quotas, payer)
    }

    /// The pool a run of `memory_mib` goes to: "" with no pools, else the smallest that fits.
    #[must_use]
    pub fn pool_for(&self, memory_mib: u32) -> Option<String> {
        if self.pools.is_empty() {
            return Some(String::new());
        }
        self.pools
            .iter()
            .filter(|(_, p)| p.max_memory_mib >= memory_mib)
            .min_by_key(|(name, p)| (p.max_memory_mib, name.as_str()))
            .map(|(name, _)| name.clone())
    }

    /// `name` when the cluster defines that pool and a run of `memory_mib` fits it.
    fn named_pool_for(&self, name: String, memory_mib: u32) -> Option<String> {
        let fits = self.pools.get(&name)?.max_memory_mib >= memory_mib;
        fits.then_some(name)
    }

    /// The pool `slot` counts against (a slot from before pools belongs to the default one).
    fn slot_pool<'a>(&self, slot: &'a RunSlot) -> &'a str {
        match (self.pools.is_empty(), slot.pool.as_str()) {
            (true, _) => "",
            (false, "") => DEFAULT_POOL,
            (false, pool) => pool,
        }
    }

    /// The pool the leader dispatches run `run` of `app` to, as `grant_runs` counts it.
    #[must_use]
    pub fn dispatch_pool(&self, app: &str, run: &str) -> Option<String> {
        self.run_slot(app, run)
            .map(|slot| self.slot_pool(slot).to_owned())
    }

    /// Whether `node` serves `pool`; every job node does on a cluster with no pools.
    #[must_use]
    pub fn node_in_pool(&self, node: &NodeRecord, pool: &str) -> bool {
        self.pools.is_empty() || Self::node_pool(node) == pool
    }

    fn node_pool(node: &NodeRecord) -> &str {
        if node.pool.is_empty() {
            DEFAULT_POOL
        } else {
            &node.pool
        }
    }

    /// The runs `pool`'s job nodes take at once; a pool the cluster does not define has none.
    #[must_use]
    pub fn pool_capacity(&self, pool: &str) -> usize {
        if self.pools.is_empty() {
            return self.job_nodes().count() * RUNS_PER_JOB_NODE;
        }
        let Some(config) = self.pools.get(pool) else {
            return 0;
        };
        let nodes = self
            .job_nodes()
            .filter(|n| Self::node_pool(n) == pool)
            .count();
        nodes * config.runs_per_node as usize
    }

    /// Sets the file bytes an app holds, and its payer's total with it.
    fn set_app_file_bytes(&mut self, app_id: &str, now: u64) {
        let Some(app) = self.apps.get(app_id) else {
            return;
        };
        let payer = crate::usage::payer(app);
        let before = self.app_file_bytes.get(app_id).copied().unwrap_or(0);
        let total = self.payer_file_bytes.entry(payer.clone()).or_default();
        *total = total.saturating_sub(before).saturating_add(now);
        if *total == 0 {
            let _gone = self.payer_file_bytes.remove(&payer);
        }
        if now == 0 {
            let _gone = self.app_file_bytes.remove(app_id);
        } else {
            let _prev = self.app_file_bytes.insert(app_id.to_owned(), now);
        }
    }

    /// The members of `app_id`'s unit: the apps its release created with it (its `links`),
    /// and theirs in turn, that still exist; in order. Empty for an app that created none.
    #[must_use]
    pub fn unit_of(&self, app_id: &str) -> Vec<AppId> {
        let mut members = BTreeSet::new();
        let mut next = vec![app_id];
        while let Some(id) = next.pop() {
            let Some(a) = self.apps.get(id) else {
                continue;
            };
            for m in a.links.values() {
                if m != app_id && self.apps.contains_key(m) && members.insert(m.clone()) {
                    next.push(m);
                }
            }
        }
        members.into_iter().collect()
    }

    /// The instance whose unit `app_id` is a member of: the app whose `links` name it.
    #[must_use]
    pub fn unit_parent(&self, app_id: &str) -> Option<&str> {
        self.apps
            .values()
            .find(|a| a.app_id != app_id && a.links.values().any(|l| l == app_id))
            .map(|a| a.app_id.as_str())
    }

    /// Removes an app, the file bytes counted against its payer and the compute runs it held.
    fn remove_app(&mut self, app_id: &str) {
        self.set_app_file_bytes(app_id, 0);
        let _gone = self.apps.remove(app_id);
        // A grant from or to the app goes with it.
        for sources in self.delegations.values_mut() {
            let _from = sources.remove(app_id);
            for targets in sources.values_mut() {
                let _to = targets.remove(app_id);
            }
            sources.retain(|_, t| !t.is_empty());
        }
        self.delegations.retain(|_, s| !s.is_empty());
        // An app attached elsewhere is detached with it; a unit member's link stays, as the
        // record that it was made with the instance.
        for a in self.apps.values_mut() {
            a.attached.retain(|_, t| t != app_id);
        }
        let _ring = self.released_runs.remove(app_id);
        self.app_invites.retain(|_, i| i.app != app_id);
        if self.compute.remove(app_id).is_some() {
            self.grant_runs();
        }
    }

    /// The slot of `run` of `app`.
    #[must_use]
    pub fn run_slot(&self, app: &str, run: &str) -> Option<&RunSlot> {
        self.compute.get(app)?.get(run)
    }

    /// The granted runs each pool holds, and the `(runs, fuel)` each payer holds.
    fn held(&self) -> (BTreeMap<String, usize>, BTreeMap<String, (u32, u64)>) {
        let mut granted_in: BTreeMap<String, usize> = BTreeMap::new();
        let mut load: BTreeMap<String, (u32, u64)> = BTreeMap::new();
        for slot in self.compute.values().flat_map(BTreeMap::values) {
            if slot.refused || !slot.granted {
                continue;
            }
            *granted_in
                .entry(self.slot_pool(slot).to_owned())
                .or_default() += 1;
            let held = load.entry(slot.payer.clone()).or_default();
            held.0 += 1;
            held.1 = held.1.saturating_add(slot.fuel);
        }
        (granted_in, load)
    }

    /// Why a run the queue has not granted waits, by the tests `grant_runs` applies: no place in
    /// its pool at all, every place held, its payer at the run quota, then at the fuel quota.
    #[must_use]
    pub fn wait_reason(&self, app: &str, run: &str) -> Option<&'static str> {
        let slot = self
            .run_slot(app, run)
            .filter(|s| !s.granted && !s.refused)?;
        let pool = self.slot_pool(slot);
        let capacity = self.pool_capacity(pool);
        let (granted_in, load) = self.held();
        let (max_runs, max_fuel, _) = self.payer_quotas(&slot.payer);
        let (runs, fuel) = load.get(&slot.payer).copied().unwrap_or_default();
        if capacity == 0 {
            Some("NO_CAPACITY")
        } else if granted_in.get(pool).copied().unwrap_or(0) >= capacity {
            Some("POOL_FULL")
        } else if runs >= max_runs {
            Some("PAYER_AT_RUN_QUOTA")
        } else if fuel.saturating_add(slot.fuel) > max_fuel {
            Some("PAYER_AT_FUEL_QUOTA")
        } else {
            None
        }
    }

    /// Grants waiting runs up to the job nodes' places, to the payers in turn (longest served
    /// first); a payer at a quota is passed over for the pass. Granted runs stay.
    fn grant_runs(&mut self) {
        for slot in self.compute.values_mut().flat_map(BTreeMap::values_mut) {
            let fuel = limits_of(&self.quotas, &self.org_quotas, &slot.payer).1;
            slot.refused |= !slot.granted && slot.fuel > fuel;
        }
        let (mut granted_in, mut load) = self.held();
        let mut waiting: BTreeMap<String, Vec<WaitingRun>> = BTreeMap::new();
        for (app, runs) in &self.compute {
            for (run, slot) in runs.iter().filter(|(_, s)| !s.refused && !s.granted) {
                waiting.entry(slot.payer.clone()).or_default().push((
                    slot.seq,
                    app.clone(),
                    run.clone(),
                    slot.fuel,
                    self.slot_pool(slot).to_owned(),
                ));
            }
        }
        let mut waiting: BTreeMap<String, VecDeque<_>> = waiting
            .into_iter()
            .map(|(payer, mut runs)| {
                runs.sort();
                (payer, runs.into())
            })
            .collect();
        let mut grants = Vec::new();
        let mut passed = BTreeSet::new();
        loop {
            let next = waiting
                .iter()
                .filter(|(payer, _)| !passed.contains(*payer))
                .filter_map(|(payer, queue)| {
                    let served = self.run_served.get(payer).copied().unwrap_or(0);
                    Some((served, queue.front()?.0, payer.clone()))
                })
                .min();
            let Some((_, _, payer)) = next else { break };
            let Some((_, app, run, fuel, pool)) =
                waiting.get_mut(&payer).and_then(VecDeque::pop_front)
            else {
                break;
            };
            // A full pool stays full for this pass: the run waits, the payer's next may go.
            let used = granted_in.entry(pool.clone()).or_default();
            if *used >= self.pool_capacity(&pool) {
                continue;
            }
            let (max_runs, max_fuel, _) = self.payer_quotas(&payer);
            let held = load.entry(payer.clone()).or_default();
            if held.0 < max_runs && held.1.saturating_add(fuel) <= max_fuel {
                held.0 += 1;
                held.1 += fuel;
                *used += 1;
                self.run_grants += 1;
                let _prev = self.run_served.insert(payer, self.run_grants);
                grants.push((app, run));
            } else {
                let _new = passed.insert(payer);
            }
        }
        for (app, run) in grants {
            if let Some(slot) = self.compute.get_mut(&app).and_then(|r| r.get_mut(&run)) {
                slot.granted = true;
            }
        }
        let payers: BTreeSet<&String> = self
            .compute
            .values()
            .flat_map(BTreeMap::values)
            .map(|slot| &slot.payer)
            .collect();
        self.run_served.retain(|payer, _| payers.contains(payer));
    }

    /// The file bytes held by the apps `payer` ([`crate::usage::payer`]) is billed for.
    #[must_use]
    pub fn file_bytes(&self, payer: &str) -> u64 {
        self.payer_file_bytes.get(payer).copied().unwrap_or(0)
    }

    /// The releases approved to hold `app`'s data: its organization's (or its
    /// publisher's, [`Self::approved_for_release`]). Empty means none was ever
    /// recorded, which leaves the app unrestricted.
    #[must_use]
    pub fn approved_releases(&self, app: &AppRecord) -> Option<&BTreeSet<String>> {
        self.approved_for_release(self.governing_release(app), &app.org)
    }

    /// The organization that publishes the package `signer` signs: the one
    /// that proved it holds the key and still exists.
    #[must_use]
    pub fn publisher_of_signer(&self, signer: &str) -> Option<&OrgId> {
        self.publishers
            .get(signer)
            .map(|b| &b.org)
            .filter(|o| self.orgs.contains_key(o.as_str()))
    }

    /// The generation the next binding of `signer` signs.
    #[must_use]
    pub fn binding_generation(&self, signer: &str) -> u64 {
        self.publishers.get(signer).map_or_else(
            || self.unbound_publishers.get(signer).copied().unwrap_or(0),
            |b| b.generation,
        )
    }

    /// Whether `account` holds `APPROVE_RELEASES` in `org`, whoever holds
    /// release approvals: what binding a publisher key takes, since the
    /// binding decides whose approvals cover the key's packages.
    #[must_use]
    pub fn holds_approve_releases(&self, org: &str, account: &str) -> bool {
        self.orgs.get(org).is_some_and(|o| {
            o.role(account).is_some_and(|r| r >= OrgRole::Member)
                && o.may(account, None, crate::types::caps::APPROVE_RELEASES)
        })
    }

    /// The organization that publishes `release` (a governing release:
    /// [`Self::governing_release`]); `None` for a raw upload or a package
    /// whose key is bound to no organization.
    #[must_use]
    pub fn publisher_of(&self, release: Option<&PackageRef>) -> Option<&OrgId> {
        self.publisher_of_signer(&self.packages.get(&release?.package)?.signer_id)
    }

    /// The personal organization `account` owns
    /// ([`crate::types::Org::personal`]), if any, and only while personal organizations
    /// are kept apart: from [`Self::ordinary_orgs`] on, none is (such an organization is an
    /// ordinary one with its owner as member).
    #[must_use]
    pub fn personal_org_of(&self, account: &str) -> Option<&OrgId> {
        if self.ordinary_orgs() {
            return None;
        }
        self.orgs
            .iter()
            .find(|(_, o)| o.personal && o.role(account) == Some(OrgRole::Owner))
            .map(|(id, _)| id)
    }

    /// The release whose rules govern an app running `app.bundle`: its
    /// upgrades, its publisher and so its release approvals
    /// ([`Self::governing`]).
    #[must_use]
    pub fn governing_release<'a>(&'a self, app: &'a AppRecord) -> Option<&'a PackageRef> {
        // When it took its bundle: created with it, or upgraded to it since.
        let since = app
            .last_upgrade
            .as_ref()
            .filter(|u| u.ok && u.to == app.bundle)
            .map_or(app.created_at, |u| u.at);
        self.governing(app.release.as_ref(), &app.bundle, since)
    }

    /// The release whose rules govern `bundle`, named as release `release`
    /// and taken at `since`: `release` itself, never merely the first one
    /// recorded as shipping the same WASM. Without one (a raw upload, a
    /// bundle named by hash, a record from before releases were kept), the
    /// release the bundle is recorded from, if it was installed by `since`:
    /// a release installed later does not take over an app running a raw
    /// upload of the same WASM. Before [`Self::attributes_by_release`], the
    /// recorded release alone, as earlier releases judged.
    #[must_use]
    pub fn governing<'a>(
        &'a self,
        release: Option<&'a PackageRef>,
        bundle: &str,
        since: u64,
    ) -> Option<&'a PackageRef> {
        let recorded = || self.bundles.get(bundle).and_then(|b| b.package.as_ref());
        if !self.attributes_by_release {
            return recorded();
        }
        if release.is_some() {
            return release;
        }
        recorded().filter(|r| {
            self.packages
                .get(&r.package)
                .and_then(|p| p.versions.get(&r.version))
                .is_some_and(|v| v.installed_at <= since)
        })
    }

    /// The releases approved to hold an app of `org` governed by `release`
    /// ([`Self::governing_release`]): the operator's where it holds release
    /// approvals; else the release's publisher's, for every instance of its
    /// package; else, for a raw upload or an unbound package, the app's own
    /// organization's ([`Self::approved_for`]).
    #[must_use]
    pub fn approved_for_release(
        &self,
        release: Option<&PackageRef>,
        org: &str,
    ) -> Option<&BTreeSet<String>> {
        match self.publisher_of(release) {
            Some(p) if self.governance().releases == Authority::Owner => self.approved_for(p),
            _ => self.approved_for(org),
        }
    }

    /// Whether `who` (an account, or [`OPERATOR`]) may use the release `v`: list it,
    /// install, create, upgrade to or offer it. Everyone, for a release the operator
    /// installed (the cluster's own catalog); the operator, only those. A public release:
    /// everyone. A private one: a member of an organization it was installed for; installed
    /// for none (its installers were in none), its installing accounts. A release
    /// recorded before either (`visibility` absent): whoever installed it, an account
    /// sharing an organization with an installing account, and an account that reaches an
    /// app running one of its services. Anyone else is answered as if it were not installed.
    #[must_use]
    pub fn sees_release(&self, v: &PackageVersion, who: &str) -> bool {
        if v.installed_by_any(OPERATOR) {
            return true;
        }
        if who == OPERATOR {
            return false;
        }
        match v.visibility {
            Some(Visibility::Public) => true,
            Some(Visibility::Private) if v.orgs.is_empty() => v.installed_by_any(who),
            Some(Visibility::Private) => v.orgs.iter().any(|o| self.member_of(o, who)),
            None => {
                v.installed_by_any(who)
                    || v.installing_accounts().any(|installer| {
                        self.orgs
                            .values()
                            .any(|o| o.role(installer).is_some() && o.role(who).is_some())
                    })
                    || self.reaches_release(v, who)
            }
        }
    }

    /// Whether `who` may read the facts of the release `v` of `package` (its name and version
    /// where an instance runs it): [`Self::sees_release`], or `who` reaches an app running it
    /// (one that names no release, only when `v` is not recorded private).
    #[must_use]
    pub fn reads_release(&self, package: &str, v: &PackageVersion, who: &str) -> bool {
        self.sees_release(v, who)
            || (who != OPERATOR
                && self.apps.values().any(|a| {
                    // Releases may share a WASM: an app runs the one its record names, and
                    // one naming none is never told a private sibling.
                    a.release
                        .as_ref()
                        .map_or(v.visibility != Some(Visibility::Private), |r| {
                            r.package == package && r.version == v.version
                        })
                        && v.app_parts().values().any(|h| *h == a.bundle)
                        && self.may_reach(a, Some(who))
                }))
    }

    /// Whether `who` reaches an app running one of `v`'s services.
    fn reaches_release(&self, v: &PackageVersion, who: &str) -> bool {
        self.apps
            .values()
            .any(|a| v.app_parts().values().any(|h| *h == a.bundle) && self.may_reach(a, Some(who)))
    }

    /// [`Self::sees_release`] for a session bound to the package's frontend (`frontend`):
    /// a release recorded before `visibility` keeps the rule from before, every release of
    /// it; any other, what the account sees.
    #[must_use]
    pub fn sees_release_from(&self, v: &PackageVersion, who: &str, frontend: bool) -> bool {
        (frontend && v.visibility.is_none()) || self.sees_release(v, who)
    }

    /// Whether `who` is a member (or above) of `org`.
    #[must_use]
    pub fn member_of(&self, org: &str, who: &str) -> bool {
        self.orgs
            .get(org)
            .and_then(|o| o.role(who))
            .is_some_and(|r| r >= OrgRole::Member)
    }

    /// The organizations `v` was installed for that `who` is a member of: never another.
    pub fn release_orgs_of<'a>(
        &'a self,
        v: &'a PackageVersion,
        who: &'a str,
    ) -> impl Iterator<Item = &'a crate::types::Org> + 'a {
        v.orgs
            .iter()
            .filter(move |o| self.member_of(o, who))
            .filter_map(|o| self.orgs.get(o))
    }

    // The lookups below are how a route reads a release a client names; anything else
    // answers as if it were not installed (`tests/it/release_lookups.rs`).

    /// The releases of `package` that `who` may use ([`Self::sees_release`]); `frontend`: a
    /// session bound to the package's frontend ([`Self::sees_release_from`]).
    pub fn releases_used<'a>(
        &'a self,
        package: &str,
        who: &'a str,
        frontend: bool,
    ) -> impl Iterator<Item = &'a PackageVersion> + 'a {
        self.packages
            .get(package)
            .into_iter()
            .flat_map(|p| p.versions.values())
            .filter(move |v| self.sees_release_from(v, who, frontend))
    }

    /// `package` if `who` may use one of its releases ([`Self::releases_used`]).
    #[must_use]
    pub fn package_used(&self, package: &str, who: &str, frontend: bool) -> Option<&PackageRecord> {
        let p = self.packages.get(package)?;
        p.versions
            .values()
            .any(|v| self.sees_release_from(v, who, frontend))
            .then_some(p)
    }

    /// `package@version` if `who` may use it ([`Self::sees_release`]).
    #[must_use]
    pub fn release_used(&self, package: &str, version: &str, who: &str) -> Option<&PackageVersion> {
        self.packages
            .get(package)?
            .versions
            .get(version)
            .filter(|v| self.sees_release(v, who))
    }

    /// The newest release of `package` that `who` may use.
    #[must_use]
    pub fn newest_used(&self, package: &str, who: &str) -> Option<&PackageVersion> {
        self.packages
            .get(package)?
            .latest_where(|v| self.sees_release(v, who))
    }

    /// The newest release of `package` whose facts `who` reads ([`Self::reads_release`]); a
    /// session bound to its frontend (`frontend`) also every release recorded before
    /// `visibility`.
    #[must_use]
    pub fn newest_read(&self, package: &str, who: &str, frontend: bool) -> Option<&PackageVersion> {
        self.packages.get(package)?.latest_where(|v| {
            (frontend && v.visibility.is_none()) || self.reads_release(package, v, who)
        })
    }

    /// `to`, for an instance `app` from a deployment, when the move stays on that deployment
    /// ([`Self::within_deployment`]: its offer that still serves, or a unit member's target in
    /// `run`), whether or not its owner may use the release otherwise.
    #[must_use]
    pub fn release_offered_to(
        &self,
        app: &AppRecord,
        to: &PackageRef,
        run: Option<&str>,
    ) -> Option<&PackageVersion> {
        app.deployment.as_ref()?;
        self.within_deployment(app, Some(to), run).ok()?;
        self.packages.get(&to.package)?.versions.get(&to.version)
    }

    /// Whether a WASM of `release`, to be installed under `package` signed by
    /// `signer_id`, ships in a release of another package signed by another
    /// key: a WASM belongs to one signer, whose packages alone may name it.
    /// One signer may ship a WASM in several packages: each instance is
    /// judged by the release it runs ([`AppRecord::release`]).
    #[must_use]
    pub fn ships_in_other_package(
        &self,
        package: &str,
        signer_id: &str,
        release: &PackageVersion,
    ) -> bool {
        release.parts(package).any(|(hash, _)| {
            self.packages
                .iter()
                .filter(|(name, p)| name.as_str() != package && p.signer_id != signer_id)
                .any(|(name, p)| {
                    p.versions
                        .values()
                        .any(|v| v.parts(name).any(|(h, _)| h == hash))
                })
        })
    }

    /// Whether a release of a package signed by `signer`, installed by `actor` and private to
    /// `private_for` (`None`: not private), may pin `m` of `module` as a pipeline module: one
    /// `actor` may use that is not recorded private, the operator's, its own signer's, or a
    /// private one installed for every organization the installing release is private to.
    #[must_use]
    pub fn may_pin(
        &self,
        module: &PackageRecord,
        m: &PackageVersion,
        actor: &str,
        signer: &str,
        private_for: Option<&BTreeSet<OrgId>>,
    ) -> bool {
        self.sees_release(m, actor)
            && (m.visibility != Some(Visibility::Private)
                || m.installed_by_any(OPERATOR)
                || module.signer_id == signer
                || private_for.is_some_and(|orgs| orgs.is_subset(&m.orgs)))
    }

    /// Whether every module `release` pins passes [`Self::may_pin`].
    #[must_use]
    pub fn may_pin_all(
        &self,
        release: &PackageVersion,
        actor: &str,
        signer: &str,
        private_for: Option<&BTreeSet<OrgId>>,
    ) -> bool {
        release
            .pipelines
            .values()
            .flat_map(|r| r.modules.values())
            .all(|pin| {
                self.packages.get(&pin.package).is_some_and(|p| {
                    p.versions
                        .get(&pin.version)
                        .is_some_and(|m| self.may_pin(p, m, actor, signer, private_for))
                })
            })
    }

    /// Whether a release other than `package`'s own pins `package@version` as a pipeline module.
    #[must_use]
    pub fn module_pinned(&self, package: &str, version: &str) -> bool {
        self.packages
            .iter()
            .filter(|(name, _)| name.as_str() != package)
            .flat_map(|(_, p)| p.versions.values())
            .flat_map(|v| v.pipelines.values())
            .flat_map(|r| r.modules.values())
            .any(|m| m.package == package && m.version == version)
    }

    /// Whether every module `release` pins is an installed release with that compute component.
    #[must_use]
    pub fn modules_installed(&self, release: &PackageVersion) -> bool {
        release
            .pipelines
            .values()
            .flat_map(|r| r.modules.values())
            .all(|m| {
                self.packages
                    .get(&m.package)
                    .and_then(|p| p.versions.get(&m.version))
                    .is_some_and(|v| v.compute.get("") == Some(&m.compute))
            })
    }

    /// Whether `hash` is a release's compute component, which no app runs.
    #[must_use]
    pub fn is_compute_bundle(&self, hash: &str) -> bool {
        self.bundles
            .get(hash)
            .and_then(|b| b.package.as_ref())
            .is_some_and(|at| at.compute)
    }

    /// The components of the release `r` names; an app from a raw upload has an app component.
    #[must_use]
    pub fn components_of(&self, r: Option<&PackageRef>) -> Components {
        r.and_then(|r| self.packages.get(&r.package)?.versions.get(&r.version))
            .map_or(Components::App, |v| v.components)
    }

    /// Whether `r` names an installed release whose service `r.service` is
    /// `bundle`: what an app recorded as running `r` must run.
    #[must_use]
    pub fn release_ships(&self, r: &PackageRef, bundle: &str) -> bool {
        let service = r.service.as_deref().unwrap_or_default();
        !r.compute
            && self
                .packages
                .get(&r.package)
                .and_then(|p| p.versions.get(&r.version))
                .and_then(|v| v.app_parts().get(service))
                .is_some_and(|h| h == bundle)
    }

    /// The one release whose service is `bundle`; none when no release ships it
    /// or several do, as releases may share a WASM.
    #[must_use]
    pub fn sole_release_of(&self, bundle: &str) -> Option<PackageRef> {
        let mut found = None;
        for (name, p) in &self.packages {
            for v in p.versions.values() {
                for (service, hash) in v.app_parts() {
                    if hash != bundle {
                        continue;
                    }
                    if found.is_some() {
                        return None;
                    }
                    found = Some(PackageRef {
                        package: name.clone(),
                        version: v.version.clone(),
                        service: (!service.is_empty()).then(|| service.clone()),
                        compute: false,
                    });
                }
            }
        }
        found
    }

    /// The release `app` runs: the one its record names, else, for a record
    /// without it, the only release shipping its bundle ([`Self::sole_release_of`]).
    #[must_use]
    pub fn running_release(&self, app: &AppRecord) -> Option<PackageRef> {
        app.release
            .clone()
            .or_else(|| self.sole_release_of(&app.bundle))
    }

    /// Whether a release ships `compute` (a compute bundle hash, or none) for the service
    /// whose app component is `bundle`. Releases may share an app component and differ in
    /// compute, so the caller names the compute hash and this checks the pair.
    #[must_use]
    pub fn ships_compute(&self, bundle: &str, compute: Option<&str>) -> bool {
        let Some(compute) = compute else {
            return true;
        };
        self.packages
            .values()
            .flat_map(|p| p.versions.values())
            .any(|v| {
                v.app_parts().iter().any(|(service, main)| {
                    main == bundle && v.compute.get(service).is_some_and(|c| c == compute)
                })
            })
    }

    /// The compute component `bundle` alone names: the one every release `who` sees ships with it,
    /// none if none does, an error if they differ.
    pub fn compute_of(&self, bundle: &str, who: &str) -> Result<Option<String>, &'static str> {
        let shipped: BTreeSet<Option<&String>> = self
            .packages
            .values()
            .flat_map(|p| p.versions.values())
            .filter(|v| self.sees_release(v, who))
            .flat_map(|v| {
                v.app_parts()
                    .iter()
                    .filter(|(_, main)| *main == bundle)
                    .map(|(service, _)| v.compute.get(service))
            })
            .collect();
        match shipped.len() {
            0 | 1 => Ok(shipped.into_iter().next().flatten().cloned()),
            _ => Err("releases that share this bundle ship different compute components; name the package and version"),
        }
    }

    /// Whether `who` may name the bundle `hash` for an app: it must be
    /// recorded, and if a package release ships it, one such release must be
    /// visible to `who`; the uploader of a raw bundle always names it, while a release's
    /// first installer, recorded as `uploaded_by` too, counts only beside a release recorded
    /// before `visibility`. An uploader is told from an installer by not being one of the
    /// installers of the release the row names.
    #[must_use]
    pub fn sees_bundle(&self, hash: &str, who: &str) -> bool {
        let Some(b) = self.bundles.get(hash) else {
            return false;
        };
        if self.is_compute_bundle(hash) {
            return false;
        }
        let mut shipped = self
            .packages
            .values()
            .flat_map(|p| p.versions.values())
            .filter(|v| v.app_parts().values().any(|h| h == hash))
            .peekable();
        let uploaded_raw = b.uploaded_by == who
            && !b.package.as_ref().is_some_and(|r| {
                self.packages
                    .get(&r.package)
                    .and_then(|p| p.versions.get(&r.version))
                    .is_some_and(|v| v.installed_by_any(who))
            });
        uploaded_raw
            || shipped.peek().is_none()
            || shipped.any(|v| {
                self.sees_release(v, who) || (v.visibility.is_none() && b.uploaded_by == who)
            })
    }

    /// [`Self::approved_releases`] for an app of `org`, before it exists.
    #[must_use]
    pub fn approved_for(&self, org: &str) -> Option<&BTreeSet<String>> {
        if self.governance().releases == Authority::Operator {
            let set = &self.operator_releases;
            return (!set.is_empty()).then_some(set);
        }
        let set = &self.orgs.get(org)?.approved_releases;
        (!set.is_empty()).then_some(set)
    }

    /// What `app`'s organization approved for apps signed by `signer` to call
    /// out to.
    #[must_use]
    pub fn outbound_approval(
        &self,
        app: &AppRecord,
        signer: &str,
    ) -> Option<&crate::types::OutboundApproval> {
        if self.governance().outbound == Authority::Operator {
            return self.operator_outbound.get(signer);
        }
        self.orgs.get(&app.org)?.outbound_approvals.get(signer)
    }

    /// Who turned egress off for `app`, if anyone: its organization
    /// ([`crate::types::Org::egress_off`]) before the instance itself
    /// ([`crate::types::AppRecord::egress_off`]). While it is off no outbound request
    /// leaves the cluster for the app, whatever was approved (`Node::outbound_for`).
    #[must_use]
    pub fn egress_off(&self, app: &AppRecord) -> Option<EgressOff> {
        if self.orgs.get(&app.org).is_some_and(|o| o.egress_off) {
            return Some(EgressOff::Org);
        }
        app.egress_off.then_some(EgressOff::Instance)
    }

    /// The release `app` runs, if installed: the one it records
    /// (`AppRecord::release`), else the one its bundle came out of (as
    /// `Node::release_of_app`).
    fn app_release<'a>(&'a self, app: &'a AppRecord) -> Option<&'a PackageRef> {
        let at = match &app.release {
            Some(at) => at,
            None => self.bundles.get(&app.bundle)?.package.as_ref()?,
        };
        let _installed = self.packages.get(&at.package)?.versions.get(&at.version)?;
        Some(at)
    }

    /// The apps an organization's upgrade run of `package` covers: its apps
    /// (with `team`, that team's, and with `cascade` the teams below it too)
    /// that run a release of `package`.
    #[must_use]
    pub fn org_run_apps(
        &self,
        org: &str,
        package: &str,
        team: Option<&str>,
        cascade: bool,
    ) -> Vec<AppId> {
        let Some(o) = self.orgs.get(org) else {
            return Vec::new();
        };
        let teams: Option<Vec<String>> = team.map(|t| {
            if cascade {
                o.subtree(t)
            } else {
                vec![t.to_owned()]
            }
        });
        self.apps
            .values()
            .filter(|a| a.org == org)
            .filter(|a| match &teams {
                None => true,
                Some(ts) => a.access.teams().into_iter().any(|t| ts.contains(t)),
            })
            .filter(|a| self.app_release(a).is_some_and(|r| r.package == package))
            .map(|a| a.app_id.clone())
            .collect()
    }

    /// The instances a publisher's run of `package` covers: every one whose
    /// bundle came out of `package` that `account` may upgrade (with
    /// `owner_approves`, those of the organizations it is an owner or admin of).
    #[must_use]
    pub fn published_run_apps(
        &self,
        package: &str,
        account: &str,
        by_release: bool,
        owner_approves: bool,
    ) -> Vec<AppId> {
        self.apps
            .values()
            .filter(|a| {
                let runs = if by_release {
                    // The release it runs, as `org_run_apps` and
                    // `run_may_move` judge it.
                    self.app_release(a).is_some_and(|r| r.package == package)
                } else {
                    // As an entry from before `by_release` applied: the
                    // package the bundle was first recorded from.
                    self.bundles
                        .get(&a.bundle)
                        .and_then(|b| b.package.as_ref())
                        .is_some_and(|at| at.package == package)
                };
                runs && self.may_upgrade(a, account, owner_approves)
            })
            .map(|a| a.app_id.clone())
            .collect()
    }

    /// The apps a new run of `org` covers, from the state and the method its
    /// consent names: a publisher's run every instance of its package the
    /// starter may upgrade, a retry the apps the run it repeats failed on,
    /// any other the organization's apps in its scope ([`Self::org_run_apps`]).
    fn upgrade_run_apps(
        &self,
        org: &str,
        run: &crate::types::UpgradeRun,
        consent: &Value,
        by_release: bool,
        owner_approves: bool,
    ) -> Vec<AppId> {
        let st = serde_json::from_value::<crate::proof::Statement>(consent.clone()).ok();
        match st.as_ref().map(|st| st.method.as_str()) {
            Some("__upgrade_unit") => st
                .and_then(|st| {
                    // The instance the consent names, if it runs the run's package, and
                    // those of its linked members the run gives a target.
                    let root = st.args()["app"].as_str()?.to_owned();
                    let a = self.apps.get(&root).filter(|a| a.org == org)?;
                    if !self
                        .app_release(a)
                        .is_some_and(|r| r.package == run.package)
                    {
                        return None;
                    }
                    let mut apps = vec![root];
                    apps.extend(
                        a.links
                            .values()
                            .filter(|m| run.targets.contains_key(*m) && self.apps.contains_key(*m))
                            .cloned(),
                    );
                    Some(apps)
                })
                .unwrap_or_default(),
            Some("__upgrade_published") => {
                self.published_run_apps(&run.package, &run.by, by_release, owner_approves)
            }
            Some("__upgrade_org_retry") => st
                .and_then(|st| {
                    let prev = st.args()["run"].as_str().map(str::to_owned)?;
                    let prev = self.orgs.get(org)?.upgrade_runs.get(&prev)?;
                    Some(
                        prev.failed
                            .keys()
                            .filter(|a| self.apps.contains_key(*a))
                            .cloned()
                            .collect(),
                    )
                })
                .unwrap_or_default(),
            _ => self.org_run_apps(org, &run.package, run.team.as_deref(), run.cascade),
        }
    }

    /// Whether run `r` may move `app` now: an app it names and has not
    /// upgraded or failed yet, still on a release of the run's package, to
    /// the run's version or a newer one.
    ///
    /// # Errors
    /// Why it may not: [`NOT_IN_RUN`], [`RUN_MOVED_APP`],
    /// [`RUN_OTHER_PACKAGE`] or [`RUN_OLDER_RELEASE`].
    pub fn run_may_move(
        &self,
        r: &crate::types::UpgradeRun,
        app: &AppRecord,
    ) -> Result<(), &'static str> {
        if !r.apps.contains(&app.app_id) {
            return Err(NOT_IN_RUN);
        }
        if r.done.contains(&app.app_id) || r.failed.contains_key(&app.app_id) {
            return Err(RUN_MOVED_APP);
        }
        let (package, version) = r.target_of(&app.app_id);
        let Some(at) = self.app_release(app).filter(|at| at.package == package) else {
            return Err(RUN_OTHER_PACKAGE);
        };
        if compare_versions(version, &at.version) == std::cmp::Ordering::Less {
            return Err(RUN_OLDER_RELEASE);
        }
        Ok(())
    }

    /// Note an app's upgrade on the organization-wide run it belongs to.
    fn note_run(&mut self, run: Option<&str>, app: &str, error: Option<Value>) {
        let Some(run) = run else {
            return;
        };
        for o in self.orgs.values_mut() {
            if let Some(r) = o.upgrade_runs.get_mut(run) {
                r.failed.remove(app);
                r.done.retain(|a| a != app);
                match error {
                    Some(e) => {
                        let _prev = r.failed.insert(app.to_owned(), e);
                    }
                    None => r.done.push(app.to_owned()),
                }
                return;
            }
        }
    }

    /// The rules this cluster was deployed with.
    #[must_use]
    pub fn governance(&self) -> Governance {
        self.governance.clone().unwrap_or_default()
    }

    /// Whether `account` may install apps (packages, raw bundles) at `now`,
    /// the install's own time: anyone where deployments are open; where they
    /// are granted, the operator, an account holding a live grant of its own,
    /// or one holding `MANAGE_APPLICATION` organization-wide in an
    /// organization holding one. Creating instances of a release one sees is
    /// not an install.
    #[must_use]
    pub fn may_deploy(&self, account: &str, now: u64) -> bool {
        if account == OPERATOR || self.governance().deployments == Deployments::Open {
            return true;
        }
        let own = Deployer::Account(account.to_owned()).key();
        if self.deployers.get(&own).is_some_and(|g| g.live(now)) {
            return true;
        }
        self.deployers.iter().any(|(key, g)| {
            g.live(now)
                && matches!(Deployer::from_key(key), Some(Deployer::Org(o)) if self.manages_apps(&o, account, None))
        })
    }

    /// Whether `account` may create, at `at`, an instance of `release` from no deployment's
    /// offer: whoever [may deploy](Self::may_deploy) then, and anyone for a release the
    /// operator installed. Having installed the release is not enough: a revoked grant ends it.
    #[must_use]
    pub fn creates_outside_offers(
        &self,
        account: &str,
        release: Option<&PackageRef>,
        at: u64,
    ) -> bool {
        self.may_deploy(account, at)
            || release.is_some_and(|r| {
                self.packages
                    .get(&r.package)
                    .and_then(|p| p.versions.get(&r.version))
                    .is_some_and(|v| v.installed_by_any(OPERATOR))
            })
    }

    /// Whether `actor` may move `app` to `bundle` (from `release`) at `at`: an instance
    /// from a deployment, or one moving within its package, may; else only into what
    /// `actor` [creates outside offers](Self::creates_outside_offers).
    #[must_use]
    pub fn upgrades_outside_offers(
        &self,
        app: &AppRecord,
        actor: &str,
        release: Option<&PackageRef>,
        bundle: &str,
        at: u64,
    ) -> bool {
        if app.deployment.is_some() {
            return true;
        }
        let from = app
            .release
            .as_ref()
            .map(|r| r.package.as_str())
            .or_else(|| self.package_of(app));
        let to = release.map(|r| r.package.as_str()).or_else(|| {
            let b = self.bundles.get(bundle)?;
            b.package.as_ref().map(|r| r.package.as_str())
        });
        (from.is_some() && from == to) || self.creates_outside_offers(actor, release, at)
    }

    /// The owner that grants deployers: the cluster's owner where governance
    /// says `deployers: owner` and the operator has named it; `None` while
    /// the operator grants them.
    #[must_use]
    pub fn deployers_owner(&self) -> Option<&Deployer> {
        if self.governance().deployers != Deployers::Owner {
            return None;
        }
        self.cluster_owner.as_ref()
    }

    /// Whether `account` acts for `owner`: it is that account, or holds
    /// `MANAGE_APPLICATION` organization-wide in that organization.
    #[must_use]
    pub fn acts_for(&self, owner: &Deployer, account: &str) -> bool {
        match owner {
            Deployer::Account(a) => a == account,
            Deployer::Org(o) => self.manages_apps(o, account, None),
        }
    }

    /// Whether the cluster's sign-up policy lets an account in with `invite`
    /// (none, an organization invite or a cluster sign-up invite), and how.
    /// `open` takes anyone, `invite` an invite of either kind, `closed` a
    /// cluster sign-up invite only. An invite presented is checked whatever
    /// the policy: unknown, expired or used up is a refusal, never an account
    /// created without it.
    ///
    /// # Errors
    /// The refusal ([`RejectionKind::SignUpRefused`]).
    pub fn admits(&self, invite: Option<&SignUpRedemption>) -> Result<Admission, Rejection> {
        self.admits_key(invite.map(|r| (r.invite_key.as_str(), r.now)))
    }

    /// [`Self::admits`] for an invite key presented at `now`.
    ///
    /// # Errors
    /// The refusal ([`RejectionKind::SignUpRefused`]).
    pub fn admits_key(&self, invite: Option<(&str, u64)>) -> Result<Admission, Rejection> {
        let admission = match invite {
            None => Admission::Open,
            Some((key, now)) => {
                if let Some(i) = self.sign_up_invites.get(key) {
                    if i.expires_at <= now || i.uses_left == 0 {
                        return Err(Rejection::sign_up("the invite has expired"));
                    }
                    Admission::SignUpInvite
                } else if let Some(i) = self.invites.get(key) {
                    if i.expires_at <= now || i.uses_left == 0 {
                        return Err(Rejection::sign_up("the invite has expired"));
                    }
                    if !self.orgs.contains_key(&i.org) {
                        return Err(Rejection::sign_up("the invite's organization is gone"));
                    }
                    Admission::OrgInvite
                } else {
                    return Err(Rejection::sign_up(
                        "this invite was used up, revoked or never existed",
                    ));
                }
            }
        };
        match (self.governance().sign_up, admission) {
            (SignUp::Open, _)
            | (SignUp::Invite, Admission::OrgInvite | Admission::SignUpInvite)
            | (SignUp::Closed, Admission::SignUpInvite) => Ok(admission),
            (SignUp::Invite, Admission::Open) => {
                Err(Rejection::sign_up("this cluster takes new accounts by invite only"))
            }
            (SignUp::Closed, _) => Err(
                Rejection::sign_up("this cluster takes only accounts its operator or owner pre-authorized with a sign-up invite"),
            ),
        }
    }

    /// Spend one use of an organization invite for `account`, which joins
    /// the organization (and the teams the invite names) at the invite's role,
    /// or keeps a higher one. Everything is checked before anything changes.
    ///
    /// With `free_when_unchanged`, a redemption that changes nothing for the
    /// account (a member already, at the invite's role or above, in every
    /// named team the organization still has) answers the organization and
    /// spends no use: an owner opening their own link leaves it for the person
    /// it was made for. Judged on the state alone, so every replica agrees.
    fn redeem_org_invite(
        &mut self,
        invite_key: &str,
        account: &str,
        now: u64,
        free_when_unchanged: bool,
        clamp_grants: bool,
    ) -> Result<OrgId, Rejection> {
        let inv = self.invites.get(invite_key).ok_or(INVITE_GONE)?;
        if inv.expires_at <= now || inv.uses_left == 0 {
            return Err(INVITE_EXPIRED.into());
        }
        // The flag, or a cluster version that brings the rule to every node:
        // read from state at this log position, so replicas agree.
        let clamp =
            clamp_grants || self.cluster_brings(cluster_version::Feature::InviteCapabilities);
        let all_apps = inv.all_apps && (!clamp || self.is_org_admin(&inv.org, &inv.created_by));
        let o = self.orgs.get(&inv.org).ok_or("no such organization")?;
        // From the logged version that brings it, an invite gives only what its
        // creator could still give: a creator removed (or unknown), demoted below
        // the invite's role, or no longer seeing a team it names gives nothing,
        // so a removed admin's invite does not bring them back as one.
        if self.cluster_brings(cluster_version::Feature::InviteCreatorStanding)
            && !Self::invite_creator_stands(o, inv)
        {
            return Err(INVITE_CREATOR_LACKS);
        }
        let unchanged = o.members.get(account).is_some_and(|r| *r >= inv.role)
            && (!all_apps || o.all_apps.contains(account))
            && inv.teams.iter().all(|t| {
                o.teams
                    .get(t)
                    .is_none_or(|team| team.members.contains_key(account))
            });
        if free_when_unchanged && unchanged {
            return Ok(inv.org.clone());
        }
        let inv = self.invites.get_mut(invite_key).ok_or(INVITE_GONE)?;
        inv.uses_left -= 1;
        let inv = inv.clone();
        if inv.uses_left == 0 {
            let _prev = self.invites.remove(invite_key);
        }
        // What its creator may grant now: a demoted, stripped or removed
        // creator's invite hands out no more than they could.
        let granted = self.orgs.get(&inv.org).and_then(|o| {
            o.role(&inv.created_by)?;
            inv.capabilities.map(|c| c & o.org_caps(&inv.created_by))
        });
        let o = self.orgs.get_mut(&inv.org).ok_or("no such organization")?;
        let role = o
            .members
            .get(account)
            .copied()
            .map_or(inv.role, |r| r.max(inv.role));
        let new_member = !o.members.contains_key(account);
        let _prev = o.members.insert(account.to_owned(), role);
        if let (Some(bits), true) = (granted, new_member) {
            let _prev = o.capabilities.insert(account.to_owned(), bits);
        }
        if all_apps {
            let _new = o.all_apps.insert(account.to_owned());
        }
        for t in &inv.teams {
            if let Some(team) = o.teams.get_mut(t) {
                let _prev = team.members.entry(account.to_owned()).or_insert(false);
            }
        }
        Ok(inv.org)
    }

    /// Whether `inv`'s creator could still make it in `o`
    /// ([`cluster_version::Feature::InviteCreatorStanding`]): a member at or
    /// above its role and, below admin, seeing every team it names that still
    /// exists, as [`SystemCmd::CreateInvite`] judged them when it was made.
    fn invite_creator_stands(o: &Org, inv: &Invite) -> bool {
        let Some(role) = o.role(&inv.created_by) else {
            return false;
        };
        if inv.role > role {
            return false;
        }
        role >= OrgRole::Admin
            || inv
                .teams
                .iter()
                .filter(|t| o.teams.contains_key(*t))
                .all(|t| o.sees_team(&inv.created_by, t))
    }

    /// Spend one use of a cluster sign-up invite; the last use removes it.
    fn spend_sign_up_invite(&mut self, invite_key: &str) {
        let Some(i) = self.sign_up_invites.get_mut(invite_key) else {
            return;
        };
        i.uses_left = i.uses_left.saturating_sub(1);
        if i.uses_left == 0 {
            let _gone = self.sign_up_invites.remove(invite_key);
        }
    }

    /// Whether `actor` runs the cluster's sign-up now (its policy and its
    /// sign-up invites): the operator until the cluster's owner is named
    /// (where governance says `deployers: owner`), then whoever acts for the
    /// owner, never the operator.
    ///
    /// # Errors
    /// Someone else.
    pub fn runs_sign_up(&self, actor: &str) -> Result<(), &'static str> {
        match self.deployers_owner() {
            None if actor == OPERATOR => Ok(()),
            None => Err("the operator runs this cluster's sign-up"),
            Some(owner) if actor != OPERATOR && self.acts_for(owner, actor) => Ok(()),
            Some(_) => Err("this cluster's owner runs its sign-up"),
        }
    }

    /// Whether `deployer` is an account or organization that exists.
    fn exists(&self, deployer: &Deployer) -> bool {
        match deployer {
            Deployer::Account(a) => self.accounts.contains_key(a),
            Deployer::Org(o) => self.orgs.contains_key(o),
        }
    }

    /// Whether `actor` may grant or revoke deployers now: the operator while
    /// it grants them, else whoever acts for the cluster's owner.
    fn grants_deployers(&self, actor: &str) -> Result<(), &'static str> {
        match self.deployers_owner() {
            None if actor == OPERATOR => Ok(()),
            None => Err("the operator grants this cluster's deployers"),
            Some(owner) if actor != OPERATOR && self.acts_for(owner, actor) => Ok(()),
            Some(_) => Err("this cluster's owner grants its deployers"),
        }
    }

    /// Whether some release of `package` ships the WASM `bundle`.
    #[must_use]
    pub fn ships(&self, package: &str, bundle: &str) -> bool {
        self.packages.get(package).is_some_and(|p| {
            p.versions
                .values()
                .any(|v| v.app_parts().values().any(|h| h == bundle))
        })
    }

    /// The release `offer` names, if it is installed.
    #[must_use]
    pub fn offered_release(&self, offer: &Offer) -> Option<&PackageVersion> {
        self.packages
            .get(&offer.package)?
            .versions
            .get(&offer.version)
    }

    /// Whether `d`'s offer of `v` still serves: someone who acts for its owner may use `v`.
    /// A release every member of the owning organization may use is answered by whether one
    /// of them acts for it; only a release recorded before `visibility` is judged member by
    /// member.
    #[must_use]
    pub fn offer_serves(&self, d: &Deployment, v: &PackageVersion) -> bool {
        let candidates: Vec<&str> = match &d.owner {
            Deployer::Account(a) => vec![a],
            Deployer::Org(o) => {
                let mut members = self.orgs.get(o).into_iter().flat_map(|o| o.members.keys());
                let for_all = v.installed_by_any(OPERATOR)
                    || v.visibility == Some(Visibility::Public)
                    || (v.visibility == Some(Visibility::Private) && v.orgs.contains(o));
                if for_all {
                    return members.any(|m| self.acts_for(&d.owner, m));
                }
                members.map(String::as_str).collect()
            }
        };
        candidates
            .into_iter()
            .any(|m| self.acts_for(&d.owner, m) && self.sees_release(v, m))
    }

    /// Whether `d` offers a release that ships `bundle`: what an instance
    /// created from it must run.
    #[must_use]
    pub fn offers_bundle(&self, d: &Deployment, bundle: &str) -> bool {
        d.apps.values().any(|o| {
            self.offered_release(o)
                .is_some_and(|v| v.app_parts().values().any(|h| h == bundle))
        })
    }

    /// Whether some deployment offers `package@version`.
    #[must_use]
    pub fn offered(&self, package: &str, version: &str) -> bool {
        self.deployments
            .values()
            .flat_map(|d| d.apps.values())
            .any(|o| o.package == package && o.version == version)
    }

    /// Whether a release `d` offers declares secret `name`
    /// (`PackageVersion::secrets`).
    #[must_use]
    pub fn deployment_declares(&self, d: &Deployment, name: &str) -> bool {
        d.apps.values().any(|o| {
            self.offered_release(o)
                .is_some_and(|v| v.secrets.contains_key(name))
        })
    }

    /// The installed releases `v` of `package` references (`PackageVersion::uses`), each
    /// with its package: every installed release of a referenced package inside the
    /// reference's range (the one its lock pinned among them), which is what an app it
    /// creates may run, then and after an upgrade within the range. A range that does not
    /// parse reaches none, nor does a release no one acting for `d`'s owner may use
    /// ([`Self::offer_serves`]) unless `package`'s signer signs it ([`Self::same_signer`]).
    pub fn used_releases<'a>(
        &'a self,
        d: &'a Deployment,
        package: &'a str,
        v: &'a PackageVersion,
    ) -> impl Iterator<Item = (&'a str, &'a PackageVersion)> + 'a {
        // Gated so earlier log entries replay as they applied.
        let private = self.cluster_brings(cluster_version::Feature::PrivateReleases);
        v.uses.values().flat_map(move |u| {
            let range = semver::VersionReq::parse(&u.range).ok();
            let same_signer = self.same_signer(package, &u.package);
            self.packages
                .get(&u.package)
                .into_iter()
                .flat_map(|p| p.versions.values())
                .filter(move |r| {
                    range.as_ref().is_some_and(|range| {
                        semver::Version::parse(&r.version).is_ok_and(|ver| range.matches(&ver))
                    }) && (!private || same_signer || self.offer_serves(d, r))
                })
                .map(move |r| (u.package.as_str(), r))
        })
    }

    /// Whether packages `a` and `b` are pinned to one signer: a publisher composing its own
    /// packages, whose unit takes a member of either whoever may use it (`unit_members`).
    #[must_use]
    pub fn same_signer(&self, a: &str, b: &str) -> bool {
        matches!(
            (self.packages.get(a), self.packages.get(b)),
            (Some(a), Some(b)) if a.signer_id == b.signer_id
        )
    }

    /// Whether a release that a release `d` offers references
    /// ([`Self::used_releases`]) declares secret `name`: the apps it creates read the
    /// deployment's value of it, where they carry the deployment.
    #[must_use]
    pub fn deployment_uses_declare(&self, d: &Deployment, name: &str) -> bool {
        d.apps.values().any(|o| {
            self.offered_release(o).is_some_and(|v| {
                self.used_releases(d, &o.package, v)
                    .any(|(_, r)| r.secrets.contains_key(name))
            })
        })
    }

    /// Each secret name a release reachable from `d` declares (an offered one, or one an
    /// offered release references), with the offered apps that reach it and the
    /// releases (`package`, `version`) that declare it.
    #[must_use]
    pub fn deployment_declarers(&self, d: &Deployment) -> BTreeMap<String, SecretDeclarers> {
        let mut out: BTreeMap<String, SecretDeclarers> = BTreeMap::new();
        for (app, o) in &d.apps {
            let Some(v) = self.offered_release(o) else {
                continue;
            };
            let reached = std::iter::once((o.package.as_str(), v))
                .chain(self.used_releases(d, &o.package, v));
            for (package, r) in reached {
                for name in r.secrets.keys() {
                    let row = out.entry(name.clone()).or_default();
                    let _new = row.apps.insert(app.clone());
                    let _new = row
                        .declared_by
                        .insert((package.to_owned(), r.version.clone()));
                }
            }
        }
        out
    }

    /// Whether `d` offers a release of `package`, any version, or a release it
    /// offers references `package` ([`Self::used_releases`]: the apps of its
    /// unit run it): the code its deployment's secrets may reach. A package's
    /// signer is pinned, so only its publisher signs another release of it;
    /// code of any other package (an instance's owner's own, say) never gets
    /// the deployment's values.
    #[must_use]
    pub fn deployment_covers(&self, d: &Deployment, package: &str) -> bool {
        d.apps.values().any(|o| {
            o.package == package
                || self.offered_release(o).is_some_and(|v| {
                    self.used_releases(d, &o.package, v)
                        .any(|(p, _)| p == package)
                })
        })
    }

    /// Whether moving `app` to `release` keeps it on its deployment's offer:
    /// only the deployment's managers decide which release its instances run
    /// (`OfferApp`), and the instance's owner accepts it or not. An instance
    /// from no deployment may go anywhere; one from a deployment only to the
    /// exact release the deployment offers for its package (the package it
    /// runs), never a raw bundle, another version or another package.
    ///
    /// # Errors
    /// [`OUTSIDE_DEPLOYMENT`] when the move leaves the offer.
    pub fn within_deployment(
        &self,
        app: &AppRecord,
        release: Option<&PackageRef>,
        run: Option<&str>,
    ) -> Result<(), Rejection> {
        let Some(id) = app.deployment.as_deref() else {
            return Ok(());
        };
        let d = self.deployments.get(id).ok_or(OUTSIDE_DEPLOYMENT)?;
        let to = release.ok_or(OUTSIDE_DEPLOYMENT)?;
        // Only an offer that still serves, from the version that brings private releases, so
        // earlier entries replay as they applied.
        let serves = |o: &Offer| {
            !self.cluster_brings(cluster_version::Feature::PrivateReleases)
                || self
                    .offered_release(o)
                    .is_some_and(|v| self.offer_serves(d, v))
        };
        if app
            .release
            .as_ref()
            .is_some_and(|r| r.package != to.package)
        {
            return Err(OUTSIDE_DEPLOYMENT);
        }
        // A unit's member carries its instance's deployment but runs a package
        // the deployment does not offer: it moves only within its own package
        // (checked above), and only with its unit, in a run that takes the
        // unit's instance to the release the deployment offers (each member's
        // target comes from that release's `uses`, `UpgradeRun::target_of`).
        // Never alone, and never in a run to a release the developer did not
        // offer, which would move the members though the instance stays.
        if let Some(parent) = self.unit_parent(&app.app_id) {
            let unit_run = run
                .and_then(|id| self.orgs.values().find_map(|o| o.upgrade_runs.get(id)))
                .filter(|r| {
                    r.targets.contains_key(&app.app_id)
                        && self
                            .apps
                            .get(parent)
                            .and_then(|p| p.release.as_ref())
                            .is_some_and(|p| p.package == r.package)
                        && d.apps
                            .values()
                            .any(|o| o.package == r.package && o.version == r.version && serves(o))
                });
            return unit_run.map(|_| ()).ok_or(OUTSIDE_DEPLOYMENT);
        }
        if !d
            .apps
            .values()
            .any(|o| o.package == to.package && o.version == to.version && serves(o))
        {
            return Err(OUTSIDE_DEPLOYMENT);
        }
        Ok(())
    }

    /// The secret values of the deployment `app_id` was created from, by name,
    /// for a run of `package`: what the outbound gate attaches where the
    /// release binds `NAME` and the instance set no value of its own. `None` for an instance from no
    /// deployment, one deleted since, or a run of a package the deployment does
    /// not offer or reference ([`Self::deployment_covers`]): its managers set
    /// the values for the code they ship, and no other code gets them, whatever the record
    /// says it runs.
    #[must_use]
    pub fn deployment_secrets_of(
        &self,
        app_id: &str,
        package: &str,
    ) -> Option<&crate::component::InstanceSecrets> {
        let id = self.apps.get(app_id)?.deployment.as_deref()?;
        let d = self.deployments.get(id)?;
        if !self.deployment_covers(d, package) {
            return None;
        }
        self.deployment_secrets.get(id)
    }

    /// Whether the logged cluster version brings `feature` to every node: a
    /// rule judged from state at this log position, so replicas agree.
    #[must_use]
    pub fn cluster_brings(&self, feature: cluster_version::Feature) -> bool {
        self.cluster_version
            .as_deref()
            .is_some_and(|v| compare_versions(v, feature.since()) != std::cmp::Ordering::Less)
    }

    /// Whether organizations are ordinary at this log position: the logged cluster version
    /// brings [`cluster_version::Feature::OrdinaryOrgs`]. From then on a sign-up makes no
    /// organization, no personal one is made, and one made before is treated as any other
    /// (it counts against `maxOrgsPerAccount`, its apps go only to its members), except that
    /// its instances still reach the accounts they name ([`Self::may_reach`]).
    /// Before it, every entry applies as it was logged.
    #[must_use]
    pub fn ordinary_orgs(&self) -> bool {
        self.cluster_brings(cluster_version::Feature::OrdinaryOrgs)
    }

    /// Whether guests are on at this log position: the logged cluster version brings
    /// [`cluster_version::Feature::Guests`]. Before it, an instance's listed accounts
    /// reach it only as members (or in a personal organization), as they did.
    #[must_use]
    pub fn guests_on(&self) -> bool {
        self.cluster_brings(cluster_version::Feature::Guests)
    }

    /// Whether `app` takes guests: the release it runs allows them (`guests: allowed` in
    /// its signed manifest, recorded on the release; a release recorded without it, a raw
    /// bundle, or none, does not) and its organization does not turn them off
    /// ([`crate::types::Org::allow_guests`]). The stricter switch wins.
    #[must_use]
    pub fn guests_allowed(&self, app: &AppRecord) -> bool {
        let release = app.release.as_ref().and_then(|r| {
            self.packages
                .get(&r.package)
                .and_then(|p| p.versions.get(&r.version))
        });
        let developer =
            release.and_then(|v| v.guests) == Some(calimero_bundle::GuestPolicy::Allowed);
        developer && self.orgs.get(&app.org).is_some_and(|o| o.allow_guests)
    }

    /// The instance whose guest list judges `app`: the top of its unit (the app whose
    /// `links` hold it, transitively, at most [`MAX_UNIT_LINKS_UP`] steps), else itself.
    #[must_use]
    pub fn guest_holder<'a>(&'a self, app: &'a AppRecord) -> &'a AppRecord {
        let mut at = app;
        for _ in 0..MAX_UNIT_LINKS_UP {
            match self.unit_parent(&at.app_id).and_then(|p| self.apps.get(p)) {
                Some(parent) => at = parent,
                None => break,
            }
        }
        at
    }

    /// Whether `account` reaches `app` as its guest ([`cluster_version::Feature::Guests`]):
    /// listed on the instance that holds its guest list ([`Self::guest_holder`]: `app`, or
    /// the instance whose unit created it) and that instance takes guests
    /// ([`Self::guests_allowed`]). Judged on the state of the moment, so a guest taken off
    /// the list, or a switch turned off, stops reaching it at once.
    #[must_use]
    pub fn guest_reaches(&self, app: &AppRecord, account: &str) -> bool {
        if !self.guests_on() {
            return false;
        }
        let holder = self.guest_holder(app);
        holder.access.names(account) && self.guests_allowed(holder)
    }

    /// Whether `account` is a guest of `app` and not a member of its organization: it
    /// reaches the app only as a guest ([`Self::guest_reaches`]).
    #[must_use]
    pub fn is_guest(&self, app: &AppRecord, account: &str) -> bool {
        self.orgs
            .get(&app.org)
            .and_then(|o| o.role(account))
            .is_none()
            && self.guest_reaches(app, account)
    }

    /// The scope of `account`'s grant on `app` ([`crate::types::Grant::Account`]): on `app`
    /// itself, else on the instance whose unit made it ([`Self::guest_holder`]), as a guest's
    /// reach is judged. The app reads it (`env::grant_scope`); the node never does.
    #[must_use]
    pub fn grant_scope(&self, app: &AppRecord, account: &str) -> Option<Value> {
        app.access
            .scope_of(account)
            .or_else(|| self.guest_holder(app).access.scope_of(account))
            .cloned()
    }

    /// The public link under `link_key` and the instance whose grants hold it
    /// ([`crate::types::Grant::Link`]). Links live in their instances' grant lists, so this
    /// looks through the instances: a visitor's request, and making a link, are its only
    /// callers.
    #[must_use]
    pub fn public_link(&self, link_key: &str) -> Option<(&AppRecord, &crate::types::LinkGrant)> {
        self.apps
            .values()
            .find_map(|a| a.access.link(link_key).map(|l| (a, l)))
    }

    /// Public links the cluster holds ([`MAX_PUBLIC_LINKS`]).
    #[must_use]
    pub fn public_link_count(&self) -> usize {
        self.apps.values().map(|a| a.access.links().len()).sum()
    }

    /// The public link under `link_key`, with its instance, while it admits visitors at
    /// `now`: its instance holds it, it has not expired, the logged version brings public
    /// links and the instance's organization allows them
    /// ([`crate::types::Org::allow_public_links`]).
    #[must_use]
    pub fn public_link_admits(
        &self,
        link_key: &str,
        now: u64,
    ) -> Option<(&AppRecord, &crate::types::LinkGrant)> {
        if !self.cluster_brings(cluster_version::Feature::PublicLinks) {
            return None;
        }
        let (app, link) = self.public_link(link_key)?;
        if link.expires_at.is_some_and(|at| at <= now) {
            return None;
        }
        self.orgs
            .get(&app.org)
            .is_some_and(|o| o.allow_public_links)
            .then_some((app, link))
    }

    /// Whether `actor` may change `app`'s grants (who it names, its invites): whoever may
    /// change its access ([`Self::owner_acts`], or [`Self::may_set_access`] keeping its member
    /// grants: a team's leads on their team's instance, else who manages the organization's
    /// apps).
    #[must_use]
    pub fn manages_grants(&self, app: &AppRecord, actor: &str) -> bool {
        self.owner_acts(app, actor) || self.may_set_access(app, &app.access, actor)
    }

    /// Whether `org` is kept apart as a personal organization: flagged so, and only
    /// before [`Self::ordinary_orgs`].
    #[must_use]
    pub fn acts_personal(&self, org: &crate::types::Org) -> bool {
        org.personal && !self.ordinary_orgs()
    }

    /// The organizations `account` may create an app in (`member` or above), by id.
    #[must_use]
    pub fn orgs_of(&self, account: &str) -> Vec<&OrgId> {
        self.orgs
            .iter()
            .filter(|(_, o)| o.role(account).is_some_and(|r| r >= OrgRole::Member))
            .map(|(id, _)| id)
            .collect()
    }

    /// Whether `actor` may change `d`: [`Self::acts_for`] its owner.
    fn manages_deployment(&self, d: &Deployment, actor: &str) -> Result<(), &'static str> {
        if actor == OPERATOR || !self.acts_for(&d.owner, actor) {
            return Err("only the deployment's owner, or MANAGE_APPLICATION in its organization, may manage it");
        }
        Ok(())
    }

    /// Who holds the key that opens `app`'s recovery-disk backups: whoever
    /// upgrades it. The operator where governance gives it app upgrades;
    /// else the app's owner: its organization (every app has one, its owner's
    /// personal organization at the least). A package that declares `upgrades: publisher` still
    /// gives the key to the owner: a publisher ships code into attested
    /// nodes and never sees an instance's data, which the key would open.
    #[must_use]
    pub fn backup_holder(&self, app: &AppRecord) -> BackupHolder {
        if self.governance().app_upgrades == Authority::Operator {
            return BackupHolder::Operator;
        }
        BackupHolder::Org(app.org.clone())
    }

    /// The apps whose backups open with `holder`'s key.
    pub fn apps_of_holder<'a>(
        &'a self,
        holder: &'a BackupHolder,
    ) -> impl Iterator<Item = &'a AppRecord> + 'a {
        self.apps
            .values()
            .filter(move |app| self.backup_holder(app) == *holder)
    }

    /// `holder`'s [`HolderSlice`]: an organization with its apps and the
    /// accounts of its members and its apps' owners, or an account with its
    /// personal apps; the bundles and package releases they run. The
    /// operator's is the whole state (a `system` recovery file), not a slice.
    #[must_use]
    pub fn holder_slice(&self, holder: &BackupHolder) -> HolderSlice {
        let apps: BTreeMap<AppId, AppRecord> = self
            .apps_of_holder(holder)
            .map(|a| (a.app_id.clone(), a.clone()))
            .collect();
        let org = match holder {
            BackupHolder::Org(org) => self.orgs.get(org).cloned(),
            _ => None,
        };
        let mut people: BTreeSet<&str> = apps.values().map(|a| a.owner.as_str()).collect();
        match holder {
            BackupHolder::Account(account) => {
                let _new = people.insert(account);
            }
            BackupHolder::Org(_) => {
                if let Some(o) = &org {
                    people.extend(o.members.keys().map(String::as_str));
                }
            }
            BackupHolder::Operator => {}
        }
        let accounts = people
            .into_iter()
            .filter_map(|id| self.accounts.get(id))
            .map(|a| SliceAccount {
                account_id: a.account_id.clone(),
                name: a.name.clone(),
                created_at: a.created_at,
                passkeys: a.passkeys.clone(),
            })
            .collect();
        let hashes: BTreeSet<&String> = apps
            .values()
            .flat_map(|a| std::iter::once(&a.bundle).chain(a.compute_bundle.as_ref()))
            .collect();
        let bundles: BTreeMap<String, BundleRecord> = hashes
            .into_iter()
            .filter_map(|h| self.bundles.get(h).map(|b| (h.clone(), b.clone())))
            .collect();
        let packages = bundles
            .values()
            .filter_map(|b| b.package.as_ref())
            .map(|p| p.package.as_str())
            .chain(
                apps.values()
                    .filter_map(|a| a.release.as_ref().map(|r| r.package.as_str())),
            )
            .filter_map(|p| self.packages.get(p).map(|r| (p.to_owned(), r.clone())))
            .collect();
        HolderSlice {
            holder: holder.clone(),
            org,
            accounts,
            apps,
            bundles,
            packages,
        }
    }

    /// Whether `actor` may set `holder`'s backup key: the operator its own
    /// where it upgrades apps; where owners do, an organization's owner and
    /// admins (those who approve its upgrades, [`Self::approves_upgrades`]:
    /// a backup opens every app's state, as new code reads it), and an account
    /// its own. Below the logged version that brings
    /// [`cluster_version::Feature::BackupKeyApprovers`], an entry applies as it
    /// did, by org-wide `MANAGE_APPLICATION`.
    #[must_use]
    pub fn may_set_backup_key(&self, holder: &BackupHolder, actor: &str) -> bool {
        let by_operator = self.governance().app_upgrades == Authority::Operator;
        match holder {
            BackupHolder::Operator => by_operator && actor == OPERATOR,
            BackupHolder::Org(org)
                if self.cluster_brings(cluster_version::Feature::BackupKeyApprovers) =>
            {
                !by_operator && self.approves_upgrades(org, actor)
            }
            BackupHolder::Org(org) => !by_operator && self.manages_apps(org, actor, None),
            BackupHolder::Account(account) => {
                !by_operator && actor == account && self.accounts.contains_key(account)
            }
        }
    }

    /// Who upgrades an app governed by `release`
    /// ([`Self::governing_release`]): its declaration; a raw upload is its
    /// owner's.
    #[must_use]
    pub fn upgrades_of(&self, release: Option<&PackageRef>) -> UpgradeAuthority {
        release
            .and_then(|r| self.packages.get(&r.package)?.versions.get(&r.version))
            .map_or(UpgradeAuthority::Owner, |v| v.upgrades)
    }

    /// The publisher that upgrades an app governed by `release`: only when
    /// it declares `publisher` and its key is bound to an organization.
    #[must_use]
    pub fn upgraded_by_publisher(&self, release: Option<&PackageRef>) -> Option<&OrgId> {
        if self.upgrades_of(release).is_owner() {
            return None;
        }
        self.publisher_of(release)
    }

    /// Whether moving an app governed by release `from` to one governed by
    /// `to` keeps upgrades where they may go: never from an `owner` release
    /// to a `publisher` one.
    #[must_use]
    pub fn upgrade_keeps_authority(
        &self,
        from: Option<&PackageRef>,
        to: Option<&PackageRef>,
    ) -> bool {
        !(self.upgrades_of(from).is_owner() && !self.upgrades_of(to).is_owner())
    }

    /// Whether `app`'s package is upgraded by its publisher while no
    /// organization has bound the publisher's key: then nobody upgrades it.
    #[must_use]
    pub fn upgrades_unbound(&self, app: &AppRecord) -> bool {
        let r = self.governing_release(app);
        !self.upgrades_of(r).is_owner() && self.publisher_of(r).is_none()
    }

    /// Whether upgrade key `key` may move `app_id` to `bundle` at `at`: the
    /// key upgrades that app, has not expired, and `bundle` is a service of
    /// an installed release of the key's package, whose pinned signer is
    /// still the one the key names. Who may upgrade the app, deadlines and
    /// authority are the upgrade's own rules, judged as for a passkey.
    ///
    /// # Errors
    /// The key is not an upgrade key, or not for this app or this release.
    pub fn key_may_upgrade<'k>(
        &self,
        key: &'k ApiKeyRecord,
        app_id: &str,
        bundle: &str,
        at: u64,
    ) -> Result<&'k crate::types::UpgradeScope, &'static str> {
        let scope = key
            .upgrades
            .as_ref()
            .ok_or("this API key does not upgrade apps")?;
        if scope.app != app_id {
            return Err("this API key upgrades another app");
        }
        if key.expires_at <= at {
            return Err("this API key had expired");
        }
        let package = self
            .packages
            .get(&scope.package)
            .filter(|p| p.signer_id == scope.signer)
            .ok_or("this API key's package is not installed under the signer it names")?;
        if !package
            .versions
            .values()
            .any(|v| v.app_parts().values().any(|h| h == bundle))
        {
            return Err("this API key upgrades only to signed releases of its package");
        }
        Ok(scope)
    }

    /// Whether `record` may be made with upgrade scope `scope`: nothing
    /// else scoped, an app its account may upgrade now, running a release
    /// of the package, whose pinned signer the scope names.
    ///
    /// # Errors
    /// The scope names another app, package or signer, or the account may
    /// not upgrade the app.
    pub fn upgrade_key_fits(
        &self,
        record: &ApiKeyRecord,
        scope: &crate::types::UpgradeScope,
        owner_approves: bool,
    ) -> Result<(), &'static str> {
        self.upgrade_key_fits_as_logged(record, scope, OwnerRule::CURRENT, owner_approves)
    }

    /// [`Self::upgrade_key_fits`], with the app's owner judged as
    /// [`Self::may_upgrade_as_logged`] does.
    fn upgrade_key_fits_as_logged(
        &self,
        record: &ApiKeyRecord,
        scope: &crate::types::UpgradeScope,
        rule: OwnerRule,
        owner_approves: bool,
    ) -> Result<(), &'static str> {
        if record.apps.is_some()
            || record.methods.is_some()
            || record.read_only
            || record.allow_confirm
        {
            return Err("an upgrade key has no other scope");
        }
        let app = self.apps.get(&scope.app).ok_or("no such app")?;
        if !self.may_upgrade_as_logged(app, &record.account, rule, owner_approves) {
            return Err(if owner_approves {
                NOT_AN_UPGRADE_APPROVER
            } else {
                "an upgrade key needs an account that may upgrade the app"
            });
        }
        let package = self.packages.get(&scope.package).ok_or("no such package")?;
        if package.signer_id != scope.signer {
            return Err("the package is signed by another key");
        }
        let runs = package
            .versions
            .values()
            .any(|v| v.app_parts().values().any(|h| *h == app.bundle));
        if !runs {
            return Err("the app does not run a release of that package");
        }
        Ok(())
    }

    /// Whether `account` may upgrade `app`.
    ///
    /// With `owner_approves` (the rule from
    /// [`cluster_version::Feature::OwnerApprovedUpgrades`] on): an owner or admin of the
    /// app's organization ([`Self::approves_upgrades`]), whatever its package declares,
    /// and nobody else: not the app's creator as a member, a team's lead or a holder of
    /// `MANAGE_APPLICATION`, nor the publisher's organization for an instance elsewhere.
    ///
    /// Without it, as before: for a `publisher` package whose key is bound to an
    /// organization, whoever holds `MANAGE_APPLICATION` in that organization, org-wide,
    /// and nobody else; while the key is unbound, nobody. Otherwise (`owner`): its owner
    /// (of an org app, while a member: [`Self::owner_acts`]), or, for an org app, whoever
    /// [manages the organization's apps](Self::manages_apps) on the app's team.
    #[must_use]
    pub fn may_upgrade(&self, app: &AppRecord, account: &str, owner_approves: bool) -> bool {
        self.may_upgrade_as_logged(app, account, OwnerRule::CURRENT, owner_approves)
    }

    /// Whether `account` approves the upgrades of `org`'s apps under the rule
    /// [`cluster_version::Feature::OwnerApprovedUpgrades`] brings: its owner or an admin
    /// (`OrgRole::Admin` or above). Every app of `org` is theirs to upgrade, on any team,
    /// of any package.
    #[must_use]
    pub fn approves_upgrades(&self, org: &str, account: &str) -> bool {
        self.is_org_admin(org, account)
    }

    /// Whether the rule [`cluster_version::Feature::OwnerApprovedUpgrades`] brings
    /// judges an upgrade now: an entry that says so (`owner_approves`), or the logged
    /// cluster version, which every node of the cluster applies alike.
    #[must_use]
    pub fn owner_approves(&self, flagged: bool) -> bool {
        flagged || self.cluster_brings(cluster_version::Feature::OwnerApprovedUpgrades)
    }

    /// Whether an update on its own is refused where it would move the unit's members
    /// ([`cluster_version::Feature::AutoUpdateMembersStay`]): an entry that says so, or the
    /// logged cluster version.
    #[must_use]
    pub fn members_stay(&self, flagged: bool) -> bool {
        flagged || self.cluster_brings(cluster_version::Feature::AutoUpdateMembersStay)
    }

    /// [`Self::may_upgrade`], with the owner judged as the release that
    /// logged the command did ([`OwnerRule`]).
    #[must_use]
    pub fn may_upgrade_as_logged(
        &self,
        app: &AppRecord,
        account: &str,
        rule: OwnerRule,
        owner_approves: bool,
    ) -> bool {
        if owner_approves {
            return self.approves_upgrades(&app.org, account);
        }
        let release = self.governing_release(app);
        if !self.upgrades_of(release).is_owner() {
            return self
                .upgraded_by_publisher(release)
                .is_some_and(|p| self.manages_apps(p, account, None));
        }
        self.owner_acts_as_logged(app, account, rule)
            || self.manages_access(&app.org, account, &app.access)
    }

    /// Whether `actor` manages the apps an instance with `access` is: on each team it admits
    /// (every team grant), else organization-wide
    /// ([`Self::manages_apps`]).
    #[must_use]
    pub fn manages_access(&self, org: &str, actor: &str, access: &Access) -> bool {
        let teams = access.teams();
        if teams.is_empty() {
            return self.manages_apps(org, actor, None);
        }
        teams
            .into_iter()
            .all(|t| self.manages_apps(org, actor, Some(t)))
    }

    /// The package whose release `app` runs; `None` for a raw upload.
    #[must_use]
    pub fn package_of(&self, app: &AppRecord) -> Option<&str> {
        self.bundles
            .get(&app.bundle)?
            .package
            .as_ref()
            .map(|r| r.package.as_str())
    }

    /// The package `app` is an instance of, as a [`SystemCmd::ScopeApps`]
    /// judges it: with `by_release`, the release the instance records
    /// (`AppRecord::release`), since two packages may ship one component and
    /// its bundle's record names only the first; else, and for a record that
    /// names no release, [`Self::package_of`].
    fn instance_package<'a>(&'a self, app: &'a AppRecord, by_release: bool) -> Option<&'a str> {
        match &app.release {
            Some(r) if by_release => Some(r.package.as_str()),
            _ => self.package_of(app),
        }
    }

    /// Whether `actor`, not `app`'s owner, may change its access to `to`
    /// (G1): someone who manages the organization's apps (`MANAGE_APPLICATION`,
    /// [`Self::manages_apps`]) where the app is now and where it goes, on its
    /// each team its grants name, organization-wide for grants with no team
    /// ([`Self::manages_access`]). Org admins and the owner hold it everywhere, a
    /// team's leads on their team.
    #[must_use]
    pub fn may_set_access(&self, app: &AppRecord, to: &Access, actor: &str) -> bool {
        let org = app.org.as_str();
        self.manages_access(org, actor, &app.access) && self.manages_access(org, actor, to)
    }

    /// Whether `account` holds `app`'s owner powers: it owns the app and, for
    /// an organization's app, is a member of that organization (`member` or
    /// above, as owning an app there takes) and, for an app on a team (an
    /// app team, a private channel under it), in that team. An owner who
    /// left, was removed or was made a guest, or who left or was removed
    /// from the app's team, keeps the name on the record but none of its
    /// powers (its access, metadata, transfer, deletion, upgrades, upgrade
    /// keys, files and blobs), as [`Self::may_reach`] no longer lets them
    /// reach it. A personal app is its owner's alone.
    #[must_use]
    pub fn owner_acts(&self, app: &AppRecord, account: &str) -> bool {
        self.owner_acts_as_logged(app, account, OwnerRule::CURRENT)
    }

    /// [`Self::owner_acts`], as the release that logged the command judged
    /// it ([`OwnerRule`]).
    fn owner_acts_as_logged(&self, app: &AppRecord, account: &str, rule: OwnerRule) -> bool {
        if app.owner != account {
            return false;
        }
        let org = app.org.as_str();
        if rule == OwnerRule::Ownership {
            return true;
        }
        let Some(o) = self.orgs.get(org) else {
            return false;
        };
        o.role(account).is_some_and(|r| r >= OrgRole::Member)
            && (rule == OwnerRule::InOrg || self.owner_in_team(app, account))
    }

    /// Whether `app`'s team, if it is on one, holds `owner`: an owner reaches
    /// and manages an app on a team only while they are in it.
    fn owner_in_team(&self, app: &AppRecord, owner: &str) -> bool {
        let teams = app.access.teams();
        if teams.is_empty() || app.access.org_wide() || app.access.names(owner) {
            return true;
        }
        self.orgs
            .get(&app.org)
            .is_some_and(|o| teams.into_iter().any(|t| o.in_team(owner, t)))
    }

    /// Whether `account` manages `app` as a whole (transfers or deletes it,
    /// sets its metadata, deletes its blobs, reads its usage):
    /// [`Self::owner_acts`], or an admin of its organization, so an
    /// organization cleans up the app of an owner who left.
    #[must_use]
    pub fn manages_app(&self, app: &AppRecord, account: &str) -> bool {
        self.manages_app_as_logged(app, account, OwnerRule::CURRENT)
    }

    /// [`Self::manages_app`], with the owner judged as the release that
    /// logged the command did ([`OwnerRule`]).
    fn manages_app_as_logged(&self, app: &AppRecord, account: &str, rule: OwnerRule) -> bool {
        self.owner_acts_as_logged(app, account, rule) || self.is_org_admin(&app.org, account)
    }

    /// Whether `actor` sees (and may revoke) the organization invite `inv`:
    /// an admin or the owner of its organization, its creator while still a
    /// member, or whoever manages every team it names (an invite naming no
    /// team is organization-wide: an admin's).
    #[must_use]
    pub fn sees_invite(&self, inv: &Invite, actor: &str) -> bool {
        let Some(o) = self.orgs.get(&inv.org) else {
            return false;
        };
        let Some(role) = o.role(actor) else {
            return false;
        };
        role >= OrgRole::Admin
            || inv.created_by == actor
            || (!inv.teams.is_empty()
                && inv
                    .teams
                    .iter()
                    .all(|t| o.teams.contains_key(t) && o.manages_team(actor, t)))
    }
}

/// Whether `actor` may write a row, a row's capabilities or the default of
/// `team` granting `bits`, from [`cluster_version::Feature::HeldTeamGrants`]
/// on (`held`): below admin, only on a team they see, and only bits they hold
/// organization-wide, as an invite's ([`SystemCmd::CreateInvite`]). An
/// org-wide `MANAGE_MEMBERS`, which [`Org::may`] falls back to on a team
/// without a row, opens no restricted team, and nobody below admin grants
/// themselves or anyone else a capability they lack (such as
/// `MANAGE_APPLICATION`, which manages the team's apps and their access).
///
/// # Errors
///
/// [`TEAM_NOT_SEEN`], [`UNKNOWN_CAPABILITIES`] or [`CANNOT_GRANT_UNHELD`].
pub(crate) fn team_grant_refusal(
    o: &Org,
    team: &str,
    actor: &str,
    bits: Option<u32>,
    held: bool,
) -> Result<(), Rejection> {
    if !held {
        return Ok(());
    }
    if bits.is_some_and(|c| !Org::defined_caps(c)) {
        return Err(UNKNOWN_CAPABILITIES);
    }
    if o.role(actor).is_some_and(|r| r >= OrgRole::Admin) {
        return Ok(());
    }
    if !o.sees_team(actor, team) {
        return Err(TEAM_NOT_SEEN);
    }
    if bits.is_some_and(|c| !o.may_grant(actor, c)) {
        return Err(CANNOT_GRANT_UNHELD);
    }
    Ok(())
}

/// `account`'s row in `team` at `role` (with `capabilities`, else what a new
/// row takes), as `actor` sets it: [`SystemCmd::SetTeamMember`]'s rule, which
/// [`SystemCmd::SetTeamMembers`] applies to each of its teams. `held`: the
/// logged cluster version brings [`cluster_version::Feature::HeldTeamGrants`].
fn set_team_row(
    o: &mut Org,
    team: &str,
    account: &str,
    role: TeamRole,
    capabilities: Option<u32>,
    actor: &str,
    held: bool,
) -> Result<(), Rejection> {
    use crate::types::caps;

    let Some(t) = o.teams.get(team) else {
        return Err("no such team".into());
    };
    // Joining an open team one reaches anyway writes one's own row.
    let joins_self = actor == account
        && role == TeamRole::Member
        && t.join == TeamJoin::Everyone
        && o.in_team(actor, team);
    let manages = o.manages_team(actor, team)
        || (role != TeamRole::Lead && o.may(actor, Some(team), caps::MANAGE_MEMBERS));
    if !joins_self && !manages {
        return Err("not allowed to manage this team".into());
    }
    team_grant_refusal(o, team, actor, capabilities, held)?;
    if o.role(account).is_none() {
        return Err("only organization members join teams".into());
    }
    // A new row takes what the member holds organization-wide, and the
    // team's default on top.
    let default = o
        .capabilities
        .get(account)
        .copied()
        .unwrap_or(o.default_capabilities)
        | t.default_capabilities.unwrap_or(0);
    let t = o.teams.get_mut(team).ok_or("no such team")?;
    let _was = t.removed.remove(account);
    let _prev = t.members.insert(account.to_owned(), role == TeamRole::Lead);
    if role == TeamRole::ReadOnly {
        let _new = t.read_only.insert(account.to_owned());
    } else {
        let _was = t.read_only.remove(account);
    }
    match capabilities {
        Some(c) => {
            let _prev = t.capabilities.insert(account.to_owned(), c);
        }
        None => {
            let _kept = t.capabilities.entry(account.to_owned()).or_insert(default);
        }
    }
    Ok(())
}

/// Take `account`'s row in `team` (and, with `subtree`, in every team below
/// it), as `actor` removes it or `account` leaves: [`SystemCmd::RemoveTeamMember`]'s
/// rule, which [`SystemCmd::RemoveTeamMembers`] applies to each of its teams.
fn remove_team_row(
    o: &mut Org,
    team: &str,
    account: &str,
    actor: &str,
    subtree: bool,
) -> Result<(), &'static str> {
    if actor != account && !o.manages_team(actor, team) {
        return Err("not allowed to manage this team");
    }
    if !o.teams.contains_key(team) {
        return Err("no such team");
    }
    // The teams below go too (a private channel under an app's team): a
    // direct row there would keep them in. Whoever manages `team` manages
    // those; a leaver leaves them.
    let below = if subtree {
        o.subtree(team)
    } else {
        vec![team.to_owned()]
    };
    for id in &below {
        if let Some(t) = o.teams.get_mut(id) {
            let _prev = t.members.remove(account);
            let _was = t.read_only.remove(account);
            let _caps = t.capabilities.remove(account);
        }
    }
    let t = o.teams.get_mut(team).ok_or("no such team")?;
    // A removal, and a leave too, keeps them from coming back by
    // inheritance: an open team would otherwise put a leaver straight back,
    // and a read-only leaver would shed the role. They return only when a
    // lead re-adds them or an invite names the team.
    let _new = t.removed.insert(account.to_owned());
    Ok(())
}

/// Whether `actor` may set `account`'s `rows` in `o`, judged as
/// [`SystemCmd::SetTeamMembers`] applies them, on a copy: the consent page
/// asks it before it shows a page the log would refuse.
///
/// # Errors
///
/// The refusal `SetTeamMembers` would answer.
pub(crate) fn judge_team_rows(
    o: &Org,
    account: &str,
    rows: &[TeamRow],
    actor: &str,
    held: bool,
) -> Result<(), Rejection> {
    check_team_batch(rows.iter().map(|r| r.team.as_str()))?;
    let mut next = o.clone();
    for row in rows {
        let role = row.role.unwrap_or(TeamRole::Member);
        set_team_row(
            &mut next,
            &row.team,
            account,
            role,
            row.capabilities,
            actor,
            held,
        )?;
    }
    Ok(())
}

/// Whether `actor` may take `account` out of `teams` of `o`, judged as
/// [`SystemCmd::RemoveTeamMembers`] applies it, on a copy.
///
/// # Errors
///
/// The refusal `RemoveTeamMembers` would answer.
pub(crate) fn judge_remove_team_rows(
    o: &Org,
    account: &str,
    teams: &[String],
    actor: &str,
) -> Result<(), &'static str> {
    check_team_batch(teams.iter().map(String::as_str))?;
    let mut next = o.clone();
    for team in teams {
        remove_team_row(&mut next, team, account, actor, true)?;
    }
    Ok(())
}

/// A batch of team rows names 1 to [`MAX_TEAM_BATCH`] teams, each once.
///
/// # Errors
///
/// When it names none, too many, or one twice.
pub fn check_team_batch<'a>(
    teams: impl ExactSizeIterator<Item = &'a str>,
) -> Result<(), &'static str> {
    if teams.len() == 0 {
        return Err("name at least one team");
    }
    if teams.len() > MAX_TEAM_BATCH {
        return Err("too many teams in one step");
    }
    let mut seen = BTreeSet::new();
    for t in teams {
        if !seen.insert(t) {
            return Err("a team is named twice");
        }
    }
    Ok(())
}

/// How the release that logged a [`SystemCmd::ScopeApps`] judged it: each
/// flag is the entry's own, absent (`false`) in an earlier release's entry.
#[derive(Clone, Copy, Debug)]
struct ScopeAsLogged {
    /// A new app team's default holds `CREATE_CHILD_TEAM`.
    member_child_teams: bool,
    /// A new app team gives its `all_apps` members a row.
    all_apps_rows: bool,
    /// An instance's package is the one its release names
    /// ([`SystemState::instance_package`]).
    by_release: bool,
}

impl ScopeAsLogged {
    /// Every rule as this release judges it: what a command this release
    /// writes and applies in the same entry (`CreateOrg { app_team }`) uses.
    const CURRENT: Self = Self {
        member_child_teams: true,
        all_apps_rows: true,
        by_release: true,
    };
}

impl SystemState {
    /// [`SystemCmd::ScopeApps`], judged and applied as one step: everything
    /// is checked and every person who reaches an instance now is found
    /// before anything changes.
    fn scope_apps(
        &mut self,
        org: &str,
        apps: Option<Vec<ScopeApp>>,
        shared: Option<bool>,
        nonce: &str,
        actor: &str,
        logged: ScopeAsLogged,
    ) -> Result<Value, Rejection> {
        use crate::types::caps;

        let o = self.orgs.get(org).ok_or("no such organization")?;
        if !o.role(actor).is_some_and(|r| r >= OrgRole::Admin) {
            return Err(ONLY_ADMINS_INSTALL.into());
        }
        let in_org = |a: &&AppRecord| a.org == org;
        // A package named outright with no release the actor may use answers as a missing
        // one; entries logged below the version that brings private releases apply as they did.
        let gated = self.cluster_brings(cluster_version::Feature::PrivateReleases);
        let named = apps.is_some();
        let specs = match apps {
            Some(specs) => specs,
            None => self
                .apps
                .values()
                .filter(in_org)
                .filter(|a| a.access.org_wide())
                .filter_map(|a| self.instance_package(a, logged.by_release))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|package| ScopeApp {
                    package: package.to_owned(),
                    name: None,
                    instances: None,
                    join: None,
                    keep: None,
                    child_teams: None,
                })
                .collect(),
        };
        if specs.len() > MAX_SCOPE_PACKAGES {
            return Err("too many packages in one step".into());
        }
        // A package's own `join` (or `childTeams`) decides its team and no
        // other: `shared` then closes nothing more unless asked, and never
        // one opened here.
        let per_app = specs
            .iter()
            .any(|a| a.join.is_some() || a.child_teams.is_some());
        if shared == Some(true) && specs.iter().any(|a| a.join == Some(TeamJoin::Everyone)) {
            return Err("an app team cannot open while the step closes every app team".into());
        }
        // Each package: its team (found, or to make), name, instances, and
        // who reaches those instances now.
        struct Plan {
            package: String,
            team: Option<TeamId>,
            name: String,
            instances: Vec<AppId>,
            reach: BTreeSet<AccountId>,
            join: Option<TeamJoin>,
            keep: Option<BTreeSet<AccountId>>,
            child_teams: Option<bool>,
        }
        let mut plans: Vec<Plan> = Vec::with_capacity(specs.len());
        for spec in specs {
            if plans.iter().any(|p| p.package == spec.package) {
                return Err("a package is named twice".into());
            }
            let record = self.packages.get(&spec.package).ok_or("no such package")?;
            let newest = record.latest_where(|v| !gated || self.sees_release(v, actor));
            if named && newest.is_none() {
                return Err("no such package".into());
            }
            let name = match spec.name {
                Some(n) => n,
                None => newest
                    .and_then(|v| v.metadata.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(spec.package.as_str())
                    .to_owned(),
            };
            if name.trim().is_empty() || name.chars().count() > MAX_APP_TEAM_NAME_CHARS {
                return Err("an app team's name is 1 to 64 characters".into());
            }
            let keep = match spec.keep {
                None => None,
                Some(_) if spec.join != Some(TeamJoin::Invite) => {
                    return Err("only closing an app team (join: invite) names who keeps it".into());
                }
                Some(list) => {
                    if list.len() > MAX_SCOPE_KEEP {
                        return Err("too many accounts kept in one step".into());
                    }
                    if list.iter().any(|a| o.role(a).is_none()) {
                        return Err("only members of the organization keep an app".into());
                    }
                    Some(list.into_iter().collect::<BTreeSet<_>>())
                }
            };
            let of_package = |a: &AppRecord| {
                self.instance_package(a, logged.by_release) == Some(spec.package.as_str())
            };
            let instances: Vec<AppId> = match spec.instances {
                Some(ids) => {
                    if ids.len() > MAX_SCOPE_INSTANCES {
                        return Err("too many instances in one step".into());
                    }
                    let ids: BTreeSet<AppId> = ids.into_iter().collect();
                    for id in &ids {
                        let a = self.apps.get(id).ok_or("no such app")?;
                        if a.org != org || !of_package(a) {
                            return Err(
                                "an instance named is not of that package in this organization"
                                    .into(),
                            );
                        }
                    }
                    ids.into_iter().collect()
                }
                None => self
                    .apps
                    .values()
                    .filter(in_org)
                    .filter(|a| a.access.org_wide() && of_package(a))
                    .map(|a| a.app_id.clone())
                    .collect(),
            };
            let reach = instances
                .iter()
                .filter_map(|id| self.apps.get(id))
                .flat_map(|a| {
                    o.members
                        .keys()
                        .filter(|m| self.may_reach(a, Some(m.as_str())))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .collect();
            plans.push(Plan {
                team: o.app_team(&spec.package).map(|t| t.team_id.clone()),
                package: spec.package,
                name,
                instances,
                reach,
                join: spec.join,
                keep,
                child_teams: spec.child_teams,
            });
        }
        let app_teams = o
            .teams
            .values()
            .filter(|t| t.app_package().is_some())
            .count()
            + plans.iter().filter(|p| p.team.is_none()).count();
        let shared = shared.unwrap_or(!per_app && app_teams >= 2);

        let o = self.orgs.get_mut(org).ok_or("no such organization")?;
        let mut report = Vec::with_capacity(plans.len());
        for plan in plans {
            let created = plan.team.is_none();
            let team_id = match plan.team {
                Some(t) => t,
                None => {
                    let id = app_team_id(o, nonce, &plan.package);
                    let mut members = BTreeMap::new();
                    let _lead = members.insert(actor.to_owned(), true);
                    // Standalone, its members may invite others to it; shared,
                    // only its leads and the admins, until an admin says so.
                    // Either way they make private channels below it.
                    let invite = (!shared).then_some(caps::INVITE_MEMBERS);
                    let child = logged.member_child_teams.then_some(caps::CREATE_CHILD_TEAM);
                    let default_capabilities = match (invite, child) {
                        (None, None) => None,
                        (a, b) => Some(a.unwrap_or(0) | b.unwrap_or(0)),
                    };
                    let _prev = o.teams.insert(
                        id.clone(),
                        Team {
                            team_id: id.clone(),
                            name: plan.name.clone(),
                            parent: None,
                            join: TeamJoin::Everyone,
                            members,
                            read_only: BTreeSet::new(),
                            capabilities: BTreeMap::new(),
                            metadata: json!({ "app": { "package": plan.package, "name": plan.name } }),
                            removed: BTreeSet::new(),
                            default_capabilities,
                        },
                    );
                    id
                }
            };
            // A new app team is an app added to the organization: whoever
            // gets every app added later is in it, and stays in on a close
            // with a list.
            let all_apps: BTreeSet<AccountId> = if created && logged.all_apps_rows {
                o.all_apps_members().into_iter().collect()
            } else {
                BTreeSet::new()
            };
            // Closing with a list: the people the admin keeps, whoever
            // reaches the moved instances or not (the other rows go below).
            // Otherwise everyone who reaches them, and, closing, everyone in
            // the team now.
            let mut seed: BTreeSet<AccountId> = match (&plan.keep, plan.join) {
                (Some(keep), _) => keep.clone(),
                (None, Some(TeamJoin::Invite)) => {
                    let mut all = plan.reach.clone();
                    all.extend(o.members.keys().filter(|m| o.in_team(m, &team_id)).cloned());
                    all
                }
                (None, _) => plan.reach.clone(),
            };
            seed.extend(all_apps.iter().cloned());
            let t = o.teams.get_mut(&team_id).ok_or("no such team")?;
            // Its members make private channels below it, or no longer do,
            // as the admin says: its default, and every row that sets its
            // own capabilities (a row added one by one keeps what it got
            // then). How an app team made before the default held
            // `CREATE_CHILD_TEAM` gets it.
            if let Some(on) = plan.child_teams {
                let set = |c: u32| {
                    if on {
                        c | caps::CREATE_CHILD_TEAM
                    } else {
                        c & !caps::CREATE_CHILD_TEAM
                    }
                };
                t.default_capabilities = match t.default_capabilities {
                    None if !on => None,
                    c => Some(set(c.unwrap_or(0))),
                };
                for c in t.capabilities.values_mut() {
                    *c = set(*c);
                }
            }
            let mut added = 0usize;
            for a in &seed {
                let _back = t.removed.remove(a);
                if !t.members.contains_key(a) {
                    let _prev = t.members.insert(a.clone(), false);
                    added += 1;
                }
            }
            let mut dropped = 0usize;
            match plan.join {
                Some(TeamJoin::Invite) => {
                    let inside: Vec<AccountId> = o
                        .members
                        .keys()
                        .filter(|m| o.in_team(m, &team_id))
                        .cloned()
                        .collect();
                    let t = o.teams.get_mut(&team_id).ok_or("no such team")?;
                    // A list names everyone but the leads who keeps it: the
                    // other rows go (not barred: opening it again lets them
                    // back in, as it does everyone).
                    if let Some(keep) = &plan.keep {
                        let out: Vec<AccountId> = t
                            .members
                            .iter()
                            .filter(|(a, lead)| {
                                !**lead && !keep.contains(*a) && !all_apps.contains(*a)
                            })
                            .map(|(a, _)| a.clone())
                            .collect();
                        for a in &out {
                            let _prev = t.members.remove(a);
                            let _was = t.read_only.remove(a);
                            let _caps = t.capabilities.remove(a);
                        }
                    }
                    t.join = TeamJoin::Invite;
                    t.default_capabilities =
                        t.default_capabilities.map(|c| c & !caps::INVITE_MEMBERS);
                    // Who lost it leaves the teams below it too: a row there
                    // would keep them in its private channels.
                    let gone: Vec<AccountId> = inside
                        .into_iter()
                        .filter(|m| !o.in_team(m, &team_id))
                        .collect();
                    dropped = gone.len();
                    o.drop_rows_below(&team_id, &gone);
                }
                Some(TeamJoin::Everyone) => t.join = TeamJoin::Everyone,
                None => {}
            }
            let join = o.teams.get(&team_id).map(|t| t.join);
            report.push(ScopedTeam {
                package: plan.package.clone(),
                team_id: team_id.clone(),
                created,
                apps: plan.instances.clone(),
                added,
                join,
                dropped,
            });
            for id in &plan.instances {
                if let Some(a) = self.apps.get_mut(id) {
                    a.access = a.access.onto_team(team_id.clone());
                }
            }
        }
        if shared {
            // Close every app team, its people kept as rows of their own.
            let o = self.orgs.get(org).ok_or("no such organization")?;
            let open: Vec<(TeamId, Vec<AccountId>)> = o
                .teams
                .values()
                .filter(|t| t.app_package().is_some() && t.join == TeamJoin::Everyone)
                .map(|t| {
                    let inside = o
                        .members
                        .keys()
                        .filter(|m| o.in_team(m, &t.team_id))
                        .cloned()
                        .collect();
                    (t.team_id.clone(), inside)
                })
                .collect();
            let o = self.orgs.get_mut(org).ok_or("no such organization")?;
            for (id, inside) in open {
                let Some(t) = o.teams.get_mut(&id) else {
                    continue;
                };
                for a in inside {
                    let _kept = t.members.entry(a).or_insert(false);
                }
                t.join = TeamJoin::Invite;
                t.default_capabilities = t.default_capabilities.map(|c| c & !caps::INVITE_MEMBERS);
            }
        }
        let mode = self
            .orgs
            .get(org)
            .map_or(crate::types::OrgMode::Legacy, Org::mode);
        Ok(json!(AppTeamsScoped {
            mode,
            teams: report
        }))
    }

    /// [`SystemCmd::MoveApp`], judged and applied as one step: every rule is
    /// checked, and everyone it brings is found, before anything changes.
    ///
    /// - **Who:** the actor manages the app where it is now
    ///   ([`Self::manages_app`]: a personal app's owner, an organization's
    ///   app's owner while a member, or an admin there) and is an admin or the
    ///   owner of `to_org`.
    /// - **Grants:** an `org` or app-team grant becomes a `team` grant on
    ///   the target's app team of the app's package, made when missing: open,
    ///   unless every app team of the target is closed (shared), where it is
    ///   closed like the rest; an app team closed alone leaves it open. A legacy target (no app team) and a raw upload
    ///   (no package) get an `org` grant, so moving an app in never
    ///   changes how the target holds its other apps. Named people and
    ///   `anyone` stay as they are; an instance only named people reach whose
    ///   package has a closed app team in the target is reached there only through
    ///   that team ([`Self::may_reach`]), so its owner and who it brings get
    ///   a row on that team (`accounts_in_app_team`; none in an entry an
    ///   earlier release logged). Any other team is refused
    ///   ([`MOVE_NEEDS_APP_TEAM`]): its rows are grants of the source.
    /// - **Owner:** stays, and must be a member of the target once the move
    ///   is done ([`MOVE_OWNER_NOT_IN_TARGET`]); nobody's ownership changes
    ///   without its own consented transfer. An owner who reaches the app now
    ///   gets a row on the target's app team, so they keep reaching and
    ///   managing it there (an owner does so only while in its team).
    /// - **Members:** with `bring_members`, every member of the source who
    ///   reaches the app joins the target at their role there, capped at
    ///   member (a guest stays a guest, an admin comes in as a member: the
    ///   target's admins grant more themselves), and gets a row in the target
    ///   app team. Someone already in the target keeps their role.
    /// - **Payer:** its file bytes and compute runs go to the target, within
    ///   the target's quotas (`quota exceeded: ...` otherwise).
    /// - **Releases:** refused when the target approves none of the releases
    ///   that hold the app now ([`MOVE_RELEASES_DISJOINT`]).
    /// - **Invites:** the source's invites naming the team the app leaves are
    ///   revoked once no app of the source is on that team.
    fn move_app(
        &mut self,
        app_id: &str,
        to_org: &str,
        bring_members: bool,
        nonce: &str,
        actor: &str,
        rows: MoveRows,
    ) -> Result<Value, Rejection> {
        use crate::types::{caps, OrgMode};

        let MoveRows {
            all_apps_rows,
            accounts_in_app_team,
            org_quotas,
        } = rows;

        let app = self.apps.get(app_id).ok_or("no such app")?;
        let from = app.org.clone();
        if from == to_org {
            return Err("the app is in that organization already".into());
        }
        if !self.manages_app(app, actor) {
            return Err(ONLY_MANAGERS_MOVE.into());
        }
        let target = self.orgs.get(to_org).ok_or("no such organization")?;
        if !target.role(actor).is_some_and(|r| r >= OrgRole::Admin) {
            return Err(ONLY_ADMINS_MOVE_IN.into());
        }
        let source = self.orgs.get(&from);
        let package = self.package_of(app).map(str::to_owned);
        // A team grant moves only as the package's app team, which the target has its own of.
        let source_team = match app.access.teams().as_slice() {
            [] => None,
            [team] => {
                let app_team = source
                    .and_then(|o| o.teams.get(*team))
                    .and_then(Team::app_package)
                    .is_some_and(|p| package.as_deref() == Some(p));
                if !app_team {
                    return Err(MOVE_NEEDS_APP_TEAM.into());
                }
                Some((*team).clone())
            }
            _ => return Err(MOVE_NEEDS_APP_TEAM.into()),
        };
        let to_app_team = (app.access.org_wide() || source_team.is_some())
            && package.is_some()
            && target.mode() != OrgMode::Legacy;

        // Who comes along: the source's members who reach the app now.
        let reached: Vec<(AccountId, OrgRole)> = match source {
            Some(o) if bring_members => o
                .members
                .iter()
                .filter(|(m, _)| self.may_reach(app, Some(m.as_str())))
                .map(|(m, r)| (m.clone(), *r))
                .collect(),
            _ => Vec::new(),
        };
        // An owner who reaches the app now keeps reaching it on the target's
        // app team (an owner reaches an app on a team only while in it), so
        // a move never takes their app away without their consent.
        let owner_keeps = self
            .may_reach(app, Some(app.owner.as_str()))
            .then(|| app.owner.clone());
        let joins: Vec<(AccountId, OrgRole)> = reached
            .iter()
            .filter(|(m, _)| target.role(m).is_none())
            .map(|(m, r)| (m.clone(), (*r).min(OrgRole::Member)))
            .collect();
        let owner_role = target
            .role(&app.owner)
            .or_else(|| joins.iter().find(|(m, _)| *m == app.owner).map(|(_, r)| *r));
        if !owner_role.is_some_and(|r| r >= OrgRole::Member) {
            return Err(MOVE_OWNER_NOT_IN_TARGET.into());
        }

        let release = self.governing_release(app);
        let before = self.approved_for_release(release, &from);
        let after = self.approved_for_release(release, to_org);
        if let (Some(b), Some(a)) = (before, after) {
            if b.is_disjoint(a) {
                return Err(MOVE_RELEASES_DISJOINT.into());
            }
        }

        // The target pays from here on: its quotas hold what the app holds.
        // Its own override where it has one; an entry from before
        // `org_quotas` judged the cluster's limits, and replays so.
        let (max_runs, max_fuel, max_bytes) = if org_quotas {
            self.payer_quotas(to_org)
        } else {
            (
                self.quotas.max_concurrent_runs_per_org,
                self.quotas.compute_fuel_per_org,
                self.quotas.max_file_bytes_per_org,
            )
        };
        let held = self.app_file_bytes.get(app_id).copied().unwrap_or(0);
        if self.file_bytes(to_org).saturating_add(held) > max_bytes {
            return Err(Rejection::quota("file bytes"));
        }
        let granted = |slots: &mut dyn Iterator<Item = &RunSlot>| {
            slots
                .filter(|slot| slot.granted)
                .fold((0u32, 0u64), |(n, f), slot| {
                    (n.saturating_add(1), f.saturating_add(slot.fuel))
                })
        };
        let (runs, fuel) = self
            .compute
            .get(app_id)
            .map_or((0, 0), |r| granted(&mut r.values()));
        let (target_runs, target_fuel) = granted(
            &mut self
                .compute
                .values()
                .flat_map(BTreeMap::values)
                .filter(|slot| slot.payer == to_org),
        );
        if runs > 0
            && (target_runs.saturating_add(runs) > max_runs
                || target_fuel.saturating_add(fuel) > max_fuel)
        {
            return Err(Rejection::quota("compute runs"));
        }

        // The team whose invites lose their reason once the app leaves it.
        let leaves_team = source_team.or_else(|| {
            package
                .as_deref()
                .and_then(|p| source?.app_team(p))
                .map(|t| t.team_id.clone())
        });
        let gated = self.cluster_brings(cluster_version::Feature::PrivateReleases);
        let team_name = package.as_deref().map(|p| {
            let name = self
                .packages
                .get(p)
                .and_then(|r| r.latest_where(|v| !gated || self.sees_release(v, actor)))
                .and_then(|v| v.metadata.get("name"))
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(p);
            name.chars()
                .take(MAX_APP_TEAM_NAME_CHARS)
                .collect::<String>()
        });
        // Closed like the rest only where every app team is closed: an app
        // team an admin closed alone (`ScopeApp::join`) leaves the others,
        // and so the one made here, open.
        let shared = target.mode() == OrgMode::Shared
            && target
                .teams
                .values()
                .filter(|t| t.app_package().is_some())
                .all(|t| t.join == TeamJoin::Invite);
        // An app team made here is an app added to the target: whoever
        // there gets every app added later has a row on it.
        let all_apps = if all_apps_rows {
            target.all_apps_members()
        } else {
            Vec::new()
        };
        // An instance only named people reach (a DM, a private list) of a package whose
        // app team in the target is closed is reached there only by those in
        // that team: who comes along, and its owner, get a row on it, as
        // they would on the app team an `org` or `team` instance goes to.
        let accounts_team = match (app.access.named_only(), package.as_deref()) {
            (true, Some(p)) if accounts_in_app_team => target
                .app_team(p)
                .filter(|t| t.join == TeamJoin::Invite)
                .map(|t| t.team_id.clone()),
            _ => None,
        };
        // App-team access without an app team to go to: the organization's.
        let mut access = app.access.teams_to_org();

        // Everything is judged: apply.
        let o = self.orgs.get_mut(to_org).ok_or("no such organization")?;
        for (m, r) in &joins {
            let _prev = o.members.insert(m.clone(), *r);
        }
        let mut created = false;
        if let (true, Some(package), Some(name)) = (to_app_team, package, team_name) {
            let team_id = match o.app_team(&package) {
                Some(t) => t.team_id.clone(),
                None => {
                    let id = app_team_id(o, nonce, &package);
                    let mut members = BTreeMap::new();
                    let _lead = members.insert(actor.to_owned(), true);
                    let _prev = o.teams.insert(
                        id.clone(),
                        Team {
                            team_id: id.clone(),
                            name: name.clone(),
                            parent: None,
                            join: if shared {
                                TeamJoin::Invite
                            } else {
                                TeamJoin::Everyone
                            },
                            members,
                            read_only: BTreeSet::new(),
                            capabilities: BTreeMap::new(),
                            metadata: json!({ "app": { "package": package, "name": name } }),
                            removed: BTreeSet::new(),
                            // As `ScopeApps` makes one: its members make
                            // private channels below it, and invite while
                            // it is open.
                            default_capabilities: Some(if shared {
                                caps::CREATE_CHILD_TEAM
                            } else {
                                caps::INVITE_MEMBERS | caps::CREATE_CHILD_TEAM
                            }),
                        },
                    );
                    created = true;
                    id
                }
            };
            let t = o.teams.get_mut(&team_id).ok_or("no such team")?;
            let new_team = if created { all_apps.as_slice() } else { &[] };
            for m in reached
                .iter()
                .map(|(m, _)| m)
                .chain(owner_keeps.iter())
                .chain(new_team)
            {
                let _back = t.removed.remove(m);
                let _row = t.members.entry(m.clone()).or_insert(false);
            }
            access = access.onto_team(team_id);
        }
        if let Some(team_id) = accounts_team {
            let t = o.teams.get_mut(&team_id).ok_or("no such team")?;
            for m in reached.iter().map(|(m, _)| m).chain(owner_keeps.iter()) {
                let _back = t.removed.remove(m);
                let _row = t.members.entry(m.clone()).or_insert(false);
            }
        }
        self.set_app_file_bytes(app_id, 0);
        if let Some(a) = self.apps.get_mut(app_id) {
            a.org = to_org.to_owned();
            a.access = access.clone();
        }
        // Its update policy and a first approval of its update were the source's owner's
        // and admins' decisions: the target's decide there.
        if let Some(o) = self.orgs.get_mut(&from) {
            let _policy = o.app_upgrade_policies.remove(app_id);
            let _approval = o.pending_upgrades.remove(app_id);
        }
        self.set_app_file_bytes(app_id, held);
        if let Some(slots) = self.compute.get_mut(app_id) {
            for slot in slots.values_mut() {
                to_org.clone_into(&mut slot.payer);
            }
            self.grant_runs();
        }
        let mut revoked = 0usize;
        if let Some(team) = leaves_team.as_deref() {
            let from = from.as_str();
            let still = self
                .apps
                .values()
                .any(|a| a.org == from && a.access.teams().iter().any(|t| t.as_str() == team));
            if !still {
                let before = self.invites.len();
                self.invites
                    .retain(|_, i| !(i.org == from && i.teams.iter().any(|t| t == team)));
                revoked = before - self.invites.len();
            }
        }
        Ok(json!(AppMoved {
            app_id: app_id.to_owned(),
            from,
            org: to_org.to_owned(),
            grants: access,
            created,
            added: joins.into_iter().map(|(m, _)| m).collect(),
            revoked_invites: revoked as u64,
        }))
    }

    /// `MANAGE_APPLICATION`: whether `account` may upgrade `org`'s apps
    /// (on `team`, or org-wide when `None`) and approve its outbound hosts.
    /// The owner, an admin, a team's lead, or a member holding the capability
    /// there; never someone outside the organization or below member.
    #[must_use]
    pub fn manages_apps(&self, org: &str, account: &str, team: Option<&str>) -> bool {
        self.orgs.get(org).is_some_and(|o| {
            o.role(account).is_some_and(|r| r >= OrgRole::Member)
                && o.may(account, team, crate::types::caps::MANAGE_APPLICATION)
        })
    }

    /// `APPROVE_RELEASES`: whether `account` may approve (or withdraw) the
    /// mero-tee releases that hold `org`'s apps. The owner, an admin, or a
    /// member granted the capability organization-wide (a team grant does not
    /// reach it: one approval covers every app of the organization); never
    /// someone outside it or below member, and nobody where the operator
    /// holds release approvals.
    #[must_use]
    pub fn approves_releases(&self, org: &str, account: &str) -> bool {
        self.governance().releases == Authority::Owner && self.holds_approve_releases(org, account)
    }

    /// Who may approve `org`'s releases ([`Self::approves_releases`]).
    #[must_use]
    pub fn release_approvers(&self, org: &str) -> Vec<AccountId> {
        self.orgs.get(org).map_or_else(Vec::new, |o| {
            o.members
                .keys()
                .filter(|a| self.approves_releases(org, a))
                .cloned()
                .collect()
        })
    }

    /// Whether a scope whose approved releases are `approved` has lost new
    /// work at `now`: some release's deadline has passed and the newest
    /// release it approved is older than that release. The newest such
    /// release is named, since approving it lifts every one. `None` approved
    /// (state from before approvals) is unrestricted, and so never stalled.
    #[must_use]
    pub fn stalled(&self, approved: Option<&BTreeSet<String>>, now: u64) -> Option<Stall> {
        let newest = approved?.iter().max_by(|a, b| compare_versions(a, b))?;
        self.release_deadlines
            .iter()
            .filter(|(r, after)| **after <= now && compare_versions(newest, r).is_lt())
            .max_by(|a, b| compare_versions(a.0, b.0))
            .map(|(r, after)| Stall {
                release: r.clone(),
                after: *after,
            })
    }

    /// [`Self::stalled`] for the set `org` holds itself: what governs the
    /// packages it publishes and its own apps of unbound packages.
    #[must_use]
    pub fn stalled_for(&self, org: &str, now: u64) -> Option<Stall> {
        self.stalled(self.approved_for(org), now)
    }

    /// [`Self::stalled`] for an app of `org` governed by `release`: judged on
    /// the set that governs it ([`Self::approved_for_release`]).
    #[must_use]
    pub fn stalled_for_release(
        &self,
        release: Option<&PackageRef>,
        org: &str,
        now: u64,
    ) -> Option<Stall> {
        self.stalled(self.approved_for_release(release, org), now)
    }

    /// Whether `actor` may install no release at `now`. An installed release
    /// belongs to its installer, who may use it in their own apps or in any
    /// organization they are a member of, so the install is refused only when
    /// every one of those scopes is stalled; the app it would become is
    /// judged on its own (`CreateApp`). The operator's installs never are.
    /// A release of a bound key is judged on its publisher's set instead.
    #[must_use]
    pub fn install_stalled(&self, actor: &str, signer: &str, now: u64) -> Option<Stall> {
        if actor == OPERATOR {
            return None;
        }
        if let Some(p) = self.publisher_of_signer(signer) {
            if self.governance().releases == Authority::Owner {
                return self.stalled_for(p, now);
            }
        }
        // Refused only when every organization they may use it in is stalled;
        // an account in none yet gets a personal organization, made on the
        // release it is created through, with its first app.
        let mut stalls = self
            .orgs
            .values()
            .filter(|o| o.role(actor).is_some_and(|r| r >= OrgRole::Member))
            .map(|o| self.stalled_for(&o.org_id, now));
        let first = stalls.next()?;
        if first.is_none() || stalls.any(|s| s.is_none()) {
            return None;
        }
        first
    }

    /// Whether a node on `release` is past a deadline at `now`: some newer
    /// release's deadline has passed. Such a node keeps the apps it holds but
    /// is placed no new one.
    #[must_use]
    pub fn retired(&self, release: &str, now: u64) -> bool {
        self.release_deadlines
            .iter()
            .any(|(r, after)| *after <= now && compare_versions(release, r).is_lt())
    }

    /// Whether any release's deadline has passed at `now`.
    #[must_use]
    pub fn any_deadline_passed(&self, now: u64) -> bool {
        self.release_deadlines.values().any(|after| *after <= now)
    }

    /// The deadlines as the read APIs show them, oldest release first.
    #[must_use]
    pub fn deadlines_json(&self) -> Vec<VersionDeadline> {
        let mut out: Vec<(&String, &u64)> = self.release_deadlines.iter().collect();
        out.sort_by(|a, b| compare_versions(a.0, b.0));
        out.into_iter()
            .map(|(r, after)| VersionDeadline {
                version: r.clone(),
                after: rfc3339(*after),
                after_secs: *after,
            })
            .collect()
    }

    /// Whether a node on `release` may hold `app`'s data. `None` is a node
    /// whose release is not known yet, which may not.
    #[must_use]
    pub fn release_may_hold(&self, app: &AppRecord, release: Option<&str>) -> bool {
        match self.approved_releases(app) {
            None => true,
            Some(set) => release.is_some_and(|r| set.contains(r)),
        }
    }

    /// Whether a node on `release` may vouch for `account` (sign its
    /// sessions): a release the operator approved where it holds release
    /// approvals; else one an organization it belongs to approved (its
    /// personal one, made at sign-up, included). An account none of whose
    /// organizations recorded an approval is unrestricted, as its apps are.
    #[must_use]
    pub fn release_may_serve_account(&self, account: &str, release: &str) -> bool {
        if self.governance().releases == Authority::Operator {
            return self.operator_releases.is_empty() || self.operator_releases.contains(release);
        }
        let sets: Vec<&BTreeSet<String>> = self
            .orgs
            .values()
            .filter(|o| o.role(account).is_some())
            .map(|o| &o.approved_releases)
            .filter(|set| !set.is_empty())
            .collect();
        sets.is_empty() || sets.iter().any(|set| set.contains(release))
    }

    /// Whether an entry at `index` must carry its session proof
    /// ([`Self::session_proofs_from`]).
    #[must_use]
    pub fn proves_sessions(&self, index: u64) -> bool {
        self.session_proofs_from.is_some_and(|from| index >= from)
    }

    /// Whether an entry at `index` applies under this release's rules,
    /// whatever its flags name ([`Self::current_rules_from`]).
    #[must_use]
    pub fn holds_current_rules(&self, index: u64) -> bool {
        self.current_rules_from.is_some_and(|from| index >= from)
    }

    /// Whether a refresh token was ever issued under `hash` and is still
    /// remembered (live or exchanged).
    fn refresh_known(&self, hash: &str) -> bool {
        self.refresh.contains_key(hash) || self.rotated_refresh.contains_key(hash)
    }

    /// Revoke `session`: its refresh tokens go and its tokens stop verifying.
    fn revoke_session(&mut self, session: &str, now: u64) {
        self.refresh.retain(|_, r| r.session != session);
        let _new = self.revoked_sessions.insert(session.to_owned());
        let _prev = self.revoked_at.insert(session.to_owned(), now);
    }

    /// Revoke every session `account` has, and forget what no token can
    /// outlive: answers how many it revoked.
    fn revoke_account_sessions(&mut self, account: &str, now: u64) -> usize {
        let sessions: BTreeSet<String> = self
            .refresh
            .values()
            .filter(|r| r.account == account)
            .map(|r| r.session.clone())
            .collect();
        let n = sessions.len();
        for session in sessions {
            self.revoke_session(&session, now);
        }
        self.prune_sessions(now);
        n
    }

    /// Forget revocations no token can outlive, and exchanged refresh tokens
    /// past their expiry.
    fn prune_sessions(&mut self, now: u64) {
        let old: Vec<String> = self
            .revoked_at
            .iter()
            .filter(|(_, at)| **at + REVOKED_SESSION_KEEP_SECS <= now)
            .map(|(s, _)| s.clone())
            .collect();
        for s in old {
            let _gone = self.revoked_at.remove(&s);
            let _gone = self.revoked_sessions.remove(&s);
        }
        self.rotated_refresh.retain(|_, r| r.expires_at > now);
        if self.expires_proofs {
            self.forget_expired_proofs(now);
        }
    }

    /// Forget the proofs whose session expired by `now` (an entry's own time):
    /// a sign-in that expired by then is refused before its proof is checked
    /// against [`Self::used_proofs`] ([`Self::proofs_forgotten_through`]).
    fn forget_expired_proofs(&mut self, now: u64) {
        let live = self.proof_expiry.split_off(&now.saturating_add(1));
        let expired = std::mem::replace(&mut self.proof_expiry, live);
        if let Some(&through) = expired.keys().next_back() {
            self.proofs_forgotten_through = self.proofs_forgotten_through.max(through);
        }
        for proof in expired.into_values().flatten() {
            let _gone = self.used_proofs.remove(&proof);
        }
    }
}

impl SystemState {
    pub fn account_of_credential(&self, cred_id: &str) -> Option<(&Account, &Credential)> {
        let account = self.accounts.get(self.credentials.get(cred_id)?)?;
        let cred = account.passkeys.iter().find(|c| c.id == cred_id)?;
        Some((account, cred))
    }

    /// Whether `account` may reach `app` at all: through one of its grants
    /// (`Self::grants_reach`). Nobody without an account does.
    #[must_use]
    pub fn may_reach(&self, app: &AppRecord, account: Option<&str>) -> bool {
        account.is_some_and(|a| self.grants_reach(app, a))
    }

    /// Whether `account` lets `source`'s runs for them call `method` of `target` as them.
    #[must_use]
    pub fn delegates(&self, account: &str, source: &str, target: &str, method: &str) -> bool {
        self.delegations
            .get(account)
            .and_then(|s| s.get(source))
            .and_then(|t| t.get(target))
            .is_some_and(|m| m.iter().any(|x| x == method))
    }

    /// Refuses a grant list `org`'s instance cannot hold: a grant twice, the organization
    /// beside a team (it covers them), a team not of `org`, an account the cluster does not
    /// know (one name per account), a scope past [`crate::types::MAX_GRANT_SCOPE_BYTES`], or more than
    /// [`crate::types::MAX_TEAM_GRANTS`] teams or [`crate::types::MAX_ACCOUNT_GRANTS`] names.
    pub fn check_grants(&self, org: &str, grants: &[Grant]) -> Result<(), Rejection> {
        if grants.len() > crate::types::MAX_GRANTS {
            return Err(Rejection::quota("an instance holds at most 1018 grants"));
        }
        let o = self.orgs.get(org).ok_or("no such organization")?;
        let mut named: BTreeSet<&str> = BTreeSet::new();
        let mut teams: BTreeSet<&str> = BTreeSet::new();
        let (mut whole, mut anyone) = (false, false);
        for g in grants {
            let fresh = match g {
                Grant::Org => !std::mem::replace(&mut whole, true),
                Grant::Anyone => !std::mem::replace(&mut anyone, true),
                Grant::Link(_) => {
                    return Err("a public link is made with its own consent, not as a grant".into());
                }
                Grant::Team { team } => {
                    if !o.teams.contains_key(team) {
                        return Err("no such team in the app's organization".into());
                    }
                    teams.insert(team)
                }
                Grant::Account { account, scope } => {
                    if !self.accounts.contains_key(account) {
                        return Err("unknown account".into());
                    }
                    if scope.as_ref().is_some_and(|v| !crate::types::scope_fits(v)) {
                        return Err("a grant's scope is at most 256 bytes of JSON".into());
                    }
                    named.insert(account)
                }
            };
            if !fresh {
                return Err("a grant is listed twice".into());
            }
        }
        if whole && !teams.is_empty() {
            return Err("a grant to the organization covers its teams: not both".into());
        }
        if teams.len() > crate::types::MAX_TEAM_GRANTS {
            return Err(Rejection::quota("an instance names at most 16 teams"));
        }
        if named.len() > crate::types::MAX_ACCOUNT_GRANTS {
            return Err(Rejection::quota("an instance names at most 1000 accounts"));
        }
        Ok(())
    }

    /// [`Self::may_reach`] for a signed-in `a`: [`Grant::Anyone`] admits every account.
    /// Someone outside the organization reaches it as a guest
    /// ([`Self::guest_reaches`]: named here, or on the instance whose unit made it, while both
    /// switches allow), or, named in an organization flagged personal, as a personal app's list
    /// did. A member reaches it as its owner (while in a team it admits, when it admits teams
    /// only) or through a grant: the organization (an `app_only` guest of it never), a team
    /// they are in, or their name; a name, and the owner of an instance only names admit, only
    /// while they are in the package's app team where it has one.
    fn grants_reach(&self, app: &AppRecord, a: &str) -> bool {
        let grants = app.access.grants();
        if app.access.anyone() {
            return true;
        }
        let Some(o) = self.orgs.get(&app.org) else {
            return false;
        };
        if o.role(a).is_none() {
            return (o.personal && app.access.names(a)) || self.guest_reaches(app, a);
        }
        let in_app_team = || {
            self.app_team_of(app)
                .is_none_or(|(o, team)| o.in_team(a, &team.team_id))
        };
        if a == app.owner
            && self.owner_in_team(app, a)
            && (!app.access.named_only() || in_app_team())
        {
            return true;
        }
        grants.iter().any(|g| match g {
            Grant::Org => !o.app_only(a),
            Grant::Team { team } => o.in_team(a, team),
            Grant::Account { account, .. } => account == a && in_app_team(),
            Grant::Anyone => true,
            // A link admits its visitors, never an account.
            Grant::Link(_) => false,
        })
    }

    /// The app team of `app`'s package in `app`'s organization, with the
    /// organization, if both are there.
    #[must_use]
    pub fn app_team_of(&self, app: &AppRecord) -> Option<(&Org, &Team)> {
        let o = self.orgs.get(&app.org)?;
        let team = o.app_team(self.package_of(app)?)?;
        Some((o, team))
    }

    /// Whether `account` (`None`: anonymous) receives an event of `app` sent to
    /// `audience` by `emitter` (R9): it must still reach the app, and be in the
    /// audience as the system state stands at this moment, so a removed member
    /// stops receiving at once.
    #[must_use]
    pub fn receives(
        &self,
        app: &AppRecord,
        audience: &Audience,
        emitter: Option<&str>,
        account: Option<&str>,
    ) -> bool {
        if !self.may_reach(app, account) {
            return false;
        }
        let org = |id: &str| (app.org == id).then(|| self.orgs.get(id)).flatten();
        match (audience, account) {
            (Audience::All, _) => true,
            (_, None) => false,
            (Audience::Caller, Some(a)) => emitter == Some(a),
            (Audience::Accounts { accounts }, Some(a)) => accounts.iter().any(|x| x == a),
            (Audience::Team { team }, Some(a)) => {
                self.orgs.get(&app.org).is_some_and(|o| o.in_team(a, team))
            }
            (Audience::OrgMembers { org: id }, Some(a)) => {
                org(id).is_some_and(|o| o.role(a).is_some_and(|r| r >= OrgRole::Member))
            }
            (Audience::OrgAdmins { org: id }, Some(a)) => {
                org(id).is_some_and(|o| o.role(a).is_some_and(|r| r >= OrgRole::Admin))
            }
        }
    }

    /// The apps deleting `team` takes with it: those of `org` on `team` or a
    /// team below it, in id order. None for a team `org` does not hold. With
    /// `DeleteTeam { keep_apps }` the same apps stay, without those teams' grants.
    #[must_use]
    pub fn team_apps(&self, org: &str, team: &str) -> Vec<AppId> {
        let Some(o) = self.orgs.get(org).filter(|o| o.teams.contains_key(team)) else {
            return Vec::new();
        };
        let gone = o.subtree(team);
        self.apps
            .values()
            // Every team it admits is going: an instance that keeps another way in (another
            // team, a grant to the organization) stays and loses only those grants.
            .filter(|a| {
                let teams = a.access.teams();
                a.org == org
                    && !teams.is_empty()
                    && !a.access.org_wide()
                    && teams.into_iter().all(|t| gone.contains(t))
            })
            .map(|a| a.app_id.clone())
            .collect()
    }

    /// Whether `account` may make another API key: it exists and is below
    /// `maxApiKeysPerAccount`.
    ///
    /// # Errors
    /// Why not, as `CreateApiKey` refuses it.
    pub fn api_key_room(&self, account: &str) -> Result<(), Rejection> {
        if !self.accounts.contains_key(account) {
            return Err("unknown account".into());
        }
        let keys = self
            .api_keys
            .values()
            .filter(|k| k.account == account)
            .count();
        if keys >= self.quotas.max_api_keys_per_account as usize {
            return Err(Rejection::quota("API keys"));
        }
        Ok(())
    }

    /// Records `entry` in `account`'s activity ([`Self::activity`]); none
    /// for an account the state does not hold.
    pub fn record_activity(&mut self, account: &str, entry: crate::types::ActivityEntry) {
        if self.accounts.contains_key(account) {
            crate::types::record_activity(
                self.activity.entry(account.to_owned()).or_default(),
                entry,
            );
        }
    }

    #[must_use]
    pub fn is_org_admin(&self, org: &str, account: &str) -> bool {
        self.orgs
            .get(org)
            .and_then(|o| o.role(account))
            .is_some_and(|r| r >= OrgRole::Admin)
    }
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    hex::decode(s).ok()?.try_into().ok()
}

/// The subject a decision over `scope` names, as the API states it.
fn scope_subject(scope: &ReleaseScope, _actor: &str) -> String {
    match scope {
        ReleaseScope::Org { org } => format!("org:{org}"),
        ReleaseScope::Cluster => "cluster".to_owned(),
    }
}

/// Judge the proof a decision carries before it applies:
/// a person's passkey consent over exactly this change, or the operator's
/// signature over it. Answers what marks the proof used, if it carries one.
/// Every node reaches the same answer from the same state, so a forged or
/// missing proof is refused on every replica, whoever proposed it.
///
/// # Errors
/// The proof is missing, does not verify, was used before, or states
/// another change.
pub fn authorize(s: &SystemState, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    let proofs = Proofs { s };
    match cmd.area() {
        Area::Fleet => fleet::authorize(&proofs, cmd),
        Area::Packages => packages::authorize(&proofs, cmd),
        Area::Apps => apps::authorize(&proofs, cmd),
        Area::Compute => compute::authorize(&proofs, cmd),
        Area::Accounts => accounts::authorize(&proofs, cmd),
        Area::Recovery => recovery::authorize(&proofs, cmd),
        Area::Approvals => approvals::authorize(&proofs, cmd),
        Area::Deployments => deployments::authorize(&proofs, cmd),
        Area::SignUp => sign_up::authorize(&proofs, cmd),
        Area::Orgs => orgs::authorize(&proofs, cmd),
        Area::Updates => updates::authorize(&proofs, cmd),
    }
}

/// The checks a command's proof is judged with ([`authorize`]), over the
/// state it applies to.
struct Proofs<'a> {
    s: &'a SystemState,
}

impl Proofs<'_> {
    /// `actor`'s passkey consent, verified under a passkey the account has.
    fn consent_of(
        &self,
        actor: &str,
        consent: &Value,
    ) -> Result<(crate::proof::Statement, String), &'static str> {
        let credentials = self
            .s
            .accounts
            .get(actor)
            .map(|a| a.passkeys.as_slice())
            .ok_or("unknown account")?;
        crate::proof::verify_consent(consent, credentials)
    }

    /// The operator's signature, under the key recorded at genesis.
    fn operator(&self, proof: &Value) -> Result<(crate::proof::Statement, String), &'static str> {
        crate::proof::verify_operator(proof, self.s.operator_key.as_ref())
    }

    /// A proof not used before.
    fn fresh(
        &self,
        (statement, used): (crate::proof::Statement, String),
    ) -> Result<(crate::proof::Statement, String), &'static str> {
        if self.s.used_proofs.contains(&used) {
            return Err("that consent was used already");
        }
        Ok((statement, used))
    }

    /// The statement says exactly this change.
    #[allow(clippy::unused_self, reason = "read beside the other checks")]
    fn states(
        &self,
        st: &crate::proof::Statement,
        subject: &str,
        method: &str,
        args: &dyn Fn(&Value) -> bool,
    ) -> Result<(), &'static str> {
        if st.subject == subject && st.method == method && args(&st.args()) {
            Ok(())
        } else {
            Err("the consent is for another change")
        }
    }

    /// A deployer grant or revocation: the operator's signature while it
    /// grants deployers, else the passkey consent of the account acting for
    /// the cluster's owner; never the other one's.
    fn deployer_proof(
        &self,
        actor: &str,
        proof: Option<&Value>,
        consent: Option<&Value>,
    ) -> Result<(crate::proof::Statement, String), &'static str> {
        match (self.s.deployers_owner(), proof, consent) {
            (None, Some(proof), None) if actor == OPERATOR => self.fresh(self.operator(proof)?),
            (None, ..) => Err("the operator grants this cluster's deployers, with its signature"),
            (Some(_), None, Some(consent)) => self.fresh(self.consent_of(actor, consent)?),
            (Some(_), ..) => Err("this cluster's owner grants its deployers, with a passkey"),
        }
    }

    /// The sign-up policy and sign-up invites: the same two parties.
    fn sign_up_proof(
        &self,
        actor: &str,
        proof: Option<&Value>,
        consent: Option<&Value>,
    ) -> Result<(crate::proof::Statement, String), &'static str> {
        match (self.s.deployers_owner(), proof, consent) {
            (None, Some(proof), None) if actor == OPERATOR => self.fresh(self.operator(proof)?),
            (None, ..) => Err("the operator runs this cluster's sign-up, with its signature"),
            (Some(_), None, Some(consent)) => self.fresh(self.consent_of(actor, consent)?),
            (Some(_), ..) => Err("this cluster's owner runs its sign-up, with a passkey"),
        }
    }
}

/// The area a [`SystemCmd`] belongs to: the module under `system/` that
/// judges its proof and applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Area {
    /// The fleet: nodes, quotas, pools, genesis, the cluster version ([`fleet`]).
    Fleet,
    /// Bundles, package releases, publishers and package origins ([`packages`]).
    Packages,
    /// The app directory: instances, upgrades, transfers, moves, replicas, restores ([`apps`]).
    Apps,
    /// The compute queue and file-byte charges ([`compute`]).
    Compute,
    /// Accounts, passkeys, sessions and API keys ([`accounts`]).
    Accounts,
    /// Account and organization recovery ([`recovery`]).
    Recovery,
    /// Release approvals, release deadlines and outbound approvals ([`approvals`]).
    Approvals,
    /// Deployers, the cluster's owner and deployments ([`deployments`]).
    Deployments,
    /// The sign-up policy and sign-up invites ([`sign_up`]).
    SignUp,
    /// Organizations, members, teams and invites ([`orgs`]).
    Orgs,
    /// Update policies and first approvals of red updates ([`updates`]).
    Updates,
}

impl SystemCmd {
    /// The area this command belongs to; every command names one.
    pub(crate) const fn area(&self) -> Area {
        match self {
            Self::RegisterNode { .. } => Area::Fleet,
            Self::RemoveNode { .. } => Area::Fleet,
            Self::PutBundle { .. } => Area::Packages,
            Self::InstallPackage { .. } => Area::Packages,
            Self::UninstallPackage { .. } => Area::Packages,
            Self::CreateApp { .. } => Area::Apps,
            Self::UpgradeApp { .. } => Area::Apps,
            Self::SetUpgradePolicy { .. } => Area::Updates,
            Self::ApproveUpgrade { .. } => Area::Updates,
            Self::UndoUpgrade { .. } => Area::Updates,
            Self::TransferApp { .. } => Area::Apps,
            Self::MoveApp { .. } => Area::Apps,
            Self::SetDelegation { .. } => Area::Apps,
            Self::LinkApp { .. } => Area::Apps,
            Self::DeleteApp { .. } => Area::Apps,
            Self::ChargeFileBytes { .. } => Area::Compute,
            Self::AcquireRun { .. } => Area::Compute,
            Self::ReleaseRun { .. } => Area::Compute,
            Self::SetReplicas { .. } => Area::Apps,
            Self::RestoreApp { .. } => Area::Apps,
            Self::FinishRestore { .. } => Area::Apps,
            Self::SetAccess { .. } => Area::Apps,
            Self::CreateAccount { .. } => Area::Accounts,
            Self::AddPasskey { .. } => Area::Accounts,
            Self::RemovePasskey { .. } => Area::Accounts,
            Self::RevokeAllSessions { .. } => Area::Accounts,
            Self::RefreshReused { .. } => Area::Accounts,
            Self::SetSignCount { .. } => Area::Accounts,
            Self::CreateApiKey { .. } => Area::Accounts,
            Self::RevokeApiKey { .. } => Area::Accounts,
            Self::SetAccountRecoveryKey { .. } => Area::Recovery,
            Self::RemoveAccountRecoveryKey { .. } => Area::Recovery,
            Self::StartAccountRecovery { .. } => Area::Recovery,
            Self::CancelAccountRecovery { .. } => Area::Recovery,
            Self::FinishAccountRecovery { .. } => Area::Recovery,
            Self::SetBackupKey { .. } => Area::Recovery,
            Self::ImportHolder { .. } => Area::Recovery,
            Self::CreateRefresh { .. } => Area::Accounts,
            Self::UseRefresh { .. } => Area::Accounts,
            Self::RevokeSession { .. } => Area::Accounts,
            Self::SetQuotas { .. } => Area::Fleet,
            Self::SetOrgQuotas { .. } => Area::Fleet,
            Self::SetPools { .. } => Area::Fleet,
            Self::SetPriceTable { .. } => Area::Fleet,
            Self::CreateOrg { .. } => Area::Orgs,
            Self::RenameOrg { .. } => Area::Orgs,
            Self::Genesis { .. } => Area::Fleet,
            Self::RequireSessionProofs { .. } => Area::Fleet,
            Self::AttributeByRelease => Area::Fleet,
            Self::RequireCurrentRules => Area::Fleet,
            Self::ExpireProofs => Area::Fleet,
            Self::SetClusterVersion { .. } => Area::Fleet,
            Self::SetOperatorRelease { .. } => Area::Approvals,
            Self::SetReleaseApproval { .. } => Area::Approvals,
            Self::BindPublisher { .. } => Area::Packages,
            Self::SetReleaseDeadline { .. } => Area::Approvals,
            Self::GrantDeployer { .. } => Area::Deployments,
            Self::RevokeDeployer { .. } => Area::Deployments,
            Self::SetDeploymentsGranted { .. } => Area::Deployments,
            Self::SetSignUp { .. } => Area::SignUp,
            Self::CreateSignUpInvite { .. } => Area::SignUp,
            Self::RevokeSignUpInvite { .. } => Area::SignUp,
            Self::SetClusterOwner { .. } => Area::Deployments,
            Self::CreateDeployment { .. } => Area::Deployments,
            Self::OfferApp { .. } => Area::Deployments,
            Self::WithdrawApp { .. } => Area::Deployments,
            Self::AddPackageOrigin { .. } => Area::Packages,
            Self::RemovePackageOrigin { .. } => Area::Packages,
            Self::DeleteDeployment { .. } => Area::Deployments,
            Self::SetDeploymentSecret { .. } => Area::Deployments,
            Self::ClearDeploymentSecret { .. } => Area::Deployments,
            Self::SetOutboundApproval { .. } => Area::Approvals,
            Self::SetEgress { .. } => Area::Approvals,
            Self::DeleteOrg { .. } => Area::Orgs,
            Self::SetOrgMetadata { .. } => Area::Orgs,
            Self::SetMemberMetadata { .. } => Area::Orgs,
            Self::SetAppMetadata { .. } => Area::Apps,
            Self::SetPasskeyLabel { .. } => Area::Accounts,
            Self::UpgradeFailed { .. } => Area::Apps,
            Self::StartUpgradeRun { .. } => Area::Apps,
            Self::FinishUpgradeRun { .. } => Area::Apps,
            Self::SetAlias { .. } => Area::Apps,
            Self::SetOrgRole { .. } => Area::Orgs,
            Self::RemoveOrgMember { .. } => Area::Orgs,
            Self::CreateTeam { .. } => Area::Orgs,
            Self::SetTeamMember { .. } => Area::Orgs,
            Self::SetTeamMembers { .. } => Area::Orgs,
            Self::UpdateTeam { .. } => Area::Orgs,
            Self::DeleteTeam { .. } => Area::Orgs,
            Self::SetCapabilities { .. } => Area::Orgs,
            Self::SetAllApps { .. } => Area::Orgs,
            Self::SetDefaultCapabilities { .. } => Area::Orgs,
            Self::RemoveTeamMember { .. } => Area::Orgs,
            Self::RemoveTeamMembers { .. } => Area::Orgs,
            Self::CreateInvite { .. } => Area::Orgs,
            Self::RedeemInvite { .. } => Area::Orgs,
            Self::RevokeInvite { .. } => Area::Orgs,
            Self::SetOrgSettings { .. } => Area::Orgs,
            Self::CreateAppInvite { .. } => Area::Apps,
            Self::RevokeAppInvite { .. } => Area::Apps,
            Self::RedeemAppInvite { .. } => Area::Apps,
            Self::CreatePublicLink { .. } => Area::Apps,
            Self::RevokePublicLink { .. } => Area::Apps,
            Self::ScopeApps { .. } => Area::Orgs,
            Self::SetRecoveryKey { .. } => Area::Recovery,
            Self::StartRecovery { .. } => Area::Recovery,
            Self::CancelRecovery { .. } => Area::Recovery,
            Self::FinishRecovery { .. } => Area::Recovery,
        }
    }
}

/// What a person consents to when creating an API key: every field of the
/// record but the account (the statement's subject), and the hash of the
/// key, so the log cannot record another key under the same consent.
#[must_use]
pub fn api_key_args(
    hash: &str,
    record: &ApiKeyRecord,
    then: &[crate::types::SignedFollowUp],
) -> Value {
    let mut args = json!({
        "hash": hash,
        "keyId": record.key_id,
        "label": record.label,
        "apps": record.apps,
        "methods": record.methods,
        "readOnly": record.read_only,
        "allowConfirm": record.allow_confirm,
        "expiresAt": record.expires_at,
    });
    // Only an upgrade key states its scope: every other key's consent reads
    // the bytes it did before upgrade keys.
    if let (Some(scope), Some(obj)) = (&record.upgrades, args.as_object_mut()) {
        let _prev = obj.insert("upgrades".to_owned(), json!(scope));
    }
    // Likewise `then`, stated only by a key made with follow-up calls.
    if let Some(obj) = args.as_object_mut().filter(|_| !then.is_empty()) {
        let _prev = obj.insert("then".to_owned(), json!(then));
    }
    args
}

impl SystemCmd {
    /// When the session a sign-in or a sign-up opens expires, as its passkey
    /// signed it ([`sign_in_args`], [`account_args`]): when its proof may be
    /// forgotten ([`SystemState::proof_expiry`]).
    #[must_use]
    pub const fn signed_session_expiry(&self) -> Option<u64> {
        match self {
            Self::CreateRefresh { record, .. } => Some(record.expires_at),
            Self::CreateAccount {
                refresh: Some(grant),
                ..
            } => Some(grant.record.expires_at),
            _ => None,
        }
    }

    /// The commands whose proofs a cluster requires only from
    /// [`SystemState::session_proofs_from`] on.
    #[must_use]
    pub const fn is_session_command(&self) -> bool {
        matches!(
            self,
            Self::CreateAccount { .. }
                | Self::CreateRefresh { .. }
                | Self::UseRefresh { .. }
                | Self::SetSignCount { .. }
                | Self::RevokeApiKey { .. }
        )
    }

    /// Read every flag that names an earlier release's rule as this release
    /// sets it (`true`): what an entry from [`SystemState::current_rules_from`]
    /// on applies under. Those flags default to `false` so entries an earlier
    /// release logged replay as it judged them; a later entry with one left
    /// `false` comes from no node of this release, which always sets them.
    pub fn hold_to_current_rules(&mut self) {
        match self {
            Self::UpgradeApp {
                owner_in_org,
                owner_in_team,
                ..
            }
            | Self::TransferApp {
                owner_in_org,
                owner_in_team,
                ..
            }
            | Self::DeleteApp {
                owner_in_org,
                owner_in_team,
                ..
            }
            | Self::SetAccess {
                owner_in_org,
                owner_in_team,
                ..
            }
            | Self::CreateApiKey {
                owner_in_org,
                owner_in_team,
                ..
            }
            | Self::SetAppMetadata {
                owner_in_org,
                owner_in_team,
                ..
            } => {
                *owner_in_org = true;
                *owner_in_team = true;
            }
            Self::ScopeApps {
                member_child_teams,
                by_release,
                all_apps_rows,
                ..
            } => {
                *member_child_teams = true;
                *by_release = true;
                *all_apps_rows = true;
            }
            Self::UpdateTeam {
                close_drops_below,
                all_apps_rows,
                ..
            } => {
                *close_drops_below = true;
                *all_apps_rows = true;
            }
            Self::MoveApp {
                accounts_in_app_team,
                all_apps_rows,
                ..
            } => {
                *accounts_in_app_team = true;
                *all_apps_rows = true;
            }
            Self::CreateOrg { name_fits, .. } => *name_fits = true,
            Self::CreateTeam {
                member_child_teams, ..
            } => *member_child_teams = true,
            Self::DeleteTeam {
                apps_need_consent, ..
            } => *apps_need_consent = true,
            Self::RemoveTeamMember { subtree, .. } => *subtree = true,
            Self::CreateInvite { teams_seen, .. } => *teams_seen = true,
            Self::RedeemInvite {
                free_when_unchanged,
                ..
            } => *free_when_unchanged = true,
            Self::FinishAccountRecovery {
                revoke_api_keys, ..
            } => *revoke_api_keys = true,
            _ => {}
        }
    }
}

/// A refresh token as the log keeps it: under its hash, with its record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshGrant {
    pub hash: String,
    pub record: RefreshRecord,
}

/// What a sign-in's challenge states (`session / __sign_in`): the refresh
/// token it opens, by hash and key, its session and its expiry. Made before
/// the passkey is asked, so it names no account: the passkey that signs it
/// names the account, and must be the one the record names.
#[must_use]
pub fn sign_in_args(hash: &str, record: &RefreshRecord) -> Value {
    json!({
        "refresh": hash,
        "key": record.key,
        "session": record.session,
        "expiresAt": record.expires_at,
    })
}

/// What a new account's passkey signs at sign-up (`account:<id> /
/// __create_account`): the whole account (its passkey, name and approved
/// release among it) and the session the sign-up opens.
#[must_use]
pub fn account_args(account: &Account, refresh: Option<&RefreshGrant>) -> Value {
    json!({ "account": account, "refresh": refresh })
}

/// What a person consents to when creating an invite: its public key
/// ([`crate::proof::invite_key`]) and its terms.
#[must_use]
pub fn invite_args(invite_key: &str, invite: &Invite) -> Value {
    let mut args = json!({
        "inviteKey": invite_key,
        "role": invite.role,
        "teams": invite.teams,
        "expiresAt": invite.expires_at,
        "uses": invite.uses_left,
    });
    // Named only when it is given, so an invite without it is consented to
    // in the bytes it was before.
    if invite.all_apps {
        args["allApps"] = json!(true);
    }
    if let Some(bits) = invite.capabilities {
        args["capabilities"] = json!(bits);
    }
    args
}

/// What a person consents to when giving a member every app added later, or
/// stopping it (`__set_all_apps`).
#[must_use]
pub fn all_apps_args(account: &str, all_apps: bool) -> Value {
    json!({ "account": account, "allApps": all_apps })
}

/// What a person consents to when setting default capabilities
/// (`__set_default_capabilities`): the organization's as before, or a team's.
#[must_use]
pub fn default_capabilities_args(team: Option<&str>, capabilities: u32) -> Value {
    match team {
        None => json!({ "capabilities": capabilities }),
        Some(t) => json!({ "team": t, "capabilities": capabilities }),
    }
}

/// The most teams one `SetTeamMembers` or `RemoveTeamMembers` names.
pub const MAX_TEAM_BATCH: usize = 64;

/// One team of a [`SystemCmd::SetTeamMembers`]: the row as a single
/// `SetTeamMember` would set it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamRow {
    pub team: TeamId,
    /// The row's role (absent: `member`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<TeamRole>,
    /// The row's capabilities (absent: a new row takes the organization's
    /// and the team's default; an existing row keeps its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<u32>,
}

/// What a person consents to when deleting a team whose subtree holds `apps`
/// (`org:<org> / __delete_team`): `{ team, apps }`, plus `keepApps: true` when those
/// apps stay without the team's grants (`DeleteTeam { keep_apps }`), so a deletion
/// consented as before reads as it did.
#[must_use]
pub fn delete_team_args(team: &str, apps: &[AppId], keep_apps: bool) -> Value {
    if keep_apps {
        json!({ "team": team, "apps": apps, "keepApps": true })
    } else {
        json!({ "team": team, "apps": apps })
    }
}

/// What a person consents to when setting one account's rows in several
/// teams (`org:<org> / __set_team_members`).
#[must_use]
pub fn team_members_args(account: &str, teams: &[TeamRow]) -> Value {
    json!({ "account": account, "teams": teams })
}

/// What a person consents to when taking one account's rows from several
/// teams, or leaving them (`org:<org> / __remove_team_members`).
#[must_use]
pub fn remove_team_members_args(account: &str, teams: &[TeamId]) -> Value {
    json!({ "account": account, "teams": teams })
}

/// The most packages one `ScopeApps` names.
pub const MAX_SCOPE_PACKAGES: usize = 64;
/// The most instances one package of a `ScopeApps` names.
pub const MAX_SCOPE_INSTANCES: usize = 1024;
/// The most accounts one package of a `ScopeApps` keeps on a close.
pub const MAX_SCOPE_KEEP: usize = 1024;
/// The longest app team name, in characters.
pub const MAX_APP_TEAM_NAME_CHARS: usize = 64;

/// An upgrade, upgrade run or upgrade key, under
/// [`cluster_version::Feature::OwnerApprovedUpgrades`], by an account that is not an
/// owner or admin of the app's organization.
pub const NOT_AN_UPGRADE_APPROVER: &str =
    "only an owner or admin of the app's organization approves its upgrades";
/// An `UpgradeApp { run }` the run does not name.
pub const NOT_IN_RUN: &str = "the upgrade is not one its run names";
/// An `UpgradeApp { run }` for an app the run already moved or failed.
pub const RUN_MOVED_APP: &str = "the run has upgraded this app already";
/// An `UpgradeApp { run }` for an app that no longer runs the run's package.
pub const RUN_OTHER_PACKAGE: &str = "the app no longer runs the run's package";
/// An `UpgradeApp { run }` that would move an app to an older release.
pub const RUN_OLDER_RELEASE: &str = "an upgrade run does not move an app to an older release";

/// How a logged command judges an app's owner ([`SystemState::owner_acts`]),
/// by the flags the release that logged it set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerRule {
    /// Ownership alone: entries from before `owner_in_org`.
    Ownership,
    /// While a member of the app's organization: `owner_in_org` alone.
    InOrg,
    /// And, for an app on a team, while in that team: `owner_in_team` too.
    InTeam,
}

impl OwnerRule {
    /// The rule this release judges by, and logs.
    pub const CURRENT: Self = Self::InTeam;

    /// The rule a command's flags name.
    #[must_use]
    pub const fn logged(owner_in_org: bool, owner_in_team: bool) -> Self {
        match (owner_in_org, owner_in_team) {
            (false, _) => Self::Ownership,
            (true, false) => Self::InOrg,
            (true, true) => Self::InTeam,
        }
    }
}

/// One package of a [`SystemCmd::ScopeApps`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeApp {
    pub package: String,
    /// The app team's name when one is made (absent: the package's display
    /// name, else the package).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The instances to move (absent: every organization-access instance of
    /// the package here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<Vec<AppId>>,
    /// Open (`everyone`) or close (`invite`) this package's app team in this
    /// step, and no other team (absent: as the step's `shared` says). Naming
    /// it on any package leaves `shared` unset to mean "no other team".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<TeamJoin>,
    /// Closing (`join: invite`) only: who keeps the app. Absent: everyone
    /// who reaches it now gets a row of their own, so nobody loses it. A
    /// list: the team's leads and those accounts (members of the
    /// organization) keep it, each with a row, and everyone else in it, by
    /// a row or by inheritance, loses it, with their rows in the teams below
    /// it (a private channel under the app's team), as a removal from the
    /// team takes them. They are not barred: opening it again lets them in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<Vec<AccountId>>,
    /// Whether the app team's members make restricted teams below it
    /// (private channels): `true` adds `CREATE_CHILD_TEAM` to its default
    /// capabilities and to every row there that sets its own, `false` takes
    /// it out of both (absent: as they are, or, for a team made here, as a
    /// new app team's default). An app team made before that default gets
    /// it only this way. Naming it on any package leaves `shared` unset to
    /// mean "no other team", as `join` does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_teams: Option<bool>,
}

/// What an admin consents to when scoping apps (`org:<org> / __scope_apps`):
/// the request as made; the teams and rows it leads to are judged at apply.
#[must_use]
pub fn scope_apps_args(apps: Option<&[ScopeApp]>, shared: Option<bool>) -> Value {
    json!({ "apps": apps, "shared": shared })
}

/// What a person consents to when moving an app to another organization
/// (`<app> / __move`).
#[must_use]
/// What `POST /apps/{app}/move` answers: where the app went, its access there, whether an app
/// team was made, the accounts brought in and the invites revoked.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMoved {
    app_id: String,
    from: String,
    org: String,
    grants: crate::types::Access,
    created: bool,
    added: Vec<String>,
    revoked_invites: u64,
}

pub fn move_app_args(to_org: &str, bring_members: bool) -> Value {
    json!({ "toOrg": to_org, "bringMembers": bring_members })
}

/// What a person consents to when setting an update policy
/// (`org:<org> / __set_upgrade_policy`): the app it is for (`null`: the organization's)
/// and the policy (`null`: dropped).
#[must_use]
pub fn upgrade_policy_args(
    app: Option<&str>,
    policy: Option<&crate::types::UpgradePolicy>,
) -> Value {
    json!({ "app": app, "policy": policy })
}

/// What a person consents to with the first approval of a red update
/// (`<app> / __approve_upgrade`): the release, each unit member's target, and the class
/// they were shown.
#[must_use]
pub fn approve_upgrade_args(
    package: &str,
    version: &str,
    targets: &BTreeMap<AppId, crate::types::RunTarget>,
    class: &crate::types::UpgradeClass,
) -> Value {
    json!({ "package": package, "version": version, "targets": targets, "class": class })
}

/// What a delegation's consent states (`<source> / __delegate`): the app it reaches and the
/// methods called there as the person.
#[must_use]
pub fn delegation_args(target: &str, methods: &[String]) -> Value {
    json!({ "target": target, "methods": methods })
}

/// What an attach's consent states (`<app> / __link`): the reference's name and the app
/// attached under it, `null` to detach.
#[must_use]
pub fn link_app_args(name: &str, target: Option<&str>) -> Value {
    json!({ "name": name, "target": target })
}

/// The most apps one instance has attached (as a release names at most this many references).
pub const MAX_ATTACHED: usize = calimero_bundle::MAX_USES;

/// The id a `ScopeApps` (or a `MoveApp`) with `nonce` gives a new app team of `package`: the
/// first of a short series not taken in `org` and not starting with `-`.
fn app_team_id(org: &Org, nonce: &str, package: &str) -> TeamId {
    (0u32..)
        .map(|i| {
            let h = calimero_identity::sha256(format!("{nonce}/{package}/{i}").as_bytes());
            calimero_identity::b64url(&h[..16])
        })
        .find(|id| !id.starts_with('-') && !org.teams.contains_key(id))
        .unwrap_or_default()
}

/// A change to the system group.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum SystemCmd {
    /// Into the directory: the operator's (`cluster / __register_node /
    /// { id, addr, zone }`). The nodes a cluster starts with come with
    /// `Genesis`.
    RegisterNode {
        node: NodeRecord,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    /// Out of the directory; refused while an app keeps a replica on it.
    RemoveNode {
        id: u64,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    PutBundle {
        hash: String,
        size: u64,
        actor: AccountId,
        /// When it was uploaded (Unix seconds): a deployer grant is judged
        /// at this time. Absent (0) in entries from before.
        #[serde(default)]
        at: u64,
    },
    /// Record a verified package release. The node that verified the `.mpk`
    /// proposes it; `apply` re-checks the signer pin and the quota.
    InstallPackage {
        package: String,
        signer_id: String,
        release: PackageVersion,
    },
    /// Remove a release (or every release) of a package no app runs.
    UninstallPackage {
        package: String,
        version: Option<String>,
        actor: AccountId,
    },
    CreateApp {
        app: Box<AppRecord>,
        /// A unit's member created from its instance's deployment (`app.deployment`): the
        /// release the deployment offers for the instance, whose offer covers the member's
        /// own release ([`Feature::WholeUnits`](cluster_version::Feature::WholeUnits)).
        /// Absent in an entry an earlier release logged, whose members carry no deployment.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_offer: Option<PackageRef>,
        /// Where deployments are granted, an instance from no deployment is a deployer's
        /// ([`SystemState::creates_outside_offers`]). Set once every node reads it
        /// ([`cluster_version::Feature::OffersOnly`]); absent in an earlier entry.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offers_only: bool,
    },
    UpgradeApp {
        app_id: AppId,
        bundle: String,
        /// The compute component of the release `bundle` is taken from, which only the
        /// caller knows when releases share an app component; `None` for a raw upload.
        #[serde(default)]
        compute_bundle: Option<String>,
        /// The operator upgraded it (`actor` empty): allowed only where the
        /// deployment gave it app upgrades, and then nobody else may.
        #[serde(default)]
        by_operator: bool,
        actor: AccountId,
        /// When the upgrade began (Unix seconds), for the app's upgrade
        /// record; release deadlines are judged at this time, the one the
        /// API judged them at before the migration ran.
        #[serde(default)]
        at: u64,
        /// The organization-wide run it belongs to.
        #[serde(default)]
        run: Option<String>,
        /// The owner's passkey assertion over this upgrade.
        #[serde(default)]
        consent: Value,
        /// The release the app moves to (`AppRecord::release`), which must
        /// ship `bundle`; it may be one with the app's own bundle (a
        /// frontend-only release). `None` for a raw upload.
        #[serde(default)]
        release: Option<PackageRef>,
        /// An organization's app is managed by its owner only while they are
        /// a member of the organization ([`SystemState::owner_acts`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays judging ownership alone.
        #[serde(default)]
        owner_in_org: bool,
        /// An app on a team is reached and managed by its owner only while
        /// they are in that team ([`SystemState::owner_acts`]). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays judging the owner without the team.
        #[serde(default)]
        owner_in_team: bool,
        /// An instance from a deployment moves only to the release its
        /// deployment offers ([`SystemState::within_deployment`]). The API
        /// sets it once every node reads it
        /// ([`cluster_version::Feature::UpgradeInDeployment`]); absent
        /// (`false`) in an earlier release's entry, which replays as it
        /// applied unless the logged cluster version brings the rule.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        in_deployment: bool,
        /// Only an owner or admin of the app's organization approves its upgrades
        /// ([`SystemState::approves_upgrades`]). The API sets it once every node reads it
        /// ([`cluster_version::Feature::OwnerApprovedUpgrades`]); absent (`false`) in an
        /// entry an earlier release logged, which replays as it applied, unless the logged
        /// cluster version brings the rule ([`SystemState::cluster_brings`]).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        owner_approves: bool,
        /// What the classifier said of this app's move (`calimero_bundle::update`, the app
        /// and the members it has, as they run when it moves), computed where it was
        /// proposed. Apply re-derives what the log holds (each release's signer, its
        /// `outbound` and secret hosts) and refuses a class
        /// that leaves out a reason the log shows ([`SystemState::check_class`]), so a red
        /// update is never logged as green; it is recorded on the app's `last_upgrade`.
        /// The API sets it once every node reads it
        /// ([`cluster_version::Feature::UpgradeClasses`]); absent in an entry an earlier
        /// release logged, for a raw upload, and where the release could not be read,
        /// which every rule then takes as red.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<crate::types::UpgradeClass>,
        /// It applies on its own under the app's update policy, proposed by the system
        /// leader (`node::auto_updates`) with no person's consent: `actor` is
        /// [`AUTO_UPDATE`], and apply judges the policy, the class, the hold and the
        /// destinations again from the log
        /// ([`SystemState::auto_update_gate`]). [`cluster_version::Feature::AutoUpdates`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        auto: bool,
        /// An update on its own leaves the unit's members where they are, so it is refused
        /// where its release would move them ([`SystemState::moves_members`]). Set with
        /// `auto` once every node reads it ([`cluster_version::Feature::AutoUpdateMembersStay`]);
        /// absent in an earlier entry, which replays as it applied unless the logged cluster
        /// version brings the rule.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        members_stay: bool,
        /// The restore point taken just before the upgrade ran
        /// ([`crate::types::TakenRestorePoint`]): recorded as the app's
        /// `restore_point`, which its owner or an admin may undo for
        /// [`crate::types::UNDO_WINDOW_SECS`]. Absent: the app keeps no restore point
        /// after it. [`cluster_version::Feature::RestorePoints`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        restore_point: Option<crate::types::TakenRestorePoint>,
    },
    /// Set the update policy of an organization (`app` absent) or of one of its apps, or
    /// drop it (`policy` absent: an app then follows its organization's, an organization
    /// the defaults): an owner or admin of the organization
    /// ([`SystemState::approves_upgrades`]), with passkey consent over
    /// `org:<org> / __set_upgrade_policy / ` [`upgrade_policy_args`]. `hold_hours` outside
    /// `MIN_HOLD_HOURS..=MAX_HOLD_HOURS` is refused ([`BAD_UPGRADE_POLICY`]).
    SetUpgradePolicy {
        org: OrgId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<AppId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<crate::types::UpgradePolicy>,
        actor: AccountId,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    /// The first approval of a red update of `app_id` where its policy asks for a second
    /// approver: an owner or admin of its organization, with passkey consent over
    /// `<app> / __approve_upgrade / ` [`approve_upgrade_args`]. Logged as
    /// [`crate::types::PendingApproval`]; another owner or admin then applies the update
    /// (`UpgradeApp`, [`SystemState::update_gate`]).
    ApproveUpgrade {
        app_id: AppId,
        package: String,
        version: String,
        /// A unit's upgrade: each member it moves to its own release.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        targets: BTreeMap<AppId, crate::types::RunTarget>,
        /// The class the approver was shown, judged against the log as `UpgradeApp`'s is.
        class: crate::types::UpgradeClass,
        actor: AccountId,
        /// When (Unix seconds), from the proposing node's clock.
        at: u64,
        #[serde(default)]
        consent: Value,
    },
    /// Undo `app_id`'s last upgrade within [`crate::types::UNDO_WINDOW_SECS`] of it
    /// (judged at `at`): an owner or admin of its organization, with passkey consent
    /// over `<app> / __undo_upgrade / ` [`SystemState::undo_args`]. Every app in `apps`
    /// (the instance, and the members its unit's upgrade moved in the same run) goes back
    /// to the release it ran before and, where the upgrade ran on its group, into a new
    /// group seeded from its restore point by the node that holds it (`apps[id][0]`),
    /// as `RestoreApp` does: whatever it wrote since is gone. The one move to an older
    /// release. [`cluster_version::Feature::RestorePoints`].
    UndoUpgrade {
        app_id: AppId,
        /// Each app it reverts, with its new group's replicas (the restore point's
        /// holder first), or none for an app whose upgrade ran nothing on its group.
        apps: BTreeMap<AppId, Vec<u64>>,
        actor: AccountId,
        /// When (Unix seconds), from the proposing node's clock.
        at: u64,
        #[serde(default)]
        consent: Value,
        /// The release it returns to must still be installed ([`SystemState::undo_release_installed`]);
        /// set once every node reads it ([`cluster_version::Feature::UndoToInstalledRelease`]).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        release_installed: bool,
    },
    TransferApp {
        app_id: AppId,
        to: AccountId,
        actor: AccountId,
        /// The actor's passkey assertion over this transfer.
        #[serde(default)]
        consent: Value,
        /// An organization's app is managed by its owner only while they are
        /// a member of the organization ([`SystemState::owner_acts`]), and
        /// an organization admin transfers it too ([`SystemState::manages_app`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays as that release judged it: the
        /// owner alone, member or not, and no admin.
        #[serde(default)]
        owner_in_org: bool,
        /// An app on a team is reached and managed by its owner only while
        /// they are in that team ([`SystemState::owner_acts`]). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays judging the owner without the team.
        #[serde(default)]
        owner_in_team: bool,
    },
    /// Move an app instance to another organization, its data where it is:
    /// whoever manages it where it is now ([`SystemState::manages_app`]; a
    /// personal app's owner) and is an admin or the owner of `to_org`, with
    /// their passkey consent (`<app> / __move` / [`move_app_args`]). Its
    /// files, compute runs and quotas go to the target's payer; its access to
    /// the target's app team of its package (organization access in a legacy
    /// target); `bring_members` adds the people of its organization who
    /// reach it to the target too. `SystemState::move_app` has the rules.
    MoveApp {
        app_id: AppId,
        to_org: OrgId,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
        /// Bring the source organization's members who reach the app: into
        /// the target (at their role there, capped at member), and into the
        /// target app team. Absent: `false`.
        #[serde(default)]
        bring_members: bool,
        /// Random, from the API: a new app team's id derives from it
        /// (`app_team_id`), so every replica makes the same one.
        #[serde(default)]
        nonce: String,
        /// An app team made in the target gives a row to each of its members
        /// who gets every app added later ([`crate::types::Org::all_apps`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays filling the rows it filled then.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_apps_rows: bool,
        /// An instance only named people reach whose package has a closed app team in the
        /// target is reached there only through that team
        /// ([`SystemState::may_reach`]): who comes along with it, and its
        /// owner, get a row on that team. The API always sets it; absent
        /// (`false`) in an entry an earlier release logged, which replays
        /// giving no such rows, as that release did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        accounts_in_app_team: bool,
        /// The target's quotas are its own override where it has one
        /// ([`SystemState::payer_quotas`]), as every other charge judges
        /// them. The API always sets it; absent (`false`) in an entry an
        /// earlier release logged, which replays judging the cluster's
        /// limits, as that release did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        org_quotas: bool,
    },
    DeleteApp {
        app_id: AppId,
        actor: AccountId,
        /// An organization's app is managed by its owner only while they are
        /// a member of the organization ([`SystemState::owner_acts`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays judging ownership alone.
        #[serde(default)]
        owner_in_org: bool,
        /// An app on a team is reached and managed by its owner only while
        /// they are in that team ([`SystemState::owner_acts`]). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays judging the owner without the team.
        #[serde(default)]
        owner_in_team: bool,
        /// The whole-unit rule ([`Feature::WholeUnits`](cluster_version::Feature::WholeUnits)):
        /// the apps the instance's release created with it, theirs included
        /// ([`SystemState::unit_of`]), deleted with it; the entry names exactly them, and a
        /// unit's member is not deleted alone ([`UNIT_MEMBER`]). Absent in an entry an
        /// earlier release logged, which replays deleting the app alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        members: Option<Vec<AppId>>,
    },
    /// Counts `delta` bytes more (less, when negative) of files against an app's payer. A
    /// charge past `Quotas::max_file_bytes_per_org` is refused; a refund stops at zero.
    ChargeFileBytes {
        app_id: AppId,
        delta: i64,
    },
    /// Queues a compute run of `app` and grants what the job nodes and quotas allow; a queued run
    /// answers its state. `payer` is the app's ([`crate::usage::payer`]).
    AcquireRun {
        app: AppId,
        run: String,
        payer: String,
        fuel: u64,
        /// The run's memory cap, which picks its pool; absent (0) from a leader that sends none.
        #[serde(default)]
        memory_mib: u32,
        /// A pipeline step's named pool: the run goes there (it must fit `memory_mib`) instead
        /// of by memory.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pool: Option<String>,
    },
    /// Frees a run's place (finished, or never to run) and grants the next waiting runs.
    ReleaseRun {
        app: AppId,
        run: String,
    },
    /// The operator moves an app (`<app> / __set_replicas / { replicas }`,
    /// the sorted target): one signature covers the move's two writes
    /// ([`authorize`]).
    SetReplicas {
        app_id: AppId,
        replicas: Vec<u64>,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    /// The operator restores an app from its sealed backup
    /// (`<app> / __restore_app / { backup, sha256 }`, `crate::backup`): the
    /// app moves to a new group, which `replicas[0]` seeds from the backup
    /// before it adds the others ([`crate::types::PendingRestore`]), and runs
    /// the bundle the backup's state names.
    RestoreApp {
        app_id: AppId,
        replicas: Vec<u64>,
        bundle: String,
        #[serde(default)]
        compute_bundle: Option<String>,
        source: crate::types::RestoreSource,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    /// The seed of a restore holds the backup in `group` on every replica:
    /// the app takes calls again.
    FinishRestore {
        app_id: AppId,
        group: u64,
    },
    /// Replace an app's grants ([`Access`], as `grants`).
    SetAccess {
        app_id: AppId,
        #[serde(rename = "grants")]
        access: Access,
        actor: AccountId,
        /// An organization's app is managed by its owner only while they are
        /// a member of the organization ([`SystemState::owner_acts`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays judging ownership alone.
        #[serde(default)]
        owner_in_org: bool,
        /// An app on a team is reached and managed by its owner only while
        /// they are in that team ([`SystemState::owner_acts`]). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays judging the owner without the team.
        #[serde(default)]
        owner_in_team: bool,
    },
    /// A new account, with the session its sign-up opens. Carries the new
    /// passkey's assertion over [`account_args`] (`account:<id> /
    /// __create_account`): only the passkey's holder makes the account it
    /// opens, with the release and the session it states. Judged against the
    /// cluster's sign-up policy ([`Governance::sign_up`]); `invite`: an
    /// organization invite (the account joins it) or a cluster sign-up
    /// invite, spent once here.
    CreateAccount {
        account: Account,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invite: Option<SignUpRedemption>,
        /// The sign-up's first refresh token.
        #[serde(default)]
        refresh: Option<RefreshGrant>,
        /// The new passkey's consent ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
        /// When the sign-up was taken (Unix seconds): the passkey's added
        /// and last sign-in time and the session's opening
        /// ([`crate::types::PasskeyTimes`]). Outside what the passkey signs,
        /// so a release that does not know it checks the same consent. 0 (a
        /// sign-up an earlier release logged): unknown.
        #[serde(default, skip_serializing_if = "is_zero")]
        at: u64,
        /// The account's personal organization, made with it
        /// ([`crate::types::Org::personal`]): every account has one from
        /// sign-up, so every app it makes is in an organization. Outside what
        /// the passkey signs, as `at` is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        personal_org: Option<PersonalOrgSeed>,
    },
    /// A new passkey, approved by one the account already has.
    AddPasskey {
        account: AccountId,
        credential: Credential,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
        /// When it was added (Unix seconds); 0 (an earlier release's
        /// entry): unknown.
        #[serde(default, skip_serializing_if = "is_zero")]
        at: u64,
    },
    /// Remove a passkey and revoke every session opened with it.
    RemovePasskey {
        account: AccountId,
        credential_id: String,
        #[serde(default)]
        now: u64,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    /// Revoke every session of `actor` (sign out everywhere).
    RevokeAllSessions {
        actor: AccountId,
        now: u64,
    },
    /// An exchanged refresh token came back: revoke its session.
    RefreshReused {
        hash: String,
        now: u64,
    },
    /// A passkey's counter moved: carries the assertion that moved it,
    /// which only the authenticator signs.
    SetSignCount {
        credential_id: String,
        count: u32,
        #[serde(default)]
        assertion: Value,
    },
    CreateApiKey {
        hash: String,
        record: ApiKeyRecord,
        /// The account's passkey consent over [`api_key_args`].
        #[serde(default)]
        consent: Value,
        /// For an upgrade key: an organization's app is upgraded by its owner
        /// only while they are a member of the organization
        /// ([`SystemState::owner_acts`]). The API always sets it; absent
        /// (`false`) in an entry an earlier release logged, which replays
        /// judging ownership alone.
        #[serde(default)]
        owner_in_org: bool,
        /// An app on a team is reached and managed by its owner only while
        /// they are in that team ([`SystemState::owner_acts`]). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays judging the owner without the team.
        #[serde(default)]
        owner_in_team: bool,
        /// Only an owner or admin of the app's organization approves its upgrades, and so makes or uses an upgrade key for it
        /// ([`SystemState::approves_upgrades`]). The API sets it once every node reads it
        /// ([`cluster_version::Feature::OwnerApprovedUpgrades`]); absent (`false`) in an
        /// entry an earlier release logged, which replays as it applied, unless the logged
        /// cluster version brings the rule ([`SystemState::cluster_brings`]).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        owner_approves: bool,
        /// The confirm calls the consent approved with the key, run before it
        /// ([`api_key_args`]'s `then`; [`cluster_version::Feature::KeyFollowUps`]).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        then: Vec<crate::types::SignedFollowUp>,
    },
    RevokeApiKey {
        key_id: String,
        actor: AccountId,
        /// The actor's passkey consent (`account:<a> / __revoke_api_key /
        /// { keyId }`, [`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    /// Set the account's recovery code, replacing any it had: recorded
    /// under the code's public key ([`crate::proof::recovery_key`]).
    SetAccountRecoveryKey {
        account: AccountId,
        recovery_key: String,
        /// The account's passkey consent over `{ recoveryKey }`
        /// (`account:<a> / __set_recovery`).
        consent: Value,
    },
    /// Remove the account's recovery code.
    RemoveAccountRecoveryKey {
        account: AccountId,
        /// The account's passkey consent (`account:<a> / __remove_recovery
        /// {}`).
        consent: Value,
    },
    /// Start recovering an account that lost its passkeys: spend the code
    /// `recovery_key` stands for and log `credential` as the passkey it may
    /// add, [`crate::types::RECOVERY_DELAY_SECS`] after `at`, unless one of
    /// the account's passkeys cancels first. `signature` is the code key's
    /// ([`crate::proof::sign_recover`] over the account, the credential and
    /// `at`), made by the node the person typed the code into.
    StartAccountRecovery {
        recovery_key: String,
        account: AccountId,
        credential: Credential,
        at: u64,
        signature: String,
    },
    /// Cancel the recovery under way for `account` that would add
    /// `credential_id`: a passkey the account already has, with its consent
    /// (`account:<a> / __cancel_recovery / { credentialId }`).
    CancelAccountRecovery {
        account: AccountId,
        credential_id: String,
        consent: Value,
    },
    /// Finish a recovery once its delay passed, judged at `at`: add the
    /// passkey the start logged, end every session the account had, revoke
    /// its API keys (`revoke_api_keys`), and open `refresh`. `consent` is
    /// that passkey's sign-in assertion over [`sign_in_args`] of `refresh`
    /// (`session / __sign_in`), as `CreateRefresh` carries.
    FinishAccountRecovery {
        account: AccountId,
        at: u64,
        refresh: RefreshGrant,
        consent: Value,
        /// Revoke every API key of the account too. Absent (`false`) in a
        /// finish an earlier release logged: replayed, it keeps the keys, as
        /// that release did, so every replica reaches the same state.
        #[serde(default)]
        revoke_api_keys: bool,
    },
    /// Bring an organization's or account's apps back from its recovery files
    /// (`POST /recover/holder`) on a cluster that does not hold them: the
    /// holder's slice (`slice`, its org or account, the accounts of its
    /// people with their passkeys, its apps' records, bundles and releases)
    /// is added, each app in `apps` gets a new group with `seed` (the node
    /// that holds its file) as its first replica and waits there for it
    /// (`PendingRestore`, as `RestoreApp`), and the holder's backup key is the
    /// one its files were sealed to. Refused where the operator upgrades apps,
    /// or where the org, account, an app or a passkey is here already. `proof`
    /// is the operator's signature over `cluster / __recover_holder /`
    /// [`import_holder_args`]: the cluster admits it; the keys that opened
    /// the files came from the holder.
    ImportHolder {
        holder: BackupHolder,
        slice: Box<HolderSlice>,
        /// The SHA-256 of the slice file's body, as the proof names it.
        slice_sha256: String,
        public_key: String,
        seed: u64,
        apps: Vec<ImportedApp>,
        at: u64,
        proof: Value,
    },
    /// Set (or replace) the public key the recovery disk encrypts `holder`'s
    /// backups to ([`SystemState::backup_keys`]). `proof` is the operator's
    /// signature for [`BackupHolder::Operator`], else `actor`'s passkey
    /// consent; either over `<holder key> / __set_backup_key / { publicKey }`.
    /// Judged at apply by [`SystemState::may_set_backup_key`].
    SetBackupKey {
        holder: BackupHolder,
        public_key: String,
        actor: AccountId,
        at: u64,
        proof: Value,
    },
    /// A sign-in's refresh token: carries the sign-in's assertion, whose
    /// challenge states [`sign_in_args`] (`session / __sign_in`).
    CreateRefresh {
        hash: String,
        record: RefreshRecord,
        #[serde(default)]
        consent: Value,
    },
    /// A refresh token exchanged for the next: signed with the old token's
    /// key ([`crate::proof::refresh_use_message`]), which only its holder has.
    UseRefresh {
        old_hash: String,
        new_hash: String,
        record: RefreshRecord,
        now: u64,
        #[serde(default)]
        signature: String,
    },
    RevokeSession {
        session: String,
        actor: AccountId,
        #[serde(default)]
        now: u64,
    },
    SetQuotas {
        quotas: Quotas,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    /// The operator sets one payer's quota overrides (`cluster / __set_org_quotas / { payer,
    /// quotas }`); all `None` removes them.
    SetOrgQuotas {
        payer: String,
        quotas: OrgQuotas,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    /// The operator sets the compute pools (`cluster / __set_pools / { pools }`).
    SetPools {
        pools: BTreeMap<String, PoolConfig>,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    /// The operator sets the price table (`cluster / __set_price_table / { table }`).
    SetPriceTable {
        table: PriceTable,
        /// The operator's signature over it ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    CreateOrg {
        org_id: OrgId,
        name: String,
        actor: AccountId,
        /// The release of the node the org was created through: approved from
        /// the start, so its apps run where the org began.
        #[serde(default)]
        release: Option<String>,
        /// The organization's recovery key (base64url Ed25519 public key),
        /// if the creator set one ([`crate::types::OrgRecovery`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery_key: Option<String>,
        /// When it was created (Unix seconds; the recovery key's `setAt`).
        #[serde(default)]
        at: u64,
        /// The name must fit ([`org_name_fits`]). The API always sets it;
        /// absent (`false`) in an entry an earlier release logged, which
        /// replays taking any name, as that release did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        name_fits: bool,
        /// The actor's personal organization ([`crate::types::Org::personal`]):
        /// at most one per account, not counted against
        /// `maxOrgsPerAccount`, and it starts with the release and outbound
        /// approvals the account holds for apps outside an organization, so
        /// an instance created there runs where one outside would.
        /// Left out when false.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        personal: bool,
        /// A package whose app team the new organization starts with, made
        /// in the same entry as `ScopeApps` makes one for a package with no
        /// instances yet, the creator its lead (the consent page's
        /// `createOrg` asked from that package's frontend). Left out when
        /// none ([`cluster_version::Feature::OrgAppTeam`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_team: Option<String>,
        /// The organization-wide capabilities its members hold unless set for
        /// them ([`crate::types::Org::default_capabilities`]), set in this
        /// entry. Left out when none
        /// ([`cluster_version::Feature::OrgCapabilities`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<u32>,
    },
    /// Rename an organization: its owner or an admin, with their passkey
    /// consent (`org:<org> / __rename_org / { name }`), to a name that fits
    /// as a new one's must ([`org_name_fits`]). Every reader of the name
    /// (the organization, `/auth/me`, an invite redeemed later) answers the
    /// new one: nothing else keeps a copy.
    RenameOrg {
        org: OrgId,
        name: String,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// The cluster's rules, from the first leader's configuration. Accepted
    /// once, before anyone has an account: after that they are fixed, so an
    /// operator cannot change who upgrades the apps once people rely on it.
    Genesis {
        governance: Governance,
        /// The founding node's release: where the operator holds release
        /// approvals, it holds the apps until the operator approves another.
        #[serde(default)]
        release: Option<String>,
        /// The operator's verify key (`--operator-key`), if configured.
        #[serde(default)]
        operator_key: Option<crate::proof::OperatorKey>,
        /// The nodes the cluster starts with (`--peers`): every later node
        /// joins through the operator's signed `RegisterNode`.
        #[serde(default)]
        nodes: Vec<NodeRecord>,
        /// Session commands carry their proofs from genesis on
        /// ([`SystemState::session_proofs_from`]). Absent (`false`) in a
        /// genesis an earlier release logged: replayed, it requires nothing.
        #[serde(default)]
        session_proofs: bool,
    },
    /// From this entry on, the session commands carry their proofs
    /// ([`SystemState::session_proofs_from`]). Written once by the first
    /// system leader on a release that makes them, in a cluster begun before
    /// (genesis sets it in a new one). It only ever tightens: a node that
    /// logs it early takes nothing from anyone.
    RequireSessionProofs,
    /// From this entry on, an app is judged by the release it runs
    /// ([`SystemState::attributes_by_release`]). Written once by the first
    /// system leader on a release that judges so; entries before it replay
    /// as they applied.
    AttributeByRelease,
    /// From this entry on, every command applies under this release's rules
    /// ([`SystemState::current_rules_from`]): the flags an earlier release
    /// left `false` to name its older rule (`owner_in_org`, `owner_in_team`,
    /// `apps_need_consent`, `teams_seen`, ...) are read as set, and an
    /// organization's upgrade run is derived from the system state
    /// (`StartUpgradeRun`). Written once by the first system leader on a
    /// release that makes it. It only ever tightens: a node that logs it
    /// early takes nothing from anyone.
    RequireCurrentRules,
    /// From this entry on, a sign-in's or sign-up's proof is forgotten once
    /// the session it opened expires ([`SystemState::expires_proofs`]), and a
    /// sign-in whose session expired by then is refused instead. Written once
    /// by the first system leader on a release that makes it; entries before
    /// it replay as they applied.
    ExpireProofs,
    /// The system leader raises the active cluster version
    /// ([`SystemState::cluster_version`]) to `version`, once every registered
    /// node advertises it or newer (`nodes`: each one's, by id). Only forward;
    /// no proof: like `RequireSessionProofs`, it is the nodes' own.
    SetClusterVersion {
        version: String,
        /// As `[[id, version], ...]`: `SystemCmd` is internally tagged, and
        /// serde reads such an enum through a buffer that keeps a JSON
        /// object's keys as strings, which a `u64` key does not take.
        #[serde(with = "cluster_version::node_versions")]
        nodes: BTreeMap<u64, String>,
    },
    /// Where the operator holds release approvals, it approves (or
    /// withdraws) a mero-tee release for every app's data.
    SetOperatorRelease {
        version: String,
        approved: bool,
        /// The operator's signature over this decision ([`crate::proof`]).
        #[serde(default)]
        proof: Value,
    },
    /// A mero-tee release is approved (or withdrawn) for an org's apps by
    /// whoever holds `APPROVE_RELEASES` there, or by an account for its own
    /// apps outside one. Logged with the passkey consent.
    SetReleaseApproval {
        scope: ReleaseScope,
        version: String,
        approved: bool,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Bind a package signing key to an organization: the key's own
    /// signature over [`publisher_binding_message`] at the key's current
    /// generation, proposed by a member of that organization holding
    /// `APPROVE_RELEASES`, with a passkey assertion. Binding again (to
    /// another organization) takes a new signature from the same key; the
    /// organization it leaves cannot keep it.
    BindPublisher {
        signer_id: String,
        org: OrgId,
        /// The cluster's name, as the signature binds it.
        cluster: String,
        proof: PublisherProof,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// The operator sets (`Some`, Unix seconds) or lifts (`None`) the
    /// deadline after which a scope that has not approved `version` (or a
    /// newer release) starts no new work. Availability only: it moves no data
    /// and blocks no approval.
    SetReleaseDeadline {
        version: String,
        after: Option<u64>,
        /// The operator's signature over this deadline
        /// (`cluster / __release_deadline / { version, after }`).
        #[serde(default)]
        proof: Value,
    },
    /// Let `deployer` install apps (until `until`, Unix seconds, if set)
    /// where deployments are granted. Granted by the operator (`actor`
    /// [`OPERATOR`], its signature in `proof`) or, where the cluster's owner
    /// grants deployers ([`SystemState::deployers_owner`]), by an account
    /// acting for it (its passkey `consent`), never the other one. Granting
    /// again replaces the grant.
    GrantDeployer {
        deployer: Deployer,
        #[serde(default)]
        until: Option<u64>,
        /// When it was granted (Unix seconds).
        at: u64,
        actor: AccountId,
        /// `cluster / __grant_deployer / { deployer, until }`, signed with
        /// the operator key.
        #[serde(default)]
        proof: Option<Value>,
        /// The same statement, as the owner-side account's passkey consent.
        #[serde(default)]
        consent: Option<Value>,
    },
    /// End `deployer`'s grant: it installs nothing new; what it installed,
    /// and the apps running it, stay. Authorized as [`SystemCmd::GrantDeployer`]
    /// (`__revoke_deployer`, `{ deployer }`).
    RevokeDeployer {
        deployer: Deployer,
        at: u64,
        actor: AccountId,
        #[serde(default)]
        proof: Option<Value>,
        #[serde(default)]
        consent: Option<Value>,
    },
    /// The operator moves the cluster's deployments from `open` to `granted`
    /// (`cluster / __deployments_granted / {}`): once, never back. It changes
    /// the governance every receipt key attests.
    SetDeploymentsGranted {
        #[serde(default)]
        proof: Value,
    },
    /// Set the cluster's sign-up policy (`cluster / __sign_up / { policy }`):
    /// the operator's signature (`proof`) until the cluster's owner is named,
    /// then the owner-side account's passkey (`consent`), never the other.
    /// It changes the governance every receipt key attests.
    SetSignUp {
        policy: SignUp,
        actor: AccountId,
        #[serde(default)]
        proof: Option<Value>,
        #[serde(default)]
        consent: Option<Value>,
    },
    /// A cluster sign-up invite, recorded under its token's public key (the
    /// token is never logged), authorized as [`SystemCmd::SetSignUp`] over
    /// [`sign_up_invite_args`] (`__create_sign_up_invite`).
    CreateSignUpInvite {
        invite_key: String,
        invite: SignUpInvite,
        actor: AccountId,
        #[serde(default)]
        proof: Option<Value>,
        #[serde(default)]
        consent: Option<Value>,
    },
    /// Withdraw a cluster sign-up invite (`__revoke_sign_up_invite`,
    /// `{ inviteKey }`); accounts it created stay.
    RevokeSignUpInvite {
        invite_key: String,
        actor: AccountId,
        #[serde(default)]
        proof: Option<Value>,
        #[serde(default)]
        consent: Option<Value>,
    },
    /// The operator names the cluster's owner where governance says
    /// `deployers: owner` (`cluster / __cluster_owner / { owner }`): once; it
    /// grants deployers from then on.
    SetClusterOwner {
        owner: Deployer,
        #[serde(default)]
        proof: Value,
    },
    /// An empty deployment of `owner`, created by an account acting for it
    /// (`cluster / __create_deployment / { name, owner }`, its passkey
    /// `consent`); `id` is [`deployment_id`] of that consent's nonce. Where
    /// deployments are granted, `actor` needs a live deployer grant at `at`.
    CreateDeployment {
        id: String,
        name: String,
        owner: Deployer,
        actor: AccountId,
        at: u64,
        #[serde(default)]
        consent: Value,
    },
    /// Offer `package@version`, installed already, as `app` in `deployment`,
    /// adding the entry or moving it to another release of its package
    /// (`deployment:{id} / __offer_app / { app, package, version }`). By a
    /// manager who sees the release, with a live grant where deployments are
    /// granted.
    OfferApp {
        deployment: String,
        app: String,
        package: String,
        version: String,
        actor: AccountId,
        at: u64,
        #[serde(default)]
        consent: Value,
    },
    /// Remove `app` from `deployment` (`__withdraw_app / { app }`): refused
    /// while an instance created from the deployment runs its package. The
    /// release stays installed.
    WithdrawApp {
        deployment: String,
        app: String,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Register `origin` for `package`'s frontend, its ceremonies verified
    /// against `rp_id` (`package:{package} / __add_origin / { origin, rpId }`,
    /// the manager's passkey `consent`; [`crate::origins`]). `served`: the
    /// cluster serves the frontend on it too (a custom domain; the consent
    /// then names `served: true`). Left out when false, so a command from
    /// before reads as it did.
    AddPackageOrigin {
        package: String,
        origin: String,
        rp_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        served: bool,
        actor: AccountId,
        at: u64,
        #[serde(default)]
        consent: Value,
    },
    /// Remove a registered origin (`package:{package} / __remove_origin /
    /// { origin }`): the sessions opened there end with it.
    RemovePackageOrigin {
        package: String,
        origin: String,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Remove a deployment that offers nothing and no instance came from
    /// (`__delete_deployment / {}`). Its secret values go with it.
    DeleteDeployment {
        deployment: String,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// A manager of `deployment` sets its value of secret `name`, which a
    /// release it offers declares (`deployment:{id} / __set_secret / { name,
    /// fingerprint }`, the manager's passkey `consent`: the value's
    /// fingerprint, never the value). Every instance created from it uses the
    /// value unless it set its own. `value` never shows in `Debug`.
    SetDeploymentSecret {
        deployment: String,
        name: String,
        value: crate::component::SecretValue,
        actor: AccountId,
        at: u64,
        #[serde(default)]
        consent: Value,
        /// The name may also be declared by a release an offered release references
        /// (`SystemState::deployment_uses_declare`), for the apps it creates
        /// (`Feature::DeploymentUsedSecrets`). `false` (and every entry from before): the
        /// offered releases alone, as before. Left out when `false`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        used: bool,
    },
    /// A manager removes the deployment's value of `name`
    /// (`deployment:{id} / __clear_secret / { name }`): its instances fall
    /// back to the operator's.
    ClearDeploymentSecret {
        deployment: String,
        name: String,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// The owner approves URL prefixes for a package's publisher to call out
    /// to, in an org's apps or their own outside one. `selection` is the latest release's declared
    /// list or chosen prefixes; `replace` sets exactly those (how a prefix is
    /// revoked), else they are added. Logged with the passkey consent.
    SetOutboundApproval {
        scope: ReleaseScope,
        package: String,
        selection: calimero_primitives::outbound::OutboundSelection,
        #[serde(default)]
        replace: bool,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Turn egress off (`egress: false`) or back on for every app of an organization, or
    /// with `app` for that instance alone ([`crate::types::Org::egress_off`],
    /// [`crate::types::AppRecord::egress_off`]): the organization's owner or an admin, with
    /// passkey consent (`org:<org> / __set_egress / { app, egress }`). Off, no outbound
    /// request leaves the cluster for the apps it covers, over every approval, and the
    /// organization approves no new outbound host. Not gated: a node of an earlier release
    /// cannot decode it (a hard cutover).
    SetEgress {
        org: OrgId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<AppId>,
        egress: bool,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// The owner deletes the organization: its teams, invites and apps go
    /// with it. Logged with the owner's passkey consent.
    DeleteOrg {
        org: OrgId,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    SetOrgMetadata {
        org: OrgId,
        metadata: Value,
        actor: AccountId,
    },
    SetMemberMetadata {
        org: OrgId,
        account: AccountId,
        metadata: Value,
        actor: AccountId,
    },
    /// `account` lets `source`'s runs for them call `methods` of `target` as them (the
    /// target sees the person as its caller), with their passkey (`<source> / __delegate /
    /// { target, methods }`, [`delegation_args`]); `methods: None` takes it back, which
    /// needs no consent.
    SetDelegation {
        account: AccountId,
        source: AppId,
        target: AppId,
        methods: Option<Vec<String>>,
        #[serde(default)]
        consent: Value,
    },
    /// Attaches `target` under `name`, one of the `attach` references of the release the app
    /// runs (`AppRecord::attached`), or detaches it (`None`). The manager of both apps, with
    /// passkey consent (`<app> / __link / { name, target }`, [`link_app_args`]).
    LinkApp {
        app_id: AppId,
        name: String,
        target: Option<AppId>,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    SetAppMetadata {
        app_id: AppId,
        metadata: Value,
        actor: AccountId,
        /// An organization's app is managed by its owner only while they are
        /// a member of the organization ([`SystemState::owner_acts`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays judging ownership alone.
        #[serde(default)]
        owner_in_org: bool,
        /// An app on a team is reached and managed by its owner only while
        /// they are in that team ([`SystemState::owner_acts`]). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays judging the owner without the team.
        #[serde(default)]
        owner_in_team: bool,
    },
    SetPasskeyLabel {
        account: AccountId,
        credential: String,
        label: String,
    },
    /// An upgrade that did not run (its migration failed, or it was refused
    /// on the app's group): the app keeps its bundle, and says why.
    UpgradeFailed {
        app_id: AppId,
        bundle: String,
        by: String,
        at: u64,
        error: Value,
        #[serde(default)]
        run: Option<String>,
        /// The release the upgrade was to, when it named one.
        #[serde(default)]
        release: Option<PackageRef>,
        /// The release the app ran when an update on its own began (`by` is
        /// [`AUTO_UPDATE`]; `None` for any other, so their entries read as before):
        /// apply notes nothing when the app has moved since or already runs the target,
        /// as another move won (`Feature::AutoUpdates`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_release: Option<PackageRef>,
    },
    StartUpgradeRun {
        org: OrgId,
        run: Box<crate::types::UpgradeRun>,
        /// The starter's passkey consent over the run, or the operator's
        /// signature for an operator's run; each app's `UpgradeApp` of the
        /// run is then judged by the run.
        #[serde(default)]
        consent: Value,
        /// A publisher's run covers the instances that run a release of its
        /// package ([`SystemState::published_run_apps`]). The API sets it on a
        /// publisher's run; absent (`false`) in an entry an earlier release
        /// logged, which replays judging each app by the package its bundle
        /// was first recorded from, as that release did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        by_release: bool,
        /// Only an owner or admin of the app's organization approves its upgrades, so only they start a run of it (a publisher's run covers only the organizations they are one of)
        /// ([`SystemState::approves_upgrades`]). The API sets it once every node reads it
        /// ([`cluster_version::Feature::OwnerApprovedUpgrades`]); absent (`false`) in an
        /// entry an earlier release logged, which replays as it applied, unless the logged
        /// cluster version brings the rule ([`SystemState::cluster_brings`]).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        owner_approves: bool,
    },
    FinishUpgradeRun {
        org: OrgId,
        run_id: String,
    },
    /// An account names an app (`Some`) or drops the name (`None`).
    SetAlias {
        account: AccountId,
        alias: String,
        app: Option<AppId>,
    },
    SetOrgRole {
        org: OrgId,
        account: AccountId,
        role: OrgRole,
        actor: AccountId,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    /// Remove a member, or (`account == actor`) leave.
    RemoveOrgMember {
        org: OrgId,
        account: AccountId,
        actor: AccountId,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    CreateTeam {
        org: OrgId,
        team_id: TeamId,
        name: String,
        parent: Option<TeamId>,
        join: TeamJoin,
        actor: AccountId,
        /// A member of `parent` holding `caps::CREATE_CHILD_TEAM` there (an
        /// app team's default) may create a restricted team below it. The
        /// API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays with only `parent`'s managers
        /// creating below it.
        #[serde(default)]
        member_child_teams: bool,
    },
    SetTeamMember {
        org: OrgId,
        team: TeamId,
        account: AccountId,
        lead: bool,
        /// The row's role; `lead` alone answers for rows written before roles.
        #[serde(default)]
        role: Option<TeamRole>,
        /// The row's capabilities; a new row takes the organization's default.
        #[serde(default)]
        capabilities: Option<u32>,
        actor: AccountId,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    /// `account`'s rows in several teams of `org` at once, with one passkey
    /// consent (`org:<org> / __set_team_members`, [`team_members_args`]):
    /// each row judged as its own [`SystemCmd::SetTeamMember`] would be, and
    /// none set unless every one holds.
    SetTeamMembers {
        org: OrgId,
        account: AccountId,
        teams: Vec<TeamRow>,
        actor: AccountId,
        consent: Value,
    },
    /// Change a team: its name, whether it is open, where it hangs (`parent`:
    /// `Some(None)` moves it to the top), what the app keeps about it.
    UpdateTeam {
        org: OrgId,
        team: TeamId,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        join: Option<TeamJoin>,
        #[serde(
            default,
            deserialize_with = "crate::types::present",
            skip_serializing_if = "Option::is_none"
        )]
        parent: Option<Option<TeamId>>,
        #[serde(default)]
        metadata: Option<Value>,
        actor: AccountId,
        /// A team marked as an app team here gives a row to each member who
        /// gets every app added later ([`crate::types::Org::all_apps`]). The
        /// API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays adding no rows, as it did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_apps_rows: bool,
        /// Closing the team (`join: invite`) takes, from whoever loses it
        /// then (someone in it by inheritance alone), their direct rows in
        /// the teams below it, as a removal from the team does, so a private
        /// channel below does not keep them in. The API always sets it;
        /// absent (`false`) in an entry an earlier release logged, which
        /// replays keeping those rows.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        close_drops_below: bool,
    },
    /// Delete a team, the teams below it and their apps.
    DeleteTeam {
        org: OrgId,
        team: TeamId,
        actor: AccountId,
        /// The actor's passkey consent, over the apps that go with the team
        /// (`org:<org> / __delete_team / { team, apps }`, [`SystemState::team_apps`]).
        #[serde(default)]
        consent: Value,
        /// A team whose subtree holds apps needs `consent`. The API always
        /// sets it; absent (`false`) in an entry an earlier release logged,
        /// which replays deleting without one, as that release did.
        #[serde(default)]
        apps_need_consent: bool,
        /// Keep the instances the team's subtree alone admits ([`SystemState::team_apps`]):
        /// they lose its team grants and keep their other grants (named members, guests,
        /// `anyone`, links); with none left, the owner alone (an empty grant list). Absent
        /// (`false`), as every entry before it: they are deleted. Stated in the consent
        /// (`keepApps: true`). Proposed only from cluster version 1.0.0-rc.33
        /// (`Feature::KeepTeamApps`), as an earlier release would delete them.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        keep_apps: bool,
    },
    /// A member's capabilities, organization-wide (`team: None`) or in a team.
    SetCapabilities {
        org: OrgId,
        #[serde(default)]
        team: Option<TeamId>,
        account: AccountId,
        capabilities: u32,
        actor: AccountId,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    /// Give a member every app added to the organization from now on
    /// (`all_apps: true`, [`crate::types::Org::all_apps`]), or stop it: the
    /// owner's or an admin's, with their passkey consent (`org:<org> /
    /// __set_all_apps / { account, allApps }`). Apps already there are not
    /// touched.
    SetAllApps {
        org: OrgId,
        account: AccountId,
        all_apps: bool,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// The capabilities members hold unless set for them: organization-wide
    /// (`team: None`), or everyone in `team` on top of their own (G6).
    SetDefaultCapabilities {
        org: OrgId,
        /// Absent in entries from before, which set the organization's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<TeamId>,
        capabilities: u32,
        actor: AccountId,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
    },
    RemoveTeamMember {
        org: OrgId,
        team: TeamId,
        account: AccountId,
        actor: AccountId,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
        /// The removal takes the account's direct rows in the teams below
        /// `team` too. The API always sets it; absent (`false`) in an entry an
        /// earlier release logged, which replays removing the one row.
        #[serde(default)]
        subtree: bool,
    },
    /// Take `account` out of several teams of `org` at once (or, `account ==
    /// actor`, leave them), with one passkey consent (`org:<org> /
    /// __remove_team_members`, [`remove_team_members_args`]): each judged as
    /// its own [`SystemCmd::RemoveTeamMember`] (the rows below included)
    /// would be, and none taken unless every one holds.
    RemoveTeamMembers {
        org: OrgId,
        account: AccountId,
        teams: Vec<TeamId>,
        actor: AccountId,
        consent: Value,
    },
    /// An invite, recorded under its token's public key
    /// ([`crate::proof::invite_key`]); the token itself is never logged.
    CreateInvite {
        invite_key: String,
        invite: Invite,
        actor: AccountId,
        /// The actor's passkey consent over [`invite_args`].
        #[serde(default)]
        consent: Value,
        /// Below admin, the actor must see every team the invite names
        /// ([`Org::sees_team`]). The API always sets it; absent (`false`) in an
        /// entry an earlier release logged, which replays without that check.
        #[serde(default)]
        teams_seen: bool,
    },
    /// Redeem an invite for `account`: `signature` is the token's key's
    /// ([`crate::proof::sign_invite_redeem`]), made by the node the redeemer
    /// handed the token to.
    RedeemInvite {
        invite_key: String,
        account: AccountId,
        now: u64,
        #[serde(default)]
        signature: String,
        /// A redemption that changes nothing for `account` (a member at the
        /// invite's role or above, in every team it names) spends no use.
        /// Absent (`false`) in a redemption an earlier release logged:
        /// replayed, it spends one, as that release did, so every replica
        /// reaches the same state.
        #[serde(default)]
        free_when_unchanged: bool,
        /// The invite's `all_apps` grant stands only while its creator is
        /// still an owner or admin. Absent (`false`) in an earlier
        /// release's entry, which replays granting it as minted
        /// ([`cluster_version::Feature::InviteCapabilities`]).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clamp_grants: bool,
    },
    /// Withdraw an invite before it is used up: its creator, an admin, or
    /// whoever manages every team it names ([`SystemState::sees_invite`]),
    /// with their passkey consent (`org:<org> / __revoke_invite /
    /// { inviteKey }`).
    RevokeInvite {
        org: OrgId,
        invite_key: String,
        actor: AccountId,
        consent: Value,
    },
    /// Change an organization's settings ([`OrgSettings`]: whether its instances take
    /// guests, [`crate::types::Org::allow_guests`]): its owner or an admin, with passkey
    /// consent (`org:<org> / __set_org_settings / settings`). A setting left out is kept.
    SetOrgSettings {
        org: OrgId,
        settings: OrgSettings,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Mint an instance invite ([`crate::types::AppInvite`]): whoever manages the instance's
    /// guests ([`SystemState::manages_grants`]), with passkey consent over
    /// [`app_invite_args`] (`<app> / __create_app_invite`), on any instance. Only the token's
    /// public key is logged.
    CreateAppInvite {
        invite_key: String,
        invite: crate::types::AppInvite,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Withdraw an instance invite: its creator or whoever manages the instance's guests,
    /// with passkey consent (`<app> / __revoke_app_invite / { inviteKey }`).
    RevokeAppInvite {
        app_id: AppId,
        invite_key: String,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Make a public link of an instance ([`crate::types::PublicLink`]): whoever may change
    /// its access ([`SystemState::manages_grants`]), with passkey consent over
    /// [`public_link_args`] (`<app> / __create_public_link`), while its organization allows
    /// public links. Only the token's public key is logged.
    CreatePublicLink {
        link_key: String,
        link: crate::types::PublicLink,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Revoke a public link: its creator or whoever may change the instance's access, with
    /// passkey consent (`<app> / __revoke_public_link / { linkKey }`). It admits nobody from
    /// the next request.
    RevokePublicLink {
        app_id: AppId,
        link_key: String,
        actor: AccountId,
        #[serde(default)]
        consent: Value,
    },
    /// Redeem an instance invite: `account` joins the instance's guest list. Carries the
    /// token key's signature over the redemption (`proof::sign_invite_redeem`), so a node
    /// that only saw the log cannot redeem one. One already listed spends no use.
    RedeemAppInvite {
        invite_key: String,
        account: AccountId,
        now: u64,
        signature: String,
    },
    /// Give apps their own access lists in one step (G3): for each package
    /// named (absent: every package with an instance an `org` grant admits
    /// here), find or make its app team ([`crate::types::Team::app_package`]),
    /// give every person who reaches its instances now a row there, and give
    /// the instances its `team` grant in place of `org`. `shared` (absent: whether the
    /// organization then holds two app teams or more) closes every app team,
    /// rows filled in first, so nobody loses an app. A package's own `join`
    /// opens or closes its team alone ([`ScopeApp::join`], `shared` absent
    /// then means no other team changes), a close keeping everyone who
    /// reaches it or only the accounts named ([`ScopeApp::keep`]). An
    /// admin's, with their passkey consent over [`scope_apps_args`].
    ScopeApps {
        org: OrgId,
        #[serde(default)]
        apps: Option<Vec<ScopeApp>>,
        #[serde(default)]
        shared: Option<bool>,
        /// Random, from the API: new teams' ids derive from it
        /// (`app_team_id`), so every replica makes the same ones.
        nonce: String,
        actor: AccountId,
        consent: Value,
        /// A new app team's default capabilities let its members create
        /// restricted teams below it (`caps::CREATE_CHILD_TEAM`). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays making the default it made then.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        member_child_teams: bool,
        /// A new app team gives a row to each member who gets every app
        /// added later ([`crate::types::Org::all_apps`]), kept on a close
        /// with a list too. The API always sets it; absent (`false`) in an
        /// entry an earlier release logged, which replays filling the rows
        /// it filled then.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_apps_rows: bool,
        /// An instance belongs to the package its release names
        /// (`AppRecord::release`), not the one its component's bundle record
        /// names: two packages may ship a byte-identical component. The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays judging by the bundle, as that release did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        by_release: bool,
    },
    /// Set, rotate (`Some`) or remove (`None`) an organization's recovery
    /// key: the owner's, with their passkey consent (`org:<org> /
    /// __set_recovery_key / { publicKey }`). Cancels a pending recovery.
    SetRecoveryKey {
        org: OrgId,
        public_key: Option<String>,
        at: u64,
        actor: AccountId,
        consent: Value,
    },
    /// Start handing `org` to `account`: `signature` is the recovery key's
    /// ([`calimero_identity::recovery::sign_recovery`]) over `org`, `account`
    /// and `nonce`, made by whoever holds the seed. It may finish
    /// [`crate::types::RECOVERY_DELAY_SECS`] after `at`.
    StartRecovery {
        org: OrgId,
        account: AccountId,
        nonce: String,
        signature: String,
        at: u64,
    },
    /// Cancel the recovery handing `org` to `account`: the owner's, with
    /// their passkey consent (`__cancel_recovery / { account }`).
    CancelRecovery {
        org: OrgId,
        account: AccountId,
        actor: AccountId,
        consent: Value,
    },
    /// Finish a recovery once its delay passed: the account it names, with
    /// its passkey consent (`__finish_recovery / { org }`). The account
    /// becomes the owner; the previous owner leaves the organization and its
    /// apps there become the new owner's.
    FinishRecovery {
        org: OrgId,
        at: u64,
        actor: AccountId,
        consent: Value,
    },
}

/// The longest passkey label or alias, in characters.
const MAX_LABEL_CHARS: usize = 64;

/// How many aliases one account keeps.
const MAX_ALIASES: usize = 1000;

/// Refuses metadata over [`crate::types::MAX_METADATA_BYTES`].
fn metadata_fits(metadata: &Value) -> Result<(), &'static str> {
    if serde_json::to_vec(metadata).map_or(usize::MAX, |v| v.len())
        > crate::types::MAX_METADATA_BYTES
    {
        return Err("metadata is larger than 16 KiB");
    }
    Ok(())
}

/// The account activity entry for the passkey consent `cmd` carries, which
/// [`authorize`] verified and marked `used`: the account whose passkey gave
/// it, the method and subject it states, at the time it carries. None for an
/// operator's or an API key's proof, and for a consent with no time (a
/// sign-in's, whose entry comes from its session, or one an earlier release
/// logged), so replaying an earlier release's log adds nothing. A key's
/// follow-ups add one entry each, the call it approved.
fn approval_of(
    s: &SystemState,
    cmd: &SystemCmd,
    used: &str,
) -> Option<(AccountId, Vec<crate::types::ActivityEntry>)> {
    let Ok(Value::Object(fields)) = serde_json::to_value(cmd) else {
        return None;
    };
    let consent = fields.into_iter().find_map(|(_, v)| {
        v.get("assertion")?;
        let c: crate::proof::Consent = serde_json::from_value(v).ok()?;
        (c.at > 0 && c.used().ok()? == used).then_some(c)
    })?;
    let passkey = consent.assertion.credential_id;
    let account = s.credentials.get(&passkey)?.clone();
    let text = |t: &str| t.chars().take(crate::types::ACTIVITY_TEXT_CHARS).collect();
    let entry = |method: &str, subject: &str| crate::types::ActivityEntry {
        at: consent.at,
        passkey: passkey.clone(),
        what: crate::types::Activity::Approval {
            method: text(method),
            subject: text(subject),
        },
    };
    let follow_ups = match cmd {
        SystemCmd::CreateApiKey { then, .. } => then.as_slice(),
        _ => &[],
    };
    let entries = std::iter::once(entry(&consent.statement.method, &consent.statement.subject))
        .chain(follow_ups.iter().map(|f| entry(&f.method, &f.app)))
        .collect();
    Some((account, entries))
}

/// The activity entry of a sign-in that opened `record`'s session, at its
/// opening (0, an earlier release's: none).
fn sign_in_of(record: &RefreshRecord) -> Option<crate::types::ActivityEntry> {
    Some(crate::types::ActivityEntry {
        at: record.opened_at,
        passkey: record.passkey.clone()?,
        what: crate::types::Activity::SignIn {
            app: record.app.clone(),
            origin: record.origin.clone(),
        },
    })
    .filter(|e| e.at > 0)
}

/// The system group's state machine.
#[derive(Default)]
pub struct SystemMachine {
    pub state: SystemState,
    /// The log index of the entry being applied (0 outside the log, in
    /// tests): what [`SystemState::session_proofs_from`] is judged against.
    index: u64,
    /// This cluster's name (`--cluster`): what a publisher binding is checked
    /// against at apply. It is fixed for a cluster's life (the storage key is
    /// derived from it), so every replica and every replay judges alike.
    cluster: String,
    /// Indexes and approval decisions derived from `state`, for the Raft
    /// messages of every app group ([`routing`]); dropped whenever `state`
    /// changes through [`Machine::apply`] or [`Machine::restore`].
    routing: routing::Routing,
}

impl SystemMachine {
    /// The system machine of the cluster named `cluster`.
    #[must_use]
    pub fn new(cluster: String) -> Self {
        Self {
            cluster,
            ..Self::default()
        }
    }

    /// The app group `group` replicates: an index probe, not a scan.
    #[must_use]
    pub fn app_of_group(&self, group: u64) -> Option<&AppRecord> {
        self.routing.app_of_group(&self.state, group)
    }

    /// The apps that name `node` a replica, by app id.
    pub fn apps_on(&self, node: u64) -> impl Iterator<Item = &AppRecord> + '_ {
        self.routing.apps_on(&self.state, node)
    }

    /// [`SystemState::release_may_hold`] for the app `group` replicates,
    /// cached until the state next changes; `None` for a group no app has.
    #[must_use]
    pub fn group_may_hold(&self, group: u64, release: Option<&str>) -> Option<bool> {
        self.routing.group_may_hold(&self.state, group, release)
    }

    /// Drop what [`Self::app_of_group`], [`Self::apps_on`] and
    /// [`Self::group_may_hold`] derived: for a change to `state` made
    /// outside the log (tests).
    pub fn forget_routing(&mut self) {
        self.routing.forget();
    }
}

fn ok(v: Value) -> Vec<u8> {
    // `Value`'s `Display` is its compact JSON and cannot fail.
    json!({ "ok": v }).to_string().into_bytes()
}

fn err(refused: &Rejection) -> Vec<u8> {
    refused.to_output()
}

/// Read a system command's output: its value, or its refusal of the kind
/// the answer's `code` names ([`Rejection::from_output`]).
///
/// # Errors
/// The command was refused, or the output is not JSON.
pub fn output(bytes: &[u8]) -> Result<Value, Rejection> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| Rejection::from(e.to_string()))?;
    if let Some(e) = v.get("err") {
        return Err(Rejection::from_output(&v, e));
    }
    Ok(v.get("ok").cloned().unwrap_or(Value::Null))
}

/// What `AcquireRun` answers when it refuses; `retryable` says a later acquire may succeed.
fn acquire_refused(code: &str, retryable: bool) -> Value {
    json!({ "state": "refused", "code": code, "retryable": retryable })
}

impl SystemMachine {
    fn org_mut(&mut self, org: &str) -> Result<&mut Org, &'static str> {
        self.state.orgs.get_mut(org).ok_or("no such organization")
    }

    /// Apply `cmd`, only once its proof holds ([`authorize`]): refused if it
    /// is new work its scope has lost ([`Self::new_work_refusal`]), else by its
    /// own rules. The proof is spent only when the command applies.
    fn run(&mut self, mut cmd: SystemCmd) -> Result<Value, Rejection> {
        // From the mark on, a flag naming an older rule is no choice of the
        // proposer's: this release's nodes always set it.
        if self.state.holds_current_rules(self.index) {
            cmd.hold_to_current_rules();
        }
        // An entry logged before the cluster required session proofs applies
        // as it did then: a restart replays it, and every replica must reach
        // the answer the release that logged it reached.
        let used = if cmd.is_session_command() && !self.state.proves_sessions(self.index) {
            None
        } else {
            authorize(&self.state, &cmd)?
        };
        if let Some(stall) = self.new_work_refusal(&cmd) {
            return Err(stall.rejection());
        }
        // Found before it applies: the command may remove the passkey.
        let approval = used
            .as_deref()
            .and_then(|used| approval_of(&self.state, &cmd, used));
        // From the `ExpireProofs` mark on, a sign-in's proof is forgotten
        // once its session expires: one whose session expired by the last
        // time proofs were forgotten may be a forgotten proof logged again.
        let expiry = if self.state.expires_proofs && used.is_some() {
            cmd.signed_session_expiry()
        } else {
            None
        };
        if expiry.is_some_and(|at| at <= self.state.proofs_forgotten_through) {
            return Err("that sign-in's session has expired: sign in again".into());
        }
        let out = self.run_authorized(cmd)?;
        if let Some(used) = used {
            if let Some(at) = expiry {
                let _new = self
                    .state
                    .proof_expiry
                    .entry(at)
                    .or_default()
                    .insert(used.clone());
            }
            let _new = self.state.used_proofs.insert(used);
        }
        if let Some((account, entries)) = approval {
            for entry in entries {
                self.state.record_activity(&account, entry);
            }
        }
        Ok(out)
    }

    /// [`Self::run`] without the proof: the command's own rules and the
    /// new-work refusal, for tests of those rules.
    #[cfg(test)]
    fn run_unproven(&mut self, cmd: SystemCmd) -> Result<Value, Rejection> {
        if let Some(stall) = self.new_work_refusal(&cmd) {
            return Err(stall.rejection());
        }
        self.run_authorized(cmd)
    }

    /// Creating an app, upgrading one or installing a release is new work:
    /// past a release's deadline, refused for a scope that has not approved
    /// it. Judged at the time the command carries (the proposing node's
    /// clock, stamped once and replicated with it), so every replica, and
    /// every replay, decides the same. Anything else, calls included, goes on.
    fn new_work_refusal(&self, cmd: &SystemCmd) -> Option<Stall> {
        let s = &self.state;
        match cmd {
            SystemCmd::CreateApp { app, .. } => {
                s.stalled_for_release(s.governing_release(app), &app.org, app.created_at)
            }
            SystemCmd::UpgradeApp {
                app_id,
                bundle,
                at,
                release,
                ..
            } => {
                let a = s.apps.get(app_id)?;
                let to = s.governing(release.as_ref(), bundle, *at);
                s.stalled_for_release(to, &a.org, *at)
            }
            SystemCmd::InstallPackage {
                release, signer_id, ..
            } => s.install_stalled(&release.installed_by, signer_id, release.installed_at),
            _ => None,
        }
    }

    /// Apply `cmd` by its own rules, its proof already judged ([`Self::run`]).
    fn run_authorized(&mut self, cmd: SystemCmd) -> Result<Value, Rejection> {
        match cmd.area() {
            Area::Fleet => fleet::apply(self, cmd),
            Area::Packages => packages::apply(self, cmd),
            Area::Apps => apps::apply(self, cmd),
            Area::Compute => compute::apply(self, cmd),
            Area::Accounts => accounts::apply(self, cmd),
            Area::Recovery => recovery::apply(self, cmd),
            Area::Approvals => approvals::apply(self, cmd),
            Area::Deployments => deployments::apply(self, cmd),
            Area::SignUp => sign_up::apply(self, cmd),
            Area::Orgs => orgs::apply(self, cmd),
            Area::Updates => updates::apply(self, cmd),
        }
    }
}

/// A refusal: no account recovery code is (still) known under that key.
pub const UNKNOWN_RECOVERY_CODE: &str = "unknown or used recovery code";
/// A refusal: a recovery of the account is under way already.
pub const ACCOUNT_RECOVERY_PENDING: Rejection = Rejection::new(
    RejectionKind::RecoveryPending,
    "a recovery of this account is under way",
);
/// A refusal: no recovery of the account is under way (for that passkey).
pub const NO_ACCOUNT_RECOVERY_PENDING: &str = "no recovery of this account is under way";

/// A refusal: the organization holds no recovery key.
pub const NO_RECOVERY_KEY: Rejection = Rejection::new(
    RejectionKind::NoRecoveryKey,
    "this organization has no recovery key",
);
/// A refusal: a recovery is under way already.
pub const RECOVERY_PENDING: Rejection = Rejection::new(
    RejectionKind::RecoveryPending,
    "a recovery of this organization is under way",
);
/// A refusal: the bundle a backup's state runs is no longer installed.
pub const RESTORE_BUNDLE_GONE: &str =
    "the bundle the backup's state runs is no longer installed: install that release again first";
/// A refusal: no recovery under way for that account.
pub const NO_RECOVERY_PENDING: &str =
    "no recovery of this organization is under way for that account";
/// A refusal: the recovery's delay has not passed.
pub const RECOVERY_NOT_READY: Rejection = Rejection::new(
    RejectionKind::RecoveryNotReady,
    "the recovery may finish only once its delay has passed",
);
/// A refusal: someone but the owner manages the recovery key.
pub const OWNER_ONLY_RECOVERY: &str = "only the organization's owner manages its recovery";

/// A recovery key as the organization keeps it, refused unless it is an
/// Ed25519 public key.
fn recovery_key_of(public_key: String, at: u64) -> Result<crate::types::OrgRecovery, &'static str> {
    let _key = calimero_identity::recovery::parse_public(&public_key)
        .map_err(|_| "a recovery key is a base64url Ed25519 public key")?;
    Ok(crate::types::OrgRecovery {
        public_key,
        set_at: at,
        pending: None,
    })
}

impl Machine for SystemMachine {
    const IN_MEMORY: bool = true;

    fn apply(
        &mut self,
        index: u64,
        command: &[u8],
        state: &mut calimero_log::StateTx<'_>,
    ) -> Vec<u8> {
        self.index = index;
        // Whatever the entry changes, what was derived from the state goes.
        self.routing.forget();
        match serde_json::from_slice::<SystemCmd>(command) {
            Ok(cmd) => match self.run(cmd) {
                Ok(v) => ok(v),
                Err(e) => err(&e),
            },
            Err(e) => {
                // A feature gate keeps every entry within what each node
                // applies (`cluster_version`): one that does not decode here
                // was written by a later release (this node rolled back, or
                // joined on an older one). Skipping it would leave this node
                // serving a state the others no longer have (a revoked key
                // still working here), so the group stops on it instead.
                // Named, never quoted: serde_json's words quote the entry (a name, an
                // email), and the operator reads this line and the stop reason.
                let n = crate::metrics::undecodable_entry(crate::metrics::LogKind::System);
                let named = crate::log_safe::serde(&e);
                error!(index, error_kind = %named, undecodable = n,
                    "a system entry this node cannot decode: the system group stops here; run a release that reads it");
                state.fail(format_args!(
                    "system entry {index} does not decode: {named}"
                ));
                err(&Rejection::from(format!("malformed command: {e}")))
            }
        }
    }

    fn snapshot(&self) -> eyre::Result<Vec<u8>> {
        // Never an empty snapshot: restoring one fails on the node that receives it,
        // far from here, with nothing logged where it happened.
        let bytes = serde_json::to_vec(&self.state)
            .map_err(|e| eyre::eyre!("the system state does not serialize: {e}"))?;
        crate::metrics::system_state_bytes(bytes.len());
        Ok(bytes)
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        // Empty: the group stopped before its first checkpoint, and starts over.
        self.state = if data.is_empty() {
            SystemState::default()
        } else {
            serde_json::from_slice(data)?
        };
        crate::metrics::system_state_bytes(data.len());
        self.routing.forget();
        Ok(())
    }
}

#[cfg(test)]
mod queue_tests;

#[cfg(test)]
mod answer_tests;
#[cfg(test)]
mod tests;
