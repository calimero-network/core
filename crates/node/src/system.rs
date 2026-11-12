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
    Access, Account, AccountId, ApiKeyRecord, AppId, AppRecord, Audience, Components, Invite,
    NodeRecord, NodeRole, Org, OrgId, OrgRole, RefreshRecord, Team, TeamId, TeamJoin, TeamRole,
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
mod sign_up;

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
    /// Plaintext bytes of the files an organization's (or a personal account's) apps hold.
    pub max_file_bytes_per_org: u64,
    /// Compute runs an organization (a personal account's, for an app outside one) may have
    /// granted to job nodes at once; the rest wait their turn.
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

/// The prefix of every apply error that is a quota refusal.
pub const QUOTA_PREFIX: &str = "quota exceeded";

/// The refusal when an app would run a compute component.
const COMPUTE_NOT_AN_APP: &str = "a compute component is run by job nodes, never as an app";

/// The refusal when an app names a compute component its bundle's release does not ship.
/// The refusal of an uninstall of a release another release's pipelines pin as a module.
pub const MODULE_IN_USE: &str =
    "an installed release's pipelines use that release as a module; uninstall them first";
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
        self.versions
            .values()
            .max_by(|a, b| compare_versions(&a.version, &b.version))
    }
}

/// The rows a `MoveApp` gives, by the flags its entry carries (an entry an
/// earlier release logged leaves them out, and replays without those rows).
#[derive(Clone, Copy, Debug)]
struct MoveRows {
    /// A new app team gives its members who get every app added later a row.
    all_apps_rows: bool,
    /// An `Accounts` instance's people get a row on its closed app team.
    accounts_in_app_team: bool,
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

/// The prefix of every apply error that refuses a sign-up under the
/// cluster's policy; the API answers it `403 SIGN_UP_REFUSED`.
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
pub const ONLY_ADMINS_INSTALL: &str =
    "only the organization's owner and admins install apps in it and change its app teams";
/// A refusal: a standing grant of every app added later ([`Org::all_apps`]),
/// by invite or set on a member, is the owner's or an admin's.
pub const ONLY_ADMINS_ALL_APPS: &str =
    "only the organization's owner and admins give someone every app added later";
/// A refusal: the package has an app team in this organization already.
pub const APP_TEAM_EXISTS: &str = "this package has an app team in this organization already";

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
pub const NOT_A_DEPLOYER: &str = "This cluster only lets Calimero Cloud deployers install apps; ask the cluster's owner or operator for a deployer grant.";

/// How many deployer grants a cluster keeps.
pub const MAX_DEPLOYERS: usize = 10_000;

/// Whose release approvals a change is to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ReleaseScope {
    Org {
        org: OrgId,
    },
    Account,
    /// Every app, by the operator: only where the deployment gave it this
    /// decision ([`Governance`]).
    Cluster,
}

/// The prefix of every apply error that refuses new work past a release
/// deadline ([`Stall`]); the API answers it `409 RELEASE_DEADLINE`.
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
    /// The refusal, naming the release to approve and its deadline.
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
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map_or_else(
            || secs.to_string(),
            |t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )
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

    /// Removes an app, the file bytes counted against its payer and the compute runs it held.
    fn remove_app(&mut self, app_id: &str) {
        self.set_app_file_bytes(app_id, 0);
        let _gone = self.apps.remove(app_id);
        let _ring = self.released_runs.remove(app_id);
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

    /// The releases approved to hold `app`'s data: its organization's, or its
    /// owner's for an app outside one. Empty means none was ever recorded
    /// (state from before approvals), which leaves the app unrestricted.
    #[must_use]
    pub fn approved_releases(&self, app: &AppRecord) -> Option<&BTreeSet<String>> {
        self.approved_for_release(self.governing_release(app), app.org.as_deref(), &app.owner)
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

    /// The releases approved to hold an app governed by `release`
    /// ([`Self::governing_release`]), of `org` (or of `owner` outside one):
    /// the operator's where it holds release approvals; else the release's
    /// publisher's, for every instance of its package; else, for a raw
    /// upload or an unbound package, the app's own organization's or
    /// owner's ([`Self::approved_for`]).
    #[must_use]
    pub fn approved_for_release(
        &self,
        release: Option<&PackageRef>,
        org: Option<&str>,
        owner: &str,
    ) -> Option<&BTreeSet<String>> {
        match self.publisher_of(release) {
            Some(p) if self.governance().releases == Authority::Owner => {
                self.approved_for(Some(p), owner)
            }
            _ => self.approved_for(org, owner),
        }
    }

    /// Whether `who` (an account, or [`OPERATOR`]) may see and use the
    /// release `v`: whoever installed it; everyone, for a release the
    /// operator installed (the cluster's own catalog, which it upgrades apps
    /// to); an account sharing an organization with an account that installed
    /// it; and an account that reaches an app running one of its services.
    /// The operator sees only the releases it installed. Anyone else is
    /// answered as if it were not installed.
    #[must_use]
    pub fn sees_release(&self, v: &PackageVersion, who: &str) -> bool {
        if v.installed_by_any(who) || v.installed_by_any(OPERATOR) {
            return true;
        }
        if who == OPERATOR {
            return false;
        }
        let shares_org = v.installing_accounts().any(|installer| {
            self.orgs
                .values()
                .any(|o| o.role(installer).is_some() && o.role(who).is_some())
        });
        shares_org
            || self.apps.values().any(|a| {
                v.app_parts().values().any(|h| *h == a.bundle) && self.may_reach(a, Some(who))
            })
    }

    /// The releases of `p` that `who` may see ([`Self::sees_release`]).
    pub fn visible_versions<'a>(
        &'a self,
        p: &'a PackageRecord,
        who: &'a str,
    ) -> impl Iterator<Item = &'a PackageVersion> + 'a {
        p.versions
            .values()
            .filter(move |v| self.sees_release(v, who))
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
    /// visible to `who`, or `who` uploaded the same WASM as a raw bundle.
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
        shipped.peek().is_none()
            || b.uploaded_by == who
            || shipped.any(|v| self.sees_release(v, who))
    }

    /// [`Self::approved_releases`] for an app of `org` (or of `owner` outside
    /// one), before it exists.
    #[must_use]
    pub fn approved_for(&self, org: Option<&str>, owner: &str) -> Option<&BTreeSet<String>> {
        if self.governance().releases == Authority::Operator {
            let set = &self.operator_releases;
            return (!set.is_empty()).then_some(set);
        }
        let set = match org {
            Some(o) => &self.orgs.get(o)?.approved_releases,
            None => &self.accounts.get(owner)?.approved_releases,
        };
        (!set.is_empty()).then_some(set)
    }

    /// What `app`'s owner (its organization's, or its account's outside one)
    /// approved for apps signed by `signer` to call out to.
    #[must_use]
    pub fn outbound_approval(
        &self,
        app: &AppRecord,
        signer: &str,
    ) -> Option<&crate::types::OutboundApproval> {
        if self.governance().outbound == Authority::Operator {
            return self.operator_outbound.get(signer);
        }
        match &app.org {
            Some(o) => self.orgs.get(o)?.outbound_approvals.get(signer),
            None => self
                .accounts
                .get(&app.owner)?
                .outbound_approvals
                .get(signer),
        }
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
            .filter(|a| a.org.as_deref() == Some(org))
            .filter(|a| match (&teams, &a.access) {
                (None, _) => true,
                (Some(ts), Access::Team { team }) => ts.contains(team),
                (Some(_), _) => false,
            })
            .filter(|a| self.app_release(a).is_some_and(|r| r.package == package))
            .map(|a| a.app_id.clone())
            .collect()
    }

    /// The instances a publisher's run of `package` covers: every one whose
    /// bundle came out of `package` that `account` may upgrade.
    #[must_use]
    pub fn published_run_apps(&self, package: &str, account: &str) -> Vec<AppId> {
        self.apps
            .values()
            .filter(|a| {
                self.bundles
                    .get(&a.bundle)
                    .and_then(|b| b.package.as_ref())
                    .is_some_and(|at| at.package == package)
                    && self.may_upgrade(a, account)
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
    ) -> Vec<AppId> {
        let st = serde_json::from_value::<crate::proof::Statement>(consent.clone()).ok();
        match st.as_ref().map(|st| st.method.as_str()) {
            Some("__upgrade_published") => self.published_run_apps(&run.package, &run.by),
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
        let Some(at) = self.app_release(app).filter(|at| at.package == r.package) else {
            return Err(RUN_OTHER_PACKAGE);
        };
        if compare_versions(&r.version, &at.version) == std::cmp::Ordering::Less {
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
    /// The refusal, starting with [`SIGN_UP_PREFIX`].
    pub fn admits(&self, invite: Option<&SignUpRedemption>) -> Result<Admission, &'static str> {
        self.admits_key(invite.map(|r| (r.invite_key.as_str(), r.now)))
    }

    /// [`Self::admits`] for an invite key presented at `now`.
    ///
    /// # Errors
    /// The refusal, starting with [`SIGN_UP_PREFIX`].
    pub fn admits_key(&self, invite: Option<(&str, u64)>) -> Result<Admission, &'static str> {
        let admission = match invite {
            None => Admission::Open,
            Some((key, now)) => {
                if let Some(i) = self.sign_up_invites.get(key) {
                    if i.expires_at <= now || i.uses_left == 0 {
                        return Err("sign-up refused: the invite has expired");
                    }
                    Admission::SignUpInvite
                } else if let Some(i) = self.invites.get(key) {
                    if i.expires_at <= now || i.uses_left == 0 {
                        return Err("sign-up refused: the invite has expired");
                    }
                    if !self.orgs.contains_key(&i.org) {
                        return Err("sign-up refused: the invite's organization is gone");
                    }
                    Admission::OrgInvite
                } else {
                    return Err(
                        "sign-up refused: this invite was used up, revoked or never existed",
                    );
                }
            }
        };
        match (self.governance().sign_up, admission) {
            (SignUp::Open, _)
            | (SignUp::Invite, Admission::OrgInvite | Admission::SignUpInvite)
            | (SignUp::Closed, Admission::SignUpInvite) => Ok(admission),
            (SignUp::Invite, Admission::Open) => {
                Err("sign-up refused: this cluster takes new accounts by invite only")
            }
            (SignUp::Closed, _) => Err(
                "sign-up refused: this cluster takes only accounts its operator or owner pre-authorized with a sign-up invite",
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
    ) -> Result<OrgId, &'static str> {
        let inv = self.invites.get(invite_key).ok_or(INVITE_GONE)?;
        if inv.expires_at <= now || inv.uses_left == 0 {
            return Err(INVITE_EXPIRED);
        }
        let o = self.orgs.get(&inv.org).ok_or("no such organization")?;
        let unchanged = o.members.get(account).is_some_and(|r| *r >= inv.role)
            && (!inv.all_apps || o.all_apps.contains(account))
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
        let o = self.orgs.get_mut(&inv.org).ok_or("no such organization")?;
        let role = o
            .members
            .get(account)
            .copied()
            .map_or(inv.role, |r| r.max(inv.role));
        let _prev = o.members.insert(account.to_owned(), role);
        if inv.all_apps {
            let _new = o.all_apps.insert(account.to_owned());
        }
        for t in &inv.teams {
            if let Some(team) = o.teams.get_mut(t) {
                let _prev = team.members.entry(account.to_owned()).or_insert(false);
            }
        }
        Ok(inv.org)
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

    /// Whether `actor` may change `d`: [`Self::acts_for`] its owner.
    fn manages_deployment(&self, d: &Deployment, actor: &str) -> Result<(), &'static str> {
        if actor == OPERATOR || !self.acts_for(&d.owner, actor) {
            return Err("only the deployment's owner, or MANAGE_APPLICATION in its organization, may manage it");
        }
        Ok(())
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

    /// Whether `account` may upgrade `app`. For a `publisher` package whose
    /// key is bound to an organization: whoever holds `MANAGE_APPLICATION`
    /// in that organization, org-wide, and nobody else; while the key is
    /// unbound, nobody. Otherwise (`owner`):
    /// its owner (of an org app, while a member: [`Self::owner_acts`]), or,
    /// for an org app, whoever
    /// [manages the organization's apps](Self::manages_apps) on the app's
    /// team.
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
    ) -> Result<(), &'static str> {
        self.upgrade_key_fits_as_logged(record, scope, OwnerRule::CURRENT)
    }

    /// [`Self::upgrade_key_fits`], with the app's owner judged as
    /// [`Self::may_upgrade_as_logged`] does.
    fn upgrade_key_fits_as_logged(
        &self,
        record: &ApiKeyRecord,
        scope: &crate::types::UpgradeScope,
        rule: OwnerRule,
    ) -> Result<(), &'static str> {
        if record.apps.is_some()
            || record.methods.is_some()
            || record.read_only
            || record.allow_confirm
        {
            return Err("an upgrade key has no other scope");
        }
        let app = self.apps.get(&scope.app).ok_or("no such app")?;
        if !self.may_upgrade_as_logged(app, &record.account, rule) {
            return Err("an upgrade key needs an account that may upgrade the app");
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

    #[must_use]
    pub fn may_upgrade(&self, app: &AppRecord, account: &str) -> bool {
        self.may_upgrade_as_logged(app, account, OwnerRule::CURRENT)
    }

    /// [`Self::may_upgrade`], with the owner judged as the release that
    /// logged the command did ([`OwnerRule`]).
    #[must_use]
    pub fn may_upgrade_as_logged(&self, app: &AppRecord, account: &str, rule: OwnerRule) -> bool {
        let release = self.governing_release(app);
        if !self.upgrades_of(release).is_owner() {
            return self
                .upgraded_by_publisher(release)
                .is_some_and(|p| self.manages_apps(p, account, None));
        }
        let team = match &app.access {
            Access::Team { team } => Some(team.as_str()),
            _ => None,
        };
        self.owner_acts_as_logged(app, account, rule)
            || app
                .org
                .as_ref()
                .is_some_and(|o| self.manages_apps(o, account, team))
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
    /// team for `Access::Team`, organization-wide for any other access. Org
    /// admins and the owner hold it everywhere, a team's leads on their team.
    #[must_use]
    pub fn may_set_access(&self, app: &AppRecord, to: &Access, actor: &str) -> bool {
        let Some(org) = app.org.as_deref() else {
            return false;
        };
        let manages = |a: &Access| match a {
            Access::Team { team } => self.manages_apps(org, actor, Some(team)),
            _ => self.manages_apps(org, actor, None),
        };
        manages(&app.access) && manages(to)
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
        let Some(org) = app.org.as_deref() else {
            return true;
        };
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
        let Access::Team { team } = &app.access else {
            return true;
        };
        app.org
            .as_deref()
            .and_then(|o| self.orgs.get(o))
            .is_some_and(|o| o.in_team(owner, team))
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
        self.owner_acts_as_logged(app, account, rule)
            || app
                .org
                .as_deref()
                .is_some_and(|o| self.is_org_admin(o, account))
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

/// `account`'s row in `team` at `role` (with `capabilities`, else what a new
/// row takes), as `actor` sets it: [`SystemCmd::SetTeamMember`]'s rule, which
/// [`SystemCmd::SetTeamMembers`] applies to each of its teams.
fn set_team_row(
    o: &mut Org,
    team: &str,
    account: &str,
    role: TeamRole,
    capabilities: Option<u32>,
    actor: &str,
) -> Result<(), &'static str> {
    use crate::types::caps;

    let Some(t) = o.teams.get(team) else {
        return Err("no such team");
    };
    // Joining an open team one reaches anyway writes one's own row.
    let joins_self = actor == account
        && role == TeamRole::Member
        && t.join == TeamJoin::Everyone
        && o.in_team(actor, team);
    let manages = o.manages_team(actor, team)
        || (role != TeamRole::Lead && o.may(actor, Some(team), caps::MANAGE_MEMBERS));
    if !joins_self && !manages {
        return Err("not allowed to manage this team");
    }
    if o.role(account).is_none() {
        return Err("only organization members join teams");
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
    /// An owner reaches an app on a team only while in it.
    owner_in_team: bool,
    /// An `Accounts` instance is reached only through its app team.
    accounts_in_app_team: bool,
    /// A new app team's default holds `CREATE_CHILD_TEAM`.
    member_child_teams: bool,
    /// A new app team gives its `all_apps` members a row.
    all_apps_rows: bool,
    /// An instance's package is the one its release names
    /// ([`SystemState::instance_package`]).
    by_release: bool,
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
    ) -> Result<Value, &'static str> {
        use crate::types::caps;

        let o = self.orgs.get(org).ok_or("no such organization")?;
        if !o.role(actor).is_some_and(|r| r >= OrgRole::Admin) {
            return Err(ONLY_ADMINS_INSTALL);
        }
        let in_org = |a: &&AppRecord| a.org.as_deref() == Some(org);
        let specs = match apps {
            Some(specs) => specs,
            None => self
                .apps
                .values()
                .filter(in_org)
                .filter(|a| a.access == Access::Org)
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
            return Err("too many packages in one step");
        }
        // A package's own `join` (or `childTeams`) decides its team and no
        // other: `shared` then closes nothing more unless asked, and never
        // one opened here.
        let per_app = specs
            .iter()
            .any(|a| a.join.is_some() || a.child_teams.is_some());
        if shared == Some(true) && specs.iter().any(|a| a.join == Some(TeamJoin::Everyone)) {
            return Err("an app team cannot open while the step closes every app team");
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
                return Err("a package is named twice");
            }
            let record = self.packages.get(&spec.package).ok_or("no such package")?;
            let name = match spec.name {
                Some(n) => n,
                None => record
                    .latest()
                    .and_then(|v| v.metadata.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(spec.package.as_str())
                    .to_owned(),
            };
            if name.trim().is_empty() || name.chars().count() > MAX_APP_TEAM_NAME_CHARS {
                return Err("an app team's name is 1 to 64 characters");
            }
            let keep = match spec.keep {
                None => None,
                Some(_) if spec.join != Some(TeamJoin::Invite) => {
                    return Err("only closing an app team (join: invite) names who keeps it");
                }
                Some(list) => {
                    if list.len() > MAX_SCOPE_KEEP {
                        return Err("too many accounts kept in one step");
                    }
                    if list.iter().any(|a| o.role(a).is_none()) {
                        return Err("only members of the organization keep an app");
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
                        return Err("too many instances in one step");
                    }
                    let ids: BTreeSet<AppId> = ids.into_iter().collect();
                    for id in &ids {
                        let a = self.apps.get(id).ok_or("no such app")?;
                        if a.org.as_deref() != Some(org) || !of_package(a) {
                            return Err(
                                "an instance named is not of that package in this organization",
                            );
                        }
                    }
                    ids.into_iter().collect()
                }
                None => self
                    .apps
                    .values()
                    .filter(in_org)
                    .filter(|a| a.access == Access::Org && of_package(a))
                    .map(|a| a.app_id.clone())
                    .collect(),
            };
            let reach = instances
                .iter()
                .filter_map(|id| self.apps.get(id))
                .flat_map(|a| {
                    o.members
                        .keys()
                        .filter(|m| {
                            self.may_reach_as_logged(
                                a,
                                Some(m.as_str()),
                                logged.owner_in_team,
                                logged.accounts_in_app_team,
                            )
                        })
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
            report.push(json!({
                "package": plan.package,
                "teamId": team_id,
                "created": created,
                "apps": plan.instances,
                "added": added,
                "join": join,
                "dropped": dropped,
            }));
            for id in &plan.instances {
                if let Some(a) = self.apps.get_mut(id) {
                    a.access = Access::Team {
                        team: team_id.clone(),
                    };
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
        Ok(json!({ "mode": mode, "teams": report }))
    }

    /// [`SystemCmd::MoveApp`], judged and applied as one step: every rule is
    /// checked, and everyone it brings is found, before anything changes.
    ///
    /// - **Who:** the actor manages the app where it is now
    ///   ([`Self::manages_app`]: a personal app's owner, an organization's
    ///   app's owner while a member, or an admin there) and is an admin or the
    ///   owner of `to_org`.
    /// - **Access:** organization or app-team access becomes team access on
    ///   the target's app team of the app's package, made when missing: open,
    ///   unless every app team of the target is closed (shared), where it is
    ///   closed like the rest; an app team closed alone leaves it open. A legacy target (no app team) and a raw upload
    ///   (no package) keep organization access, so moving an app in never
    ///   changes how the target holds its other apps. Owner, public and
    ///   accounts access stay as they are; an accounts instance whose package
    ///   has a closed app team in the target is reached there only through
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
    ) -> Result<Value, &'static str> {
        use crate::types::{caps, OrgMode};

        let MoveRows {
            all_apps_rows,
            accounts_in_app_team,
        } = rows;

        let app = self.apps.get(app_id).ok_or("no such app")?;
        let from = app.org.clone();
        if from.as_deref() == Some(to_org) {
            return Err("the app is in that organization already");
        }
        if !self.manages_app(app, actor) {
            return Err(ONLY_MANAGERS_MOVE);
        }
        let target = self.orgs.get(to_org).ok_or("no such organization")?;
        if !target.role(actor).is_some_and(|r| r >= OrgRole::Admin) {
            return Err(ONLY_ADMINS_MOVE_IN);
        }
        let source = from.as_deref().and_then(|o| self.orgs.get(o));
        let package = self.package_of(app).map(str::to_owned);
        let source_team = match &app.access {
            Access::Team { team } => {
                let app_team = source
                    .and_then(|o| o.teams.get(team))
                    .and_then(Team::app_package)
                    .is_some_and(|p| package.as_deref() == Some(p));
                if !app_team {
                    return Err(MOVE_NEEDS_APP_TEAM);
                }
                Some(team.clone())
            }
            _ => None,
        };
        let to_app_team = matches!(app.access, Access::Org | Access::Team { .. })
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
            return Err(MOVE_OWNER_NOT_IN_TARGET);
        }

        let release = self.governing_release(app);
        let before = self.approved_for_release(release, from.as_deref(), &app.owner);
        let after = self.approved_for_release(release, Some(to_org), &app.owner);
        if let (Some(b), Some(a)) = (before, after) {
            if b.is_disjoint(a) {
                return Err(MOVE_RELEASES_DISJOINT);
            }
        }

        // The target pays from here on: its quotas hold what the app holds.
        let held = self.app_file_bytes.get(app_id).copied().unwrap_or(0);
        if self.file_bytes(to_org).saturating_add(held) > self.quotas.max_file_bytes_per_org {
            return Err("quota exceeded: file bytes");
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
            && (target_runs.saturating_add(runs) > self.quotas.max_concurrent_runs_per_org
                || target_fuel.saturating_add(fuel) > self.quotas.compute_fuel_per_org)
        {
            return Err("quota exceeded: compute runs");
        }

        // The team whose invites lose their reason once the app leaves it.
        let leaves_team = source_team.or_else(|| {
            package
                .as_deref()
                .and_then(|p| source?.app_team(p))
                .map(|t| t.team_id.clone())
        });
        let team_name = package.as_deref().map(|p| {
            let name = self
                .packages
                .get(p)
                .and_then(PackageRecord::latest)
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
        // An `Accounts` instance (a DM, a private list) of a package whose
        // app team in the target is closed is reached there only by those in
        // that team: who comes along, and its owner, get a row on it, as
        // they would on the app team an `org` or `team` instance goes to.
        let accounts_team = match (&app.access, package.as_deref()) {
            (Access::Accounts { .. }, Some(p)) if accounts_in_app_team => target
                .app_team(p)
                .filter(|t| t.join == TeamJoin::Invite)
                .map(|t| t.team_id.clone()),
            _ => None,
        };
        // App-team access without an app team to go to: the organization's.
        let mut access = match &app.access {
            Access::Team { .. } => Access::Org,
            other => other.clone(),
        };

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
            access = Access::Team { team: team_id };
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
            a.org = Some(to_org.to_owned());
            a.access = access.clone();
        }
        self.set_app_file_bytes(app_id, held);
        if let Some(slots) = self.compute.get_mut(app_id) {
            for slot in slots.values_mut() {
                to_org.clone_into(&mut slot.payer);
            }
            self.grant_runs();
        }
        let mut revoked = 0usize;
        if let (Some(from), Some(team)) = (from.as_deref(), leaves_team.as_deref()) {
            let still = self.apps.values().any(|a| {
                a.org.as_deref() == Some(from)
                    && matches!(&a.access, Access::Team { team: t } if t == team)
            });
            if !still {
                let before = self.invites.len();
                self.invites
                    .retain(|_, i| !(i.org == from && i.teams.iter().any(|t| t == team)));
                revoked = before - self.invites.len();
            }
        }
        Ok(json!({
            "appId": app_id,
            "from": from,
            "org": to_org,
            "access": access,
            "created": created,
            "added": joins.iter().map(|(m, _)| m).collect::<Vec<_>>(),
            "revokedInvites": revoked,
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

    /// [`Self::stalled`] for the set `org` (or `owner` outside one) holds
    /// itself: what governs the packages it publishes and its own apps of
    /// unbound packages.
    #[must_use]
    pub fn stalled_for(&self, org: Option<&str>, owner: &str, now: u64) -> Option<Stall> {
        self.stalled(self.approved_for(org, owner), now)
    }

    /// [`Self::stalled`] for an app of `org` (or of `owner`) governed by
    /// `release`: judged on the set that governs it
    /// ([`Self::approved_for_release`]).
    #[must_use]
    pub fn stalled_for_release(
        &self,
        release: Option<&PackageRef>,
        org: Option<&str>,
        owner: &str,
        now: u64,
    ) -> Option<Stall> {
        self.stalled(self.approved_for_release(release, org, owner), now)
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
                return self.stalled_for(Some(p), actor, now);
            }
        }
        let own = self.stalled_for(None, actor, now)?;
        let an_org_may = self.orgs.values().any(|o| {
            o.role(actor).is_some_and(|r| r >= OrgRole::Member)
                && self.stalled_for(Some(&o.org_id), actor, now).is_none()
        });
        (!an_org_may).then_some(own)
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
    pub fn deadlines_json(&self) -> Vec<Value> {
        let mut out: Vec<(&String, &u64)> = self.release_deadlines.iter().collect();
        out.sort_by(|a, b| compare_versions(a.0, b.0));
        out.into_iter()
            .map(|(r, after)| json!({ "version": r, "after": rfc3339(*after), "afterSecs": after }))
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
    /// approvals; else one the account approved for its own apps, or one an
    /// organization it belongs to approved (its membership already goes to
    /// those). An account none of whose scopes ever recorded an approval
    /// (state from before approvals) is unrestricted, as its apps are.
    #[must_use]
    pub fn release_may_serve_account(&self, account: &str, release: &str) -> bool {
        if self.governance().releases == Authority::Operator {
            return self.operator_releases.is_empty() || self.operator_releases.contains(release);
        }
        let own = self.accounts.get(account).map(|a| &a.approved_releases);
        let sets: Vec<&BTreeSet<String>> = own
            .into_iter()
            .chain(
                self.orgs
                    .values()
                    .filter(|o| o.role(account).is_some())
                    .map(|o| &o.approved_releases),
            )
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
    }
}

impl SystemState {
    pub fn account_of_credential(&self, cred_id: &str) -> Option<(&Account, &Credential)> {
        let account = self.accounts.get(self.credentials.get(cred_id)?)?;
        let cred = account.passkeys.iter().find(|c| c.id == cred_id)?;
        Some((account, cred))
    }

    /// Whether `account` may reach `app` at all. An organization's app is
    /// reached only by its current members, whatever names the account: its
    /// owner, or an `Accounts` list, stop reaching it once they leave or are
    /// removed (the organization itself gone: nobody but a public app's
    /// callers). An app on a team is reached only by that team, its owner
    /// included: an owner who leaves or is removed from the team (an app's
    /// team, a private channel) stops reaching it, as they stop managing it
    /// ([`Self::owner_acts`]). An `Accounts` instance of a package with an
    /// app team in its organization (a private channel, a DM) is reached only
    /// by those who reach that app team, its owner included
    /// ([`Self::app_team_of`]): a person removed from the app is out of its
    /// private channels too.
    #[must_use]
    pub fn may_reach(&self, app: &AppRecord, account: Option<&str>) -> bool {
        self.may_reach_as_logged(app, account, true, true)
    }

    /// [`Self::may_reach`], or, for a command an earlier release logged
    /// with an app on a team reached by its owner whatever the team holds
    /// (`owner_in_team: false`), or an `Accounts` instance reached by its
    /// list whatever its app team holds (`accounts_in_app_team: false`), as
    /// that release judged it.
    fn may_reach_as_logged(
        &self,
        app: &AppRecord,
        account: Option<&str>,
        owner_in_team: bool,
        accounts_in_app_team: bool,
    ) -> bool {
        if let (Some(org), Some(a)) = (&app.org, account) {
            let member = self.orgs.get(org).and_then(|o| o.role(a)).is_some();
            if !member && app.access != Access::Public {
                return false;
            }
        }
        if let (Access::Accounts { .. }, Some(a), true) =
            (&app.access, account, accounts_in_app_team)
        {
            if let Some((o, team)) = self.app_team_of(app) {
                if !o.in_team(a, &team.team_id) {
                    return false;
                }
            }
        }
        match (&app.access, account) {
            (Access::Public, _) => true,
            (_, None) => false,
            // On a team, the owner too is judged by the team (below).
            (access, Some(a))
                if a == app.owner
                    && (!owner_in_team
                        || app.org.is_none()
                        || !matches!(access, Access::Team { .. })) =>
            {
                true
            }
            (Access::Owner, Some(_)) => false,
            (Access::Accounts { accounts }, Some(a)) => accounts.iter().any(|x| x == a),
            // Where apps have app teams, a guest has only the apps of the
            // teams they are in.
            (Access::Org, Some(a)) => app
                .org
                .as_ref()
                .and_then(|o| self.orgs.get(o))
                .is_some_and(|o| o.role(a).is_some() && !o.app_only(a)),
            (Access::Team { team }, Some(a)) => app
                .org
                .as_ref()
                .and_then(|o| self.orgs.get(o))
                .is_some_and(|o| o.in_team(a, team)),
        }
    }

    /// The app team of `app`'s package in `app`'s organization, with the
    /// organization, if both are there.
    #[must_use]
    pub fn app_team_of(&self, app: &AppRecord) -> Option<(&Org, &Team)> {
        let o = self.orgs.get(app.org.as_deref()?)?;
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
        let org = |id: &str| {
            app.org
                .as_deref()
                .filter(|o| *o == id)
                .and_then(|o| self.orgs.get(o))
        };
        match (audience, account) {
            (Audience::All, _) => true,
            (_, None) => false,
            (Audience::Caller, Some(a)) => emitter == Some(a),
            (Audience::Accounts { accounts }, Some(a)) => accounts.iter().any(|x| x == a),
            (Audience::Team { team }, Some(a)) => app
                .org
                .as_ref()
                .and_then(|o| self.orgs.get(o))
                .is_some_and(|o| o.in_team(a, team)),
            (Audience::OrgMembers { org: id }, Some(a)) => {
                org(id).is_some_and(|o| o.role(a).is_some_and(|r| r >= OrgRole::Member))
            }
            (Audience::OrgAdmins { org: id }, Some(a)) => {
                org(id).is_some_and(|o| o.role(a).is_some_and(|r| r >= OrgRole::Admin))
            }
        }
    }

    /// The apps deleting `team` takes with it: those of `org` on `team` or a
    /// team below it, in id order. None for a team `org` does not hold.
    #[must_use]
    pub fn team_apps(&self, org: &str, team: &str) -> Vec<AppId> {
        let Some(o) = self.orgs.get(org).filter(|o| o.teams.contains_key(team)) else {
            return Vec::new();
        };
        let gone = o.subtree(team);
        self.apps
            .values()
            .filter(|a| {
                a.org.as_deref() == Some(org)
                    && matches!(&a.access, Access::Team { team } if gone.contains(team))
            })
            .map(|a| a.app_id.clone())
            .collect()
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
fn scope_subject(scope: &ReleaseScope, actor: &str) -> String {
    match scope {
        ReleaseScope::Org { org } => format!("org:{org}"),
        ReleaseScope::Account => format!("account:{actor}"),
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
pub fn authorize(s: &SystemState, cmd: &SystemCmd) -> Result<Option<String>, &'static str> {
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
            Self::TransferApp { .. } => Area::Apps,
            Self::MoveApp { .. } => Area::Apps,
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
            Self::SetOutboundApproval { .. } => Area::Approvals,
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
pub fn api_key_args(hash: &str, record: &ApiKeyRecord) -> Value {
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
    args
}

impl SystemCmd {
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
                owner_in_team,
                accounts_in_app_team,
                member_child_teams,
                by_release,
                all_apps_rows,
                ..
            } => {
                *owner_in_team = true;
                *accounts_in_app_team = true;
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
pub fn move_app_args(to_org: &str, bring_members: bool) -> Value {
    json!({ "toOrg": to_org, "bringMembers": bring_members })
}

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
    /// reach it to the target too. [`SystemState::move_app`] has the rules.
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
        /// ([`app_team_id`]), so every replica makes the same one.
        #[serde(default)]
        nonce: String,
        /// An app team made in the target gives a row to each of its members
        /// who gets every app added later ([`crate::types::Org::all_apps`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays filling the rows it filled then.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_apps_rows: bool,
        /// An `Accounts` instance whose package has a closed app team in the
        /// target is reached there only through that team
        /// ([`SystemState::may_reach`]): who comes along with it, and its
        /// owner, get a row on that team. The API always sets it; absent
        /// (`false`) in an entry an earlier release logged, which replays
        /// giving no such rows, as that release did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        accounts_in_app_team: bool,
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
    SetAccess {
        app_id: AppId,
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
    /// (`__delete_deployment / {}`).
    DeleteDeployment {
        deployment: String,
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
    },
    StartUpgradeRun {
        org: OrgId,
        run: Box<crate::types::UpgradeRun>,
        /// The starter's passkey consent over the run, or the operator's
        /// signature for an operator's run; each app's `UpgradeApp` of the
        /// run is then judged by the run.
        #[serde(default)]
        consent: Value,
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
        /// (`org:<org> / __delete_team / { team, apps }`, [`team_apps`]).
        #[serde(default)]
        consent: Value,
        /// A team whose subtree holds apps needs `consent`. The API always
        /// sets it; absent (`false`) in an entry an earlier release logged,
        /// which replays deleting without one, as that release did.
        #[serde(default)]
        apps_need_consent: bool,
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
    /// Give apps their own access lists in one step (G3): for each package
    /// named (absent: every package with an organization-access instance
    /// here), find or make its app team ([`crate::types::Team::app_package`]),
    /// give every person who reaches its instances now a row there, and move
    /// the instances to `Access::Team` on it. `shared` (absent: whether the
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
        /// ([`app_team_id`]), so every replica makes the same ones.
        nonce: String,
        actor: AccountId,
        consent: Value,
        /// Who reaches an instance now is judged with an app on a team
        /// reached by its owner only while in it ([`SystemState::may_reach`]).
        /// The API always sets it; absent (`false`) in an entry an earlier
        /// release logged, which replays as that release judged it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        owner_in_team: bool,
        /// Who reaches an instance now is judged with an `Accounts` instance
        /// reached only through its package's app team
        /// ([`SystemState::may_reach`]). The API always sets it; absent
        /// (`false`) in an entry an earlier release logged, which replays as
        /// that release judged it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        accounts_in_app_team: bool,
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
/// logged), so replaying an earlier release's log adds nothing.
fn approval_of(
    s: &SystemState,
    cmd: &SystemCmd,
    used: &str,
) -> Option<(AccountId, crate::types::ActivityEntry)> {
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
    let text = |t: String| t.chars().take(crate::types::ACTIVITY_TEXT_CHARS).collect();
    Some((
        account,
        crate::types::ActivityEntry {
            at: consent.at,
            passkey,
            what: crate::types::Activity::Approval {
                method: text(consent.statement.method),
                subject: text(consent.statement.subject),
            },
        },
    ))
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
}

fn ok(v: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "ok": v })).unwrap_or_default()
}

fn err(msg: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "err": msg })).unwrap_or_default()
}

/// Read a system command's output.
pub fn output(bytes: &[u8]) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if let Some(e) = v.get("err") {
        return Err(e.as_str().unwrap_or("error").to_owned());
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
    fn run(&mut self, mut cmd: SystemCmd) -> Result<Value, String> {
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
            authorize(&self.state, &cmd).map_err(str::to_owned)?
        };
        if let Some(stall) = self.new_work_refusal(&cmd) {
            return Err(stall.message());
        }
        // Found before it applies: the command may remove the passkey.
        let approval = used
            .as_deref()
            .and_then(|used| approval_of(&self.state, &cmd, used));
        let out = self.run_authorized(cmd).map_err(str::to_owned)?;
        if let Some(used) = used {
            let _new = self.state.used_proofs.insert(used);
        }
        if let Some((account, entry)) = approval {
            self.state.record_activity(&account, entry);
        }
        Ok(out)
    }

    /// [`Self::run`] without the proof: the command's own rules and the
    /// new-work refusal, for tests of those rules.
    #[cfg(test)]
    fn run_unproven(&mut self, cmd: SystemCmd) -> Result<Value, String> {
        if let Some(stall) = self.new_work_refusal(&cmd) {
            return Err(stall.message());
        }
        self.run_authorized(cmd).map_err(str::to_owned)
    }

    /// Creating an app, upgrading one or installing a release is new work:
    /// past a release's deadline, refused for a scope that has not approved
    /// it. Judged at the time the command carries (the proposing node's
    /// clock, stamped once and replicated with it), so every replica, and
    /// every replay, decides the same. Anything else, calls included, goes on.
    fn new_work_refusal(&self, cmd: &SystemCmd) -> Option<Stall> {
        let s = &self.state;
        match cmd {
            SystemCmd::CreateApp { app } => s.stalled_for_release(
                s.governing_release(app),
                app.org.as_deref(),
                &app.owner,
                app.created_at,
            ),
            SystemCmd::UpgradeApp {
                app_id,
                bundle,
                at,
                release,
                ..
            } => {
                let a = s.apps.get(app_id)?;
                let to = s.governing(release.as_ref(), bundle, *at);
                s.stalled_for_release(to, a.org.as_deref(), &a.owner, *at)
            }
            SystemCmd::InstallPackage {
                release, signer_id, ..
            } => s.install_stalled(&release.installed_by, signer_id, release.installed_at),
            _ => None,
        }
    }

    /// Apply `cmd` by its own rules, its proof already judged ([`Self::run`]).
    fn run_authorized(&mut self, cmd: SystemCmd) -> Result<Value, &'static str> {
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
        }
    }
}

/// A refusal: no account recovery code is (still) known under that key.
pub const UNKNOWN_RECOVERY_CODE: &str = "unknown or used recovery code";
/// A refusal: a recovery of the account is under way already.
pub const ACCOUNT_RECOVERY_PENDING: &str = "a recovery of this account is under way";
/// A refusal: no recovery of the account is under way (for that passkey).
pub const NO_ACCOUNT_RECOVERY_PENDING: &str = "no recovery of this account is under way";

/// A refusal: the organization holds no recovery key.
pub const NO_RECOVERY_KEY: &str = "this organization has no recovery key";
/// A refusal: a recovery is under way already.
pub const RECOVERY_PENDING: &str = "a recovery of this organization is under way";
/// A refusal: the bundle a backup's state runs is no longer installed.
pub const RESTORE_BUNDLE_GONE: &str =
    "the bundle the backup's state runs is no longer installed: install that release again first";
/// A refusal: no recovery under way for that account.
pub const NO_RECOVERY_PENDING: &str =
    "no recovery of this organization is under way for that account";
/// A refusal: the recovery's delay has not passed.
pub const RECOVERY_NOT_READY: &str = "the recovery may finish only once its delay has passed";
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
        _state: &mut calimero_log::StateTx<'_>,
    ) -> Vec<u8> {
        self.index = index;
        match serde_json::from_slice::<SystemCmd>(command) {
            Ok(cmd) => match self.run(cmd) {
                Ok(v) => ok(v),
                Err(e) => err(&e),
            },
            Err(e) => {
                // A feature gate keeps every entry within what each node
                // applies (`cluster_version`): one that does not decode here
                // means a node proposed above it, and this node's state now
                // differs from the nodes that applied it.
                let n = crate::metrics::undecodable_entry(crate::metrics::LogKind::System);
                error!(index, error = %e, undecodable = n,
                    "a system entry this node cannot decode was skipped: its state now differs from the nodes that applied it");
                err(&format!("malformed command: {e}"))
            }
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        serde_json::to_vec(&self.state).unwrap_or_default()
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        self.state = serde_json::from_slice(data)?;
        Ok(())
    }
}

#[cfg(test)]
mod queue_tests;

#[cfg(test)]
mod tests;
