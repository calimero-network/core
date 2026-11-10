//! The system group: fleet, app directory, identity and organizations.
//!
//! Commands are applied in log order on every node. Each command that acts for
//! someone carries that `actor`, and `apply` checks their authority against
//! the state the command applies to, so the log alone decides every outcome.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use calimero_bundle::UpgradeAuthority;
use calimero_identity::Credential;
use calimero_log::Machine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::job_node::RUNS_PER_JOB_NODE;
use crate::types::{
    Access, Account, AccountId, ApiKeyRecord, AppId, AppRecord, Audience, Components, Invite,
    NodeRecord, NodeRole, Org, OrgId, OrgRole, RefreshRecord, Team, TeamId, TeamJoin, TeamRole,
};

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
const COMPUTE_NOT_SHIPPED: &str = "that compute component is not shipped with the app's bundle";

/// The refusal when an app names a release that does not ship its bundle.
const RELEASE_NOT_SHIPPED: &str = "that release does not ship the app's bundle";

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
    /// ([`SystemState::approved_for_bundle`]); where a package declares
    /// `upgrades: publisher`, its `MANAGE_APPLICATION` holders upgrade them
    /// ([`SystemState::may_upgrade`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub publishers: BTreeMap<String, PublisherBinding>,
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
/// A refusal: making, changing or dropping an app team, scoping apps, or
/// creating the first instance of a package without one in an organization
/// whose apps have app teams, is installing an app there: an admin's.
pub const ONLY_ADMINS_INSTALL: &str =
    "only the organization's owner and admins install apps in it and change its app teams";
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

    /// Grants waiting runs up to the job nodes' places, to the payers in turn (longest served
    /// first); a payer at a quota is passed over for the pass. Granted runs stay.
    fn grant_runs(&mut self) {
        for slot in self.compute.values_mut().flat_map(BTreeMap::values_mut) {
            let fuel = limits_of(&self.quotas, &self.org_quotas, &slot.payer).1;
            slot.refused |= !slot.granted && slot.fuel > fuel;
        }
        let mut granted_in: BTreeMap<String, usize> = BTreeMap::new();
        let mut load: BTreeMap<String, (u32, u64)> = BTreeMap::new();
        let mut waiting: BTreeMap<String, Vec<WaitingRun>> = BTreeMap::new();
        for (app, runs) in &self.compute {
            for (run, slot) in runs {
                if slot.refused {
                    continue;
                }
                if slot.granted {
                    *granted_in
                        .entry(self.slot_pool(slot).to_owned())
                        .or_default() += 1;
                    let held = load.entry(slot.payer.clone()).or_default();
                    held.0 += 1;
                    held.1 = held.1.saturating_add(slot.fuel);
                } else {
                    waiting.entry(slot.payer.clone()).or_default().push((
                        slot.seq,
                        app.clone(),
                        run.clone(),
                        slot.fuel,
                        self.slot_pool(slot).to_owned(),
                    ));
                }
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
        self.approved_for_bundle(&app.bundle, app.org.as_deref(), &app.owner)
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
        self.publishers.get(signer).map_or(0, |b| b.generation)
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

    /// The organization that publishes the release `bundle` ships in; `None`
    /// for a raw upload or a package whose key is bound to no organization.
    #[must_use]
    pub fn publisher_of(&self, bundle: &str) -> Option<&OrgId> {
        let at = self.bundles.get(bundle)?.package.as_ref()?;
        self.publisher_of_signer(&self.packages.get(&at.package)?.signer_id)
    }

    /// The releases approved to hold an app running `bundle`, of `org` (or of
    /// `owner` outside one): the operator's where it holds release
    /// approvals; else the bundle's publisher's, for every instance of its
    /// packages; else, for a raw upload or an unbound package, the app's own
    /// organization's or owner's ([`Self::approved_for`]).
    #[must_use]
    pub fn approved_for_bundle(
        &self,
        bundle: &str,
        org: Option<&str>,
        owner: &str,
    ) -> Option<&BTreeSet<String>> {
        match self.publisher_of(bundle) {
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

    /// Who upgrades an app running `bundle`: its release's declaration; a raw
    /// upload is its owner's.
    #[must_use]
    pub fn upgrades_of(&self, bundle: &str) -> UpgradeAuthority {
        self.bundles
            .get(bundle)
            .and_then(|b| b.package.as_ref())
            .and_then(|at| self.packages.get(&at.package)?.versions.get(&at.version))
            .map_or(UpgradeAuthority::Owner, |v| v.upgrades)
    }

    /// The publisher that upgrades an app running `bundle`: only when its
    /// release declares `publisher` and its key is bound to an organization.
    #[must_use]
    pub fn upgraded_by_publisher(&self, bundle: &str) -> Option<&OrgId> {
        if self.upgrades_of(bundle).is_owner() {
            return None;
        }
        self.publisher_of(bundle)
    }

    /// Whether moving an app from `from` to `to` keeps upgrades where they
    /// may go: never from an `owner` release to a `publisher` one.
    #[must_use]
    pub fn upgrade_keeps_authority(&self, from: &str, to: &str) -> bool {
        !(self.upgrades_of(from).is_owner() && !self.upgrades_of(to).is_owner())
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
        if !self.upgrades_of(&app.bundle).is_owner() {
            return self
                .upgraded_by_publisher(&app.bundle)
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

/// How the release that logged a [`SystemCmd::ScopeApps`] judged it: each
/// flag is the entry's own, absent (`false`) in an earlier release's entry.
#[derive(Clone, Copy, Debug)]
struct ScopeAsLogged {
    /// An owner reaches an app on a team only while in it.
    owner_in_team: bool,
    /// A new app team's default holds `CREATE_CHILD_TEAM`.
    member_child_teams: bool,
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
                .filter_map(|a| self.package_of(a))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|package| ScopeApp {
                    package: package.to_owned(),
                    name: None,
                    instances: None,
                    join: None,
                    keep: None,
                })
                .collect(),
        };
        if specs.len() > MAX_SCOPE_PACKAGES {
            return Err("too many packages in one step");
        }
        // A package's own `join` decides its team and no other: `shared`
        // then closes nothing more unless asked, and never one opened here.
        let per_app = specs.iter().any(|a| a.join.is_some());
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
            let of_package = |a: &AppRecord| self.package_of(a) == Some(spec.package.as_str());
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
                            self.may_reach_as_logged(a, Some(m.as_str()), logged.owner_in_team)
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
            // Closing with a list: the people the admin keeps, whoever
            // reaches the moved instances or not (the other rows go below).
            // Otherwise everyone who reaches them, and, closing, everyone in
            // the team now.
            let seed: BTreeSet<AccountId> = match (&plan.keep, plan.join) {
                (Some(keep), _) => keep.clone(),
                (None, Some(TeamJoin::Invite)) => {
                    let mut all = plan.reach.clone();
                    all.extend(o.members.keys().filter(|m| o.in_team(m, &team_id)).cloned());
                    all
                }
                (None, _) => plan.reach.clone(),
            };
            let t = o.teams.get_mut(&team_id).ok_or("no such team")?;
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
                            .filter(|(a, lead)| !**lead && !keep.contains(*a))
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
                    let below: Vec<TeamId> = o.subtree(&team_id).into_iter().skip(1).collect();
                    for id in &below {
                        if let Some(t) = o.teams.get_mut(id) {
                            for a in &gone {
                                let _prev = t.members.remove(a);
                                let _was = t.read_only.remove(a);
                                let _caps = t.capabilities.remove(a);
                            }
                        }
                    }
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
    ///   accounts access stay as they are. Any other team is refused
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
    ) -> Result<Value, &'static str> {
        use crate::types::{caps, OrgMode};

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

        let before = self.approved_for_bundle(&app.bundle, from.as_deref(), &app.owner);
        let after = self.approved_for_bundle(&app.bundle, Some(to_org), &app.owner);
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
            for m in reached.iter().map(|(m, _)| m).chain(owner_keeps.iter()) {
                let _back = t.removed.remove(m);
                let _row = t.members.entry(m.clone()).or_insert(false);
            }
            access = Access::Team { team: team_id };
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

    /// [`Self::stalled`] for an app of `org` (or of `owner`) running `bundle`:
    /// judged on the set that governs it ([`Self::approved_for_bundle`]).
    #[must_use]
    pub fn stalled_for_bundle(
        &self,
        bundle: &str,
        org: Option<&str>,
        owner: &str,
        now: u64,
    ) -> Option<Stall> {
        self.stalled(self.approved_for_bundle(bundle, org, owner), now)
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
    /// ([`Self::owner_acts`]).
    #[must_use]
    pub fn may_reach(&self, app: &AppRecord, account: Option<&str>) -> bool {
        self.may_reach_as_logged(app, account, true)
    }

    /// [`Self::may_reach`], or, for a command an earlier release logged
    /// (`owner_in_team: false`), with an app on a team reached by its owner
    /// whatever the team holds, as that release judged it.
    fn may_reach_as_logged(
        &self,
        app: &AppRecord,
        account: Option<&str>,
        owner_in_team: bool,
    ) -> bool {
        if let (Some(org), Some(a)) = (&app.org, account) {
            let member = self.orgs.get(org).and_then(|o| o.role(a)).is_some();
            if !member && app.access != Access::Public {
                return false;
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
    use crate::proof::{verify_consent, verify_operator, Statement};

    let consent_of = |actor: &str, consent: &Value| -> Result<(Statement, String), &'static str> {
        let credentials = s
            .accounts
            .get(actor)
            .map(|a| a.passkeys.as_slice())
            .ok_or("unknown account")?;
        verify_consent(consent, credentials)
    };
    let operator = |proof: &Value| verify_operator(proof, s.operator_key.as_ref());
    let fresh =
        |(statement, used): (Statement, String)| -> Result<(Statement, String), &'static str> {
            if s.used_proofs.contains(&used) {
                return Err("that consent was used already");
            }
            Ok((statement, used))
        };
    let states = |st: &Statement, subject: &str, method: &str, args: &dyn Fn(&Value) -> bool| {
        if st.subject == subject && st.method == method && args(&st.args()) {
            Ok(())
        } else {
            Err("the consent is for another change")
        }
    };
    // A deployer grant or revocation: the operator's signature while it
    // grants deployers, else the passkey consent of the account acting for
    // the cluster's owner; never the other one's.
    let deployer_proof = |actor: &str, proof: Option<&Value>, consent: Option<&Value>| match (
        s.deployers_owner(),
        proof,
        consent,
    ) {
        (None, Some(proof), None) if actor == OPERATOR => fresh(operator(proof)?),
        (None, ..) => Err("the operator grants this cluster's deployers, with its signature"),
        (Some(_), None, Some(consent)) => fresh(consent_of(actor, consent)?),
        (Some(_), ..) => Err("this cluster's owner grants its deployers, with a passkey"),
    };
    // The sign-up policy and sign-up invites: the same two parties.
    let sign_up_proof = |actor: &str, proof: Option<&Value>, consent: Option<&Value>| match (
        s.deployers_owner(),
        proof,
        consent,
    ) {
        (None, Some(proof), None) if actor == OPERATOR => fresh(operator(proof)?),
        (None, ..) => Err("the operator runs this cluster's sign-up, with its signature"),
        (Some(_), None, Some(consent)) => fresh(consent_of(actor, consent)?),
        (Some(_), ..) => Err("this cluster's owner runs its sign-up, with a passkey"),
    };

    match cmd {
        SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator,
            actor,
            at,
            run: None,
            consent,
            ..
        } => {
            // An upgrade key's signature, judged against the key as the log
            // holds it now: revoked or expired, it proves nothing.
            if let (false, Some(key_id)) = (*by_operator, crate::proof::key_proof_id(consent)) {
                let key = s
                    .api_keys
                    .values()
                    .find(|k| k.key_id == key_id && k.account == *actor)
                    .ok_or("no such API key: it was revoked")?;
                let scope = s.key_may_upgrade(key, app_id, bundle, *at)?;
                let (_, st, used) = crate::proof::verify_key_upgrade(consent, &scope.proof_key)?;
                let (st, used) = fresh((st, used))?;
                states(&st, app_id, "__upgrade", &|a| a["bundle"] == json!(bundle))?;
                return Ok(Some(used));
            }
            let (st, used) = if *by_operator {
                fresh(operator(consent)?)?
            } else {
                fresh(consent_of(actor, consent)?)?
            };
            states(&st, app_id, "__upgrade", &|a| a["bundle"] == json!(bundle))?;
            Ok(Some(used))
        }
        SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator,
            actor,
            run: Some(run),
            release: target,
            ..
        } => {
            // One consent covers the run (`StartUpgradeRun`); each app's
            // upgrade must be one the run named, to the release it names.
            // The run is kept by the organization that started it: the app's
            // own, or the publisher's for a publisher's run over every
            // instance of its package (as `note_run` finds it).
            let r = s
                .orgs
                .values()
                .find_map(|o| o.upgrade_runs.get(run))
                .ok_or("no such upgrade run")?;
            let by = if *by_operator {
                OPERATOR
            } else {
                actor.as_str()
            };
            let release = s
                .packages
                .get(&r.package)
                .and_then(|p| p.versions.get(&r.version));
            if r.by != by
                || r.finished
                || !r.apps.contains(app_id)
                || !release.is_some_and(|v| v.app_parts().values().any(|h| h == bundle))
                || target
                    .as_ref()
                    .is_some_and(|t| t.package != r.package || t.version != r.version)
            {
                return Err("the upgrade is not one its run names");
            }
            Ok(None)
        }
        SystemCmd::StartUpgradeRun { org, run, consent } => {
            let (st, used) = if run.by == OPERATOR {
                fresh(operator(consent)?)?
            } else {
                fresh(consent_of(&run.by, consent)?)?
            };
            let subject = format!("org:{org}");
            let same = |a: &Value| {
                a["package"] == json!(run.package)
                    && a["version"] == json!(run.version)
                    && a["team"] == json!(run.team)
                    && a["cascade"].as_bool().unwrap_or(false) == run.cascade
            };
            if st.method == "__upgrade_published" {
                // A publisher's run over every instance of its package: no
                // team; each app is still judged at apply (`may_upgrade`).
                states(&st, &subject, "__upgrade_published", &|a| {
                    a["package"] == json!(run.package) && a["version"] == json!(run.version)
                })?;
                if run.team.is_some() || run.cascade {
                    return Err("the consent is for another change");
                }
            } else if st.method == "__upgrade_org_retry" {
                // A retry repeats a run of the same org under its terms.
                let prev = st.args()["run"]
                    .as_str()
                    .and_then(|p| s.orgs.get(org)?.upgrade_runs.get(p))
                    .ok_or("the consent is for another change")?;
                let terms = json!({
                    "package": prev.package, "version": prev.version,
                    "team": prev.team, "cascade": prev.cascade,
                });
                states(&st, &subject, "__upgrade_org_retry", &|_| same(&terms))?;
            } else {
                states(&st, &subject, "__upgrade_org", &same)?;
            }
            Ok(Some(used))
        }
        SystemCmd::TransferApp {
            app_id,
            to,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(&st, app_id, "__transfer", &|a| a["to"] == json!(to))?;
            Ok(Some(used))
        }
        SystemCmd::MoveApp {
            app_id,
            to_org,
            actor,
            consent,
            bring_members,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = move_app_args(to_org, *bring_members);
            states(&st, app_id, "__move", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetReleaseApproval {
            scope,
            version,
            approved,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let method = if *approved {
                "__approve_release"
            } else {
                "__withdraw_release"
            };
            states(&st, &scope_subject(scope, actor), method, &|a| {
                a["version"] == json!(version)
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetOperatorRelease {
            version,
            approved,
            proof,
        } => {
            let (st, used) = fresh(operator(proof)?)?;
            let method = if *approved {
                "__approve_release"
            } else {
                "__withdraw_release"
            };
            states(&st, "cluster", method, &|a| a["version"] == json!(version))?;
            Ok(Some(used))
        }
        SystemCmd::SetOutboundApproval {
            scope,
            package,
            selection,
            replace,
            actor,
            consent,
        } => {
            let (st, used) = if *scope == ReleaseScope::Cluster {
                fresh(operator(consent)?)?
            } else {
                fresh(consent_of(actor, consent)?)?
            };
            states(
                &st,
                &scope_subject(scope, actor),
                "__approve_outbound",
                &|a| {
                    a["package"] == json!(package)
                        && a["prefixes"] == json!(selection)
                        && a["replace"].as_bool().unwrap_or(false) == *replace
                },
            )?;
            Ok(Some(used))
        }
        SystemCmd::BindPublisher {
            signer_id,
            org,
            actor,
            consent,
            ..
        } => {
            // Over this key's binding as it stands when the command applies:
            // a consent for an earlier generation binds nothing again.
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let generation = s.binding_generation(signer_id);
            states(&st, &format!("org:{org}"), "__bind_publisher", &|a| {
                a["signerId"] == json!(signer_id) && a["generation"] == json!(generation)
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetReleaseDeadline {
            version,
            after,
            proof,
        } => {
            let (st, used) = fresh(operator(proof)?)?;
            states(&st, "cluster", "__release_deadline", &|a| {
                a["version"] == json!(version) && a["after"] == json!(after)
            })?;
            Ok(Some(used))
        }
        SystemCmd::GrantDeployer {
            deployer,
            until,
            actor,
            proof,
            consent,
            ..
        } => {
            let (st, used) = deployer_proof(actor, proof.as_ref(), consent.as_ref())?;
            states(&st, "cluster", "__grant_deployer", &|a| {
                a["deployer"] == json!(deployer) && a["until"] == json!(until)
            })?;
            Ok(Some(used))
        }
        SystemCmd::RevokeDeployer {
            deployer,
            actor,
            proof,
            consent,
            ..
        } => {
            let (st, used) = deployer_proof(actor, proof.as_ref(), consent.as_ref())?;
            states(&st, "cluster", "__revoke_deployer", &|a| {
                a["deployer"] == json!(deployer)
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetDeploymentsGranted { proof } => {
            let (st, used) = fresh(operator(proof)?)?;
            states(&st, "cluster", "__deployments_granted", &|_| true)?;
            Ok(Some(used))
        }
        SystemCmd::SetClusterOwner { owner, proof } => {
            let (st, used) = fresh(operator(proof)?)?;
            states(&st, "cluster", "__cluster_owner", &|a| {
                a["owner"] == json!(owner)
            })?;
            Ok(Some(used))
        }
        SystemCmd::CreateDeployment {
            id,
            name,
            owner,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(&st, "cluster", "__create_deployment", &|a| {
                a["name"] == json!(name) && a["owner"] == json!(owner)
            })?;
            // The id is the consent's own, so a node cannot pick another.
            if *id != deployment_id(&st.nonce) {
                return Err("the consent is for another change");
            }
            Ok(Some(used))
        }
        SystemCmd::OfferApp {
            deployment,
            app,
            package,
            version,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(
                &st,
                &format!("deployment:{deployment}"),
                "__offer_app",
                &|a| {
                    a["app"] == json!(app)
                        && a["package"] == json!(package)
                        && a["version"] == json!(version)
                },
            )?;
            Ok(Some(used))
        }
        SystemCmd::WithdrawApp {
            deployment,
            app,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(
                &st,
                &format!("deployment:{deployment}"),
                "__withdraw_app",
                &|a| a["app"] == json!(app),
            )?;
            Ok(Some(used))
        }
        SystemCmd::DeleteDeployment {
            deployment,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(
                &st,
                &format!("deployment:{deployment}"),
                "__delete_deployment",
                &|_| true,
            )?;
            Ok(Some(used))
        }
        SystemCmd::AddPackageOrigin {
            package,
            origin,
            rp_id,
            served,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(&st, &format!("package:{package}"), "__add_origin", &|a| {
                *a == crate::origins::add_args(origin, rp_id, *served)
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemovePackageOrigin {
            package,
            origin,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(
                &st,
                &format!("package:{package}"),
                "__remove_origin",
                &|a| *a == json!({ "origin": origin }),
            )?;
            Ok(Some(used))
        }
        SystemCmd::DeleteOrg {
            org,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(&st, &format!("org:{org}"), "__delete_org", &|a| {
                a["org"] == json!(org)
            })?;
            Ok(Some(used))
        }
        // Credentials: the account's own passkey consent. A new passkey is
        // approved by one the account already has (`consent_of` reads the
        // passkeys as they stand, without the new one).
        SystemCmd::AddPasskey {
            account,
            credential,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(account, consent)?)?;
            let args = json!({
                "credentialId": credential.id,
                "publicKey": credential.public_key,
            });
            states(&st, &format!("account:{account}"), "__add_passkey", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemovePasskey {
            account,
            credential_id,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(account, consent)?)?;
            let args = json!({ "credentialId": credential_id });
            states(
                &st,
                &format!("account:{account}"),
                "__remove_passkey",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        // Account recovery (`docs/specs/account-recovery.md`): making and
        // removing a code, and cancelling a recovery, are the account's own
        // passkey consent; starting is the code key's signature over the
        // new passkey; finishing is that new passkey's sign-in assertion
        // over the session it opens.
        SystemCmd::SetAccountRecoveryKey {
            account,
            recovery_key,
            consent,
        } => {
            let (st, used) = fresh(consent_of(account, consent)?)?;
            let args = json!({ "recoveryKey": recovery_key });
            states(&st, &format!("account:{account}"), "__set_recovery", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemoveAccountRecoveryKey { account, consent } => {
            let (st, used) = fresh(consent_of(account, consent)?)?;
            states(
                &st,
                &format!("account:{account}"),
                "__remove_recovery",
                &|a| *a == json!({}),
            )?;
            Ok(Some(used))
        }
        // The code's key signs the start: only a node the person typed the
        // code into can make it, never one that only saw the log.
        SystemCmd::StartAccountRecovery {
            recovery_key,
            account,
            credential,
            at,
            signature,
        } => {
            if s.recovery_keys.get(recovery_key) != Some(account) {
                return Err(UNKNOWN_RECOVERY_CODE);
            }
            let used =
                crate::proof::verify_recover(recovery_key, account, credential, *at, signature)?;
            if s.used_proofs.contains(&used) {
                return Err("that recovery was used already");
            }
            Ok(Some(used))
        }
        SystemCmd::CancelAccountRecovery {
            account,
            credential_id,
            consent,
        } => {
            let (st, used) = fresh(consent_of(account, consent)?)?;
            let args = json!({ "credentialId": credential_id });
            states(
                &st,
                &format!("account:{account}"),
                "__cancel_recovery",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        // The passkey the start logged signs the session the finish opens,
        // as any sign-in does (`CreateRefresh`): user verified, over a
        // challenge stating that refresh token, for that passkey, unbound.
        SystemCmd::FinishAccountRecovery {
            account,
            refresh,
            consent,
            ..
        } => {
            let pending = s
                .account_recoveries
                .get(account)
                .ok_or(NO_ACCOUNT_RECOVERY_PENDING)?;
            let new = std::slice::from_ref(&pending.credential);
            let (st, used) = fresh(verify_consent(consent, new)?)?;
            if refresh.record.account != *account
                || refresh.record.passkey.as_deref() != Some(pending.credential.id.as_str())
                || refresh.record.app.is_some()
            {
                return Err("the session is for another passkey");
            }
            let args = sign_in_args(&refresh.hash, &refresh.record);
            states(&st, "session", "__sign_in", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::CreateApiKey {
            hash,
            record,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(&record.account, consent)?)?;
            let args = api_key_args(hash, record);
            states(
                &st,
                &format!("account:{}", record.account),
                "__create_api_key",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        SystemCmd::RevokeApiKey {
            key_id,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(&st, &format!("account:{actor}"), "__revoke_api_key", &|a| {
                *a == json!({ "keyId": key_id })
            })?;
            Ok(Some(used))
        }
        // Sessions. A sign-up: the new passkey's assertion over the whole
        // account and the session it opens, so a node that saw someone's new
        // credential cannot register it first under an account and a release
        // of its own.
        SystemCmd::CreateAccount {
            account,
            invite,
            refresh,
            consent,
            ..
        } => {
            let (st, used) = fresh(crate::proof::verify_sign_in(consent, &account.passkeys)?)?;
            // An invite at sign-up: the token's key signs it, as a redemption
            // of the new account, so only the node the person gave it to can.
            // Its mark is spent where the account applies (`run_authorized`).
            if let Some(r) = invite {
                if !s.invites.contains_key(&r.invite_key)
                    && !s.sign_up_invites.contains_key(&r.invite_key)
                {
                    return Err(
                        "sign-up refused: this invite was used up, revoked or never existed",
                    );
                }
                let redeemed = crate::proof::verify_invite_redeem(
                    &r.invite_key,
                    &account.account_id,
                    r.now,
                    &r.signature,
                )?;
                if s.used_proofs.contains(&redeemed) {
                    return Err("that redemption was used already");
                }
            }
            if refresh
                .as_ref()
                .is_some_and(|g| g.record.account != account.account_id)
            {
                return Err("the consent is for another change");
            }
            let args = account_args(account, refresh.as_ref());
            states(
                &st,
                &format!("account:{}", account.account_id),
                "__create_account",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        // A sign-in: the assertion it took, by the passkey the session names,
        // over a challenge that states this refresh token.
        SystemCmd::CreateRefresh {
            hash,
            record,
            consent,
        } => {
            let passkey: Vec<Credential> = s
                .accounts
                .get(&record.account)
                .ok_or("unknown account")?
                .passkeys
                .iter()
                .filter(|c| record.passkey.as_deref() == Some(c.id.as_str()))
                .cloned()
                .collect();
            let (st, used) = fresh(crate::proof::verify_sign_in(consent, &passkey)?)?;
            let args = sign_in_args(hash, record);
            states(&st, "session", "__sign_in", &|a| *a == args)?;
            Ok(Some(used))
        }
        // An exchange: the old token's key, over the new token and the
        // session it continues, which stays the old one's.
        SystemCmd::UseRefresh {
            old_hash,
            new_hash,
            record,
            now,
            signature,
        } => {
            let old = s.refresh.get(old_hash).ok_or("unknown refresh token")?;
            if old.key.is_empty() {
                return Err("this refresh token has no key; sign in again");
            }
            if old.account != record.account
                || old.session != record.session
                || old.passkey != record.passkey
                || old.app != record.app
                || old.origin != record.origin
            {
                return Err("the exchange is for another session");
            }
            let message = crate::proof::refresh_use_message(
                old_hash,
                new_hash,
                &record.key,
                &record.session,
                record.expires_at,
                *now,
            );
            crate::proof::verify_refresh_use(&old.key, &message, signature)?;
            Ok(None)
        }
        // A counter: an assertion of that passkey carrying it. Only the
        // authenticator signs a counter, so none is set past what it reached.
        SystemCmd::SetSignCount {
            credential_id,
            count,
            assertion,
        } => {
            let (_, credential) = s
                .account_of_credential(credential_id)
                .ok_or("unknown credential")?;
            let assertion: calimero_identity::Assertion = serde_json::from_value(assertion.clone())
                .map_err(|_| "the counter carries no assertion")?;
            let carried = calimero_identity::assertion_counter(credential, &assertion)
                .map_err(|_| "the assertion does not verify")?;
            if carried != *count {
                return Err("the assertion carries another counter");
            }
            Ok(None)
        }
        // Memberships: the actor's passkey consent over exactly this change
        // (a member leaving consents to that too). Who may make it is the
        // command's own rule (`run_authorized`).
        SystemCmd::SetOrgRole {
            org,
            account,
            role,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "account": account, "role": role });
            states(&st, &format!("org:{org}"), "__set_org_role", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemoveOrgMember {
            org,
            account,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "account": account });
            states(&st, &format!("org:{org}"), "__remove_member", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetTeamMember {
            org,
            team,
            account,
            lead,
            role,
            capabilities,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({
                "team": team, "account": account, "lead": lead,
                "role": role, "capabilities": capabilities,
            });
            states(&st, &format!("org:{org}"), "__set_team_member", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        // Deleting a team takes the apps below it: with apps there, the
        // actor's passkey over exactly those apps, so one added since the
        // approval is not taken silently.
        SystemCmd::DeleteTeam {
            org,
            team,
            actor,
            consent,
            apps_need_consent: true,
        } => {
            let apps = s.team_apps(org, team);
            if apps.is_empty() {
                return Ok(None);
            }
            if consent.is_null() {
                return Err(DELETE_TEAM_NEEDS_CONSENT);
            }
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "team": team, "apps": apps });
            states(&st, &format!("org:{org}"), "__delete_team", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RemoveTeamMember {
            org,
            team,
            account,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "team": team, "account": account });
            states(&st, &format!("org:{org}"), "__remove_team_member", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetCapabilities {
            org,
            team,
            account,
            capabilities,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "team": team, "account": account, "capabilities": capabilities });
            states(&st, &format!("org:{org}"), "__set_capabilities", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetDefaultCapabilities {
            org,
            team,
            capabilities,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = default_capabilities_args(team.as_deref(), *capabilities);
            states(
                &st,
                &format!("org:{org}"),
                "__set_default_capabilities",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        SystemCmd::CreateInvite {
            invite_key,
            invite,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = invite_args(invite_key, invite);
            states(
                &st,
                &format!("org:{}", invite.org),
                "__create_invite",
                &|a| *a == args,
            )?;
            Ok(Some(used))
        }
        SystemCmd::RevokeInvite {
            org,
            invite_key,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "inviteKey": invite_key });
            states(&st, &format!("org:{org}"), "__revoke_invite", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::ScopeApps {
            org,
            apps,
            shared,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = scope_apps_args(apps.as_deref(), *shared);
            states(&st, &format!("org:{org}"), "__scope_apps", &|a| *a == args)?;
            Ok(Some(used))
        }
        // The token's key signs the redemption: only a node the redeemer
        // gave the token to can make it, never one that only saw the log.
        SystemCmd::RedeemInvite {
            invite_key,
            account,
            now,
            signature,
            ..
        } => {
            if !s.invites.contains_key(invite_key) {
                return Err(INVITE_GONE);
            }
            let used = crate::proof::verify_invite_redeem(invite_key, account, *now, signature)?;
            if s.used_proofs.contains(&used) {
                return Err("that redemption was used already");
            }
            Ok(Some(used))
        }
        // Recovery: setting the key and cancelling are the owner's passkey
        // consent, finishing the recovering account's; starting is the
        // recovery key's signature, checked against the key the organization
        // holds now, so a rotated key's signatures are refused.
        SystemCmd::SetRecoveryKey {
            org,
            public_key,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "publicKey": public_key });
            states(&st, &format!("org:{org}"), "__set_recovery_key", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::StartRecovery {
            org,
            account,
            nonce,
            signature,
            ..
        } => {
            let key = s
                .orgs
                .get(org)
                .ok_or("no such organization")?
                .recovery
                .as_ref()
                .ok_or(NO_RECOVERY_KEY)?;
            let used =
                crate::proof::verify_recovery(&key.public_key, org, account, nonce, signature)?;
            if s.used_proofs.contains(&used) {
                return Err("that recovery signature was used already");
            }
            Ok(Some(used))
        }
        SystemCmd::CancelRecovery {
            org,
            account,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "account": account });
            states(&st, &format!("org:{org}"), "__cancel_recovery", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::FinishRecovery {
            org,
            actor,
            consent,
            ..
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "org": org });
            states(&st, &format!("org:{org}"), "__finish_recovery", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetSignUp {
            policy,
            actor,
            proof,
            consent,
        } => {
            let (st, used) = sign_up_proof(actor, proof.as_ref(), consent.as_ref())?;
            let args = json!({ "policy": policy });
            states(&st, "cluster", "__sign_up", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::CreateSignUpInvite {
            invite_key,
            invite,
            actor,
            proof,
            consent,
        } => {
            let (st, used) = sign_up_proof(actor, proof.as_ref(), consent.as_ref())?;
            let args = sign_up_invite_args(invite_key, invite);
            states(&st, "cluster", "__create_sign_up_invite", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RevokeSignUpInvite {
            invite_key,
            actor,
            proof,
            consent,
        } => {
            let (st, used) = sign_up_proof(actor, proof.as_ref(), consent.as_ref())?;
            let args = json!({ "inviteKey": invite_key });
            states(&st, "cluster", "__revoke_sign_up_invite", &|a| *a == args)?;
            Ok(Some(used))
        }
        // The fleet: the operator's signature.
        SystemCmd::RegisterNode { node, proof } => {
            let (st, used) = fresh(operator(proof)?)?;
            let mut args = json!({ "id": node.id, "addr": node.addr, "zone": node.zone });
            if node.role != NodeRole::Replica {
                args["role"] = json!(node.role);
            }
            if !node.pool.is_empty() {
                args["pool"] = json!(node.pool);
            }
            states(&st, "cluster", "__register_node", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RemoveNode { id, proof } => {
            let (st, used) = fresh(operator(proof)?)?;
            states(&st, "cluster", "__remove_node", &|a| {
                *a == json!({ "id": id })
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetQuotas { quotas, proof } => {
            let (st, used) = fresh(operator(proof)?)?;
            let args = json!({ "quotas": quotas });
            states(&st, "cluster", "__set_quotas", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetOrgQuotas {
            payer,
            quotas,
            proof,
        } => {
            let (st, used) = fresh(operator(proof)?)?;
            let args = json!({ "payer": payer, "quotas": quotas });
            states(&st, "cluster", "__set_org_quotas", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetPools { pools, proof } => {
            let (st, used) = fresh(operator(proof)?)?;
            let args = json!({ "pools": pools });
            states(&st, "cluster", "__set_pools", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetReplicas {
            app_id,
            replicas,
            proof,
        } => {
            // A move writes the replicas twice under one signature over its
            // target (`api::move_replicas`): first the union of the current
            // set and the target, so every node of both starts the group,
            // then the target. The target spends the proof; the union, as
            // computed here from the state it applies to, needs it unspent
            // and leaves it so. Replaying the union after the move is
            // refused (the proof is spent), and so is any other set.
            let (st, used) = fresh(operator(proof)?)?;
            let target: BTreeSet<u64> = serde_json::from_value(st.args()["replicas"].clone())
                .map_err(|_| "the consent is for another change")?;
            let args = json!({ "replicas": target });
            states(&st, app_id, "__set_replicas", &|a| *a == args)?;
            let wanted: BTreeSet<u64> = replicas.iter().copied().collect();
            if wanted.len() != replicas.len() {
                return Err("a node is named twice");
            }
            if wanted == target {
                return Ok(Some(used));
            }
            let current = s.apps.get(app_id).ok_or("no such app")?.replicas.iter();
            let union: BTreeSet<u64> = current.copied().chain(target).collect();
            if wanted == union {
                return Ok(None);
            }
            Err("the consent is for another change")
        }
        _ => Ok(None),
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
    json!({
        "inviteKey": invite_key,
        "role": invite.role,
        "teams": invite.teams,
        "expiresAt": invite.expires_at,
        "uses": invite.uses_left,
    })
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

/// The most packages one `ScopeApps` names.
pub const MAX_SCOPE_PACKAGES: usize = 64;
/// The most instances one package of a `ScopeApps` names.
pub const MAX_SCOPE_INSTANCES: usize = 1024;
/// The most accounts one package of a `ScopeApps` keeps on a close.
pub const MAX_SCOPE_KEEP: usize = 1024;
/// The longest app team name, in characters.
pub const MAX_APP_TEAM_NAME_CHARS: usize = 64;

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
        /// A new app team's default capabilities let its members create
        /// restricted teams below it (`caps::CREATE_CHILD_TEAM`). The API
        /// always sets it; absent (`false`) in an entry an earlier release
        /// logged, which replays making the default it made then.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        member_child_teams: bool,
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
    fn run(&mut self, cmd: SystemCmd) -> Result<Value, String> {
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
            SystemCmd::CreateApp { app } => {
                s.stalled_for_bundle(&app.bundle, app.org.as_deref(), &app.owner, app.created_at)
            }
            SystemCmd::UpgradeApp {
                app_id, bundle, at, ..
            } => {
                let a = s.apps.get(app_id)?;
                s.stalled_for_bundle(bundle, a.org.as_deref(), &a.owner, *at)
            }
            SystemCmd::InstallPackage {
                release, signer_id, ..
            } => s.install_stalled(&release.installed_by, signer_id, release.installed_at),
            _ => None,
        }
    }

    /// Apply `cmd` by its own rules, its proof already judged ([`Self::run`]).
    #[allow(clippy::too_many_lines, reason = "one arm per command")]
    fn run_authorized(&mut self, cmd: SystemCmd) -> Result<Value, &'static str> {
        let index = self.index;
        let s = &mut self.state;
        match cmd {
            SystemCmd::RegisterNode { node, .. } => {
                if s.nodes.get(&node.id).is_some_and(|n| n.role != node.role) {
                    return Err("a node's role is fixed: remove it first");
                }
                let addr = normalize_addr(&node.addr);
                if s.nodes
                    .values()
                    .any(|n| n.id != node.id && normalize_addr(&n.addr) == addr)
                {
                    return Err("another node has that address");
                }
                *s.node_epoch.entry(node.id).or_default() += 1;
                let _was = s.removed_nodes.remove(&node.id);
                let _prev = s.nodes.insert(node.id, node);
                s.grant_runs();
                Ok(Value::Null)
            }
            SystemCmd::RemoveNode { id, .. } => {
                if s.apps.values().any(|a| a.replicas.contains(&id)) {
                    return Err("an app still keeps a replica on that node; move it first");
                }
                let _gone = s.nodes.remove(&id).ok_or("no such node")?;
                *s.node_epoch.entry(id).or_default() += 1;
                let _new = s.removed_nodes.insert(id);
                s.grant_runs();
                Ok(Value::Null)
            }
            SystemCmd::PutBundle {
                hash,
                size,
                actor,
                at,
            } => {
                if !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
                }
                if !s.may_deploy(&actor, at) {
                    return Err(NOT_A_DEPLOYER);
                }
                if !s.bundles.contains_key(&hash) {
                    let used: u64 = s
                        .bundles
                        .values()
                        .filter(|b| b.uploaded_by == actor)
                        .map(|b| b.size)
                        .sum();
                    if used + size > s.quotas.max_bundle_bytes_per_account {
                        return Err("quota exceeded: bundle storage");
                    }
                }
                let _prev = s.bundles.entry(hash.clone()).or_insert(BundleRecord {
                    hash: hash.clone(),
                    size,
                    uploaded_by: actor,
                    package: None,
                });
                Ok(json!({ "hash": hash }))
            }
            SystemCmd::InstallPackage {
                package,
                signer_id,
                release,
            } => {
                let actor = release.installed_by.clone();
                let by_operator = actor == OPERATOR;
                if !by_operator && !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
                }
                // Installing a release again is an install too: it makes the
                // release the installer's to use.
                if !s.may_deploy(&actor, release.installed_at) {
                    return Err(NOT_A_DEPLOYER);
                }
                if let Some(p) = s.packages.get(&package) {
                    if p.signer_id != signer_id {
                        return Err("the package is published under another key");
                    }
                    if let Some(v) = p.versions.get(&release.version) {
                        // Other bytes under its version are a different,
                        // conflicting release. The same release again changes
                        // nothing but who installed it: it is theirs too.
                        if v.mpk != release.mpk {
                            return Err("that version is installed with other contents");
                        }
                        if !v.installed_by_any(&actor) {
                            if let Some(v) = s
                                .packages
                                .get_mut(&package)
                                .and_then(|p| p.versions.get_mut(&release.version))
                            {
                                let _new = v.installers.insert(actor);
                            }
                        }
                        return Ok(json!({ "package": package, "version": release.version }));
                    }
                }
                if s.packages
                    .get(&package)
                    .is_some_and(|p| !p.upgrades_in_order(&release.version, release.upgrades))
                {
                    return Err(
                        "a release may not give its package's upgrades back to the publisher once a lower one gave them to owners",
                    );
                }
                let used: u64 = s
                    .bundles
                    .values()
                    .filter(|b| b.uploaded_by == actor)
                    .map(|b| b.size)
                    .sum();
                let added = if release.part_sizes.is_empty() {
                    release.mpk_size
                } else {
                    release
                        .parts(&package)
                        .filter(|(hash, _)| !s.bundles.contains_key(*hash))
                        .map(|(hash, _)| release.part_size(hash))
                        .sum()
                };
                // The operator's installs count against no account's quota.
                if !by_operator && used + added > s.quotas.max_bundle_bytes_per_account {
                    return Err("quota exceeded: bundle storage");
                }
                for (hash, at) in release.parts(&package) {
                    let b = s.bundles.entry(hash.to_owned()).or_insert(BundleRecord {
                        hash: hash.to_owned(),
                        size: release.part_size(hash),
                        uploaded_by: actor.clone(),
                        package: None,
                    });
                    // A raw upload of the same WASM gains the release it ships in.
                    if b.package.is_none() {
                        b.package = Some(at);
                    }
                }
                let version = release.version.clone();
                let _prev = s
                    .packages
                    .entry(package.clone())
                    .or_insert_with(|| PackageRecord {
                        name: package.clone(),
                        signer_id,
                        owner: actor,
                        versions: BTreeMap::new(),
                    })
                    .versions
                    .insert(version.clone(), release);
                Ok(json!({ "package": package, "version": version }))
            }
            SystemCmd::UninstallPackage {
                package,
                version,
                actor,
            } => {
                let p = s.packages.get(&package).ok_or("no such package")?;
                if p.owner != actor {
                    return Err("only whoever installed the package (an account, or the operator) may uninstall it");
                }
                let gone: Vec<String> = match &version {
                    Some(v) if !p.versions.contains_key(v) => return Err("no such version"),
                    Some(v) => vec![v.clone()],
                    None => p.versions.keys().cloned().collect(),
                };
                // An offered release is the deployments' to keep: whoever
                // installed it first does not take it from them.
                if gone.iter().any(|v| s.offered(&package, v)) {
                    return Err("a deployment offers that release; it is uninstalled once no deployment offers it");
                }
                let hashes: BTreeSet<String> = gone
                    .iter()
                    .filter_map(|v| p.versions.get(v))
                    .flat_map(|v| v.parts(&package).map(|(h, _)| h.to_owned()))
                    .collect();
                // Two releases may ship the same WASM: a hash another release
                // still provides stays, recorded under that release.
                let provider = |s: &SystemState, h: &str| -> Option<PackageRef> {
                    s.packages.iter().find_map(|(name, p)| {
                        p.versions.iter().find_map(|(v, r)| {
                            let gone_here = *name == package && gone.contains(v);
                            (!gone_here)
                                .then(|| r.parts(name).find(|(x, _)| *x == h))
                                .flatten()
                                .map(|(_, at)| at)
                        })
                    })
                };
                let mut keep = BTreeMap::new();
                for h in &hashes {
                    if let Some(r) = provider(s, h) {
                        let _prev = keep.insert(h.clone(), r);
                    }
                }
                let in_use = |h: &String| hashes.contains(h) && !keep.contains_key(h);
                if s.apps
                    .values()
                    .any(|a| in_use(&a.bundle) || a.compute_bundle.as_ref().is_some_and(in_use))
                {
                    return Err("an app still runs that release; delete or upgrade it first");
                }
                for h in &hashes {
                    match keep.remove(h) {
                        Some(r) => {
                            if let Some(b) = s.bundles.get_mut(h) {
                                b.package = Some(r);
                            }
                        }
                        None => {
                            let _prev = s.bundles.remove(h);
                        }
                    }
                }
                let p = s.packages.get_mut(&package).ok_or("no such package")?;
                for v in &gone {
                    let _prev = p.versions.remove(v);
                }
                if p.versions.is_empty() {
                    let _prev = s.packages.remove(&package);
                    s.forget_origins(&package);
                }
                Ok(json!({ "removed": gone }))
            }
            SystemCmd::CreateApp { app } => {
                let mut app = *app;
                if !s.bundles.contains_key(&app.bundle) {
                    return Err("unknown bundle");
                }
                if app.components.is_app() && s.is_compute_bundle(&app.bundle) {
                    return Err(COMPUTE_NOT_AN_APP);
                }
                if !s.ships_compute(&app.bundle, app.compute_bundle.as_deref()) {
                    return Err(COMPUTE_NOT_SHIPPED);
                }
                if app
                    .release
                    .as_ref()
                    .is_some_and(|r| !s.release_ships(r, &app.bundle))
                {
                    return Err(RELEASE_NOT_SHIPPED);
                }
                if s.components_of(app.release.as_ref()) != app.components {
                    return Err(COMPONENTS_NOT_RELEASES);
                }
                let owned = s.apps.values().filter(|a| a.owner == app.owner).count();
                if owned >= s.quotas.max_apps_per_account as usize {
                    return Err("quota exceeded: apps");
                }
                if s.apps.contains_key(&app.app_id) {
                    return Err("app exists");
                }
                if let Some(id) = &app.deployment {
                    let d = s.deployments.get(id).ok_or("no such deployment")?;
                    let offers_release = app.release.as_ref().is_none_or(|r| {
                        d.apps
                            .values()
                            .any(|o| o.package == r.package && o.version == r.version)
                    });
                    if !s.offers_bundle(d, &app.bundle) || !offers_release {
                        return Err("the deployment does not offer the release this app would run");
                    }
                }
                if let Some(org) = &app.org {
                    let o = s.orgs.get(org);
                    let member = o.and_then(|o| o.role(&app.owner));
                    if !member.is_some_and(|r| r >= OrgRole::Member) {
                        return Err("owner is not a member of the organization");
                    }
                    // CREATE_APP: admins and the team's leads hold it,
                    // a member only when granted (not by default).
                    let team = match &app.access {
                        Access::Team { team } => Some(team.as_str()),
                        _ => None,
                    };
                    if !o.is_some_and(|o| o.may(&app.owner, team, crate::types::caps::CREATE_APP)) {
                        return Err("creating apps here needs the CREATE_APP capability");
                    }
                    // In an organization whose apps have app teams, a package
                    // with none is a new app there: an admin installs it.
                    if let (Some(o), Some(package)) = (o, s.package_of(&app)) {
                        if o.mode() != crate::types::OrgMode::Legacy
                            && o.app_team(package).is_none()
                            && !o.role(&app.owner).is_some_and(|r| r >= OrgRole::Admin)
                        {
                            return Err(ONLY_ADMINS_INSTALL);
                        }
                    }
                }
                if let Access::Team { team } = &app.access {
                    let o = app.org.as_ref().and_then(|o| s.orgs.get(o));
                    // A team's app is created by someone who writes the team.
                    if !o.is_some_and(|o| o.writes_team(&app.owner, team)) {
                        return Err("not a member of that team");
                    }
                }
                s.next_group = s.next_group.max(1) + 1;
                app.group = s.next_group;
                let out =
                    json!({ "appId": app.app_id, "group": app.group, "replicas": app.replicas });
                let _prev = s.apps.insert(app.app_id.clone(), app);
                Ok(out)
            }
            SystemCmd::UpgradeApp {
                app_id,
                bundle,
                compute_bundle,
                by_operator,
                actor,
                at,
                run,
                consent,
                release,
                owner_in_org,
                owner_in_team,
            } => {
                if !s.bundles.contains_key(&bundle) {
                    return Err("unknown bundle");
                }
                let components = s.components_of(release.as_ref());
                if components.is_app() && s.is_compute_bundle(&bundle) {
                    return Err(COMPUTE_NOT_AN_APP);
                }
                if !s.ships_compute(&bundle, compute_bundle.as_deref()) {
                    return Err(COMPUTE_NOT_SHIPPED);
                }
                if release
                    .as_ref()
                    .is_some_and(|r| !s.release_ships(r, &bundle))
                {
                    return Err(RELEASE_NOT_SHIPPED);
                }
                if s.apps
                    .get(&app_id)
                    .is_some_and(|a| a.components != components)
                {
                    return Err(COMPONENTS_CHANGED);
                }
                let operator = s.governance().app_upgrades == Authority::Operator;
                let from = s.apps.get(&app_id).map(|a| a.bundle.clone());
                if !by_operator && from.is_some_and(|f| !s.upgrade_keeps_authority(&f, &bundle)) {
                    return Err(
                        "an app whose owner upgrades it is not moved to a release its publisher upgrades",
                    );
                }
                if by_operator != operator {
                    return Err(if operator {
                        "this cluster's apps are upgraded by its operator"
                    } else {
                        "this cluster's apps are upgraded by their owners"
                    });
                }
                let allowed = if by_operator {
                    s.apps.contains_key(&app_id)
                } else {
                    s.apps.get(&app_id).is_some_and(|a| {
                        s.may_upgrade_as_logged(
                            a,
                            &actor,
                            OwnerRule::logged(owner_in_org, owner_in_team),
                        )
                    })
                };
                if !allowed {
                    return Err(
                        "upgrading this app needs its owner, or MANAGE_APPLICATION in its organization",
                    );
                }
                let by = if by_operator {
                    OPERATOR.to_owned()
                } else {
                    actor
                };
                if let Some(a) = s.apps.get_mut(&app_id) {
                    a.last_upgrade = Some(crate::types::UpgradeRecord {
                        from: a.bundle.clone(),
                        to: bundle.clone(),
                        at,
                        by,
                        ok: true,
                        error: Value::Null,
                        run: run.clone(),
                        api_key: crate::proof::key_proof_id(&consent)
                            .filter(|_| !by_operator)
                            .map(str::to_owned),
                        from_release: a.release.clone(),
                        to_release: release.clone(),
                    });
                    a.bundle = bundle;
                    a.compute_bundle = compute_bundle;
                    a.release = release;
                }
                s.note_run(run.as_deref(), &app_id, None);
                Ok(Value::Null)
            }
            SystemCmd::UpgradeFailed {
                app_id,
                bundle,
                by,
                at,
                error,
                run,
                release,
            } => {
                let a = s.apps.get_mut(&app_id).ok_or("no such app")?;
                a.last_upgrade = Some(crate::types::UpgradeRecord {
                    from_release: a.release.clone(),
                    to_release: release,
                    from: a.bundle.clone(),
                    to: bundle,
                    at,
                    by,
                    ok: false,
                    error: error.clone(),
                    run: run.clone(),
                    api_key: None,
                });
                s.note_run(run.as_deref(), &app_id, Some(error));
                Ok(Value::Null)
            }
            SystemCmd::StartUpgradeRun {
                org,
                run,
                consent: _,
            } => {
                let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
                if o.upgrade_runs.contains_key(&run.run_id) {
                    return Err("that run exists");
                }
                while o.upgrade_runs.len() >= crate::types::UPGRADE_RUNS_KEPT {
                    let oldest = o
                        .upgrade_runs
                        .values()
                        .min_by_key(|r| (r.started_at, r.run_id.clone()))
                        .map(|r| r.run_id.clone());
                    match oldest {
                        Some(id) => {
                            let _gone = o.upgrade_runs.remove(&id);
                        }
                        None => break,
                    }
                }
                let _prev = o.upgrade_runs.insert(run.run_id.clone(), *run);
                Ok(Value::Null)
            }
            SystemCmd::FinishUpgradeRun { org, run_id } => {
                let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
                let r = o.upgrade_runs.get_mut(&run_id).ok_or("no such run")?;
                r.finished = true;
                Ok(serde_json::to_value(&*r).unwrap_or_default())
            }
            SystemCmd::SetAlias {
                account,
                alias,
                app,
            } => {
                if alias.is_empty() || alias.chars().count() > MAX_LABEL_CHARS {
                    return Err("an alias is 1 to 64 characters");
                }
                let a = s.accounts.get_mut(&account).ok_or("unknown account")?;
                match app {
                    Some(app) => {
                        if a.aliases.len() >= MAX_ALIASES && !a.aliases.contains_key(&alias) {
                            return Err("too many aliases");
                        }
                        let _prev = a.aliases.insert(alias, app);
                    }
                    None => {
                        let _gone = a.aliases.remove(&alias);
                    }
                }
                Ok(Value::Null)
            }
            SystemCmd::TransferApp {
                app_id,
                to,
                actor,
                owner_in_org,
                owner_in_team,
                ..
            } => {
                if !s.accounts.contains_key(&to) {
                    return Err("unknown account");
                }
                let org = s.apps.get(&app_id).ok_or("no such app")?.org.clone();
                // An org's app stays with a member of the org (as `CreateApp`).
                if let Some(org) = org {
                    let role = s.orgs.get(&org).and_then(|o| o.role(&to));
                    if !role.is_some_and(|r| r >= OrgRole::Member) {
                        return Err("the new owner is not a member of the app's organization");
                    }
                }
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                if !owner_in_org && app.owner != actor {
                    return Err("only the owner may transfer");
                }
                if owner_in_org
                    && !s.manages_app_as_logged(app, &actor, OwnerRule::logged(true, owner_in_team))
                {
                    return Err(ONLY_MANAGERS_TRANSFER);
                }
                // A personal app's files move to the new owner's quota.
                let held = s.app_file_bytes.get(&app_id).copied().unwrap_or(0);
                let new_payer = format!("account:{to}");
                if app.org.is_none()
                    && s.file_bytes(&new_payer).saturating_add(held) > s.payer_quotas(&new_payer).2
                {
                    return Err("quota exceeded: file bytes");
                }
                s.set_app_file_bytes(&app_id, 0);
                if let Some(app) = s.apps.get_mut(&app_id) {
                    app.owner = to;
                }
                s.set_app_file_bytes(&app_id, held);
                let payer = s.apps.get(&app_id).map(crate::usage::payer);
                if let (Some(payer), Some(runs)) = (payer, s.compute.get_mut(&app_id)) {
                    for slot in runs.values_mut() {
                        slot.payer.clone_from(&payer);
                    }
                    s.grant_runs();
                }
                Ok(Value::Null)
            }
            SystemCmd::MoveApp {
                app_id,
                to_org,
                actor,
                bring_members,
                nonce,
                ..
            } => s.move_app(&app_id, &to_org, bring_members, &nonce, &actor),
            SystemCmd::DeleteApp {
                app_id,
                actor,
                owner_in_org,
                owner_in_team,
            } => {
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                let admin = app.org.as_ref().is_some_and(|o| s.is_org_admin(o, &actor));
                if !s.owner_acts_as_logged(
                    app,
                    &actor,
                    OwnerRule::logged(owner_in_org, owner_in_team),
                ) && !admin
                {
                    return Err("only the owner or an org admin may delete");
                }
                s.remove_app(&app_id);
                Ok(Value::Null)
            }
            SystemCmd::ChargeFileBytes { app_id, delta } => {
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                let held = s.app_file_bytes.get(&app_id).copied().unwrap_or(0);
                let now = if delta >= 0 {
                    let payer = crate::usage::payer(app);
                    if s.file_bytes(&payer).saturating_add(delta.unsigned_abs())
                        > s.payer_quotas(&payer).2
                    {
                        return Err("quota exceeded: file bytes");
                    }
                    held.saturating_add(delta.unsigned_abs())
                } else {
                    held.saturating_sub(delta.unsigned_abs())
                };
                s.set_app_file_bytes(&app_id, now);
                Ok(json!({ "fileBytes": now }))
            }
            SystemCmd::AcquireRun {
                app,
                run,
                payer,
                fuel,
                memory_mib,
            } => {
                let Some(record) = s.apps.get(&app) else {
                    return Ok(acquire_refused(RUN_NO_SUCH_APP, false));
                };
                if s.run_slot(&app, &run).is_none() {
                    let pool = s.pool_for(memory_mib);
                    let refusal = if payer != crate::usage::payer(record) {
                        Some((RUN_NOT_PAYER, true))
                    } else if fuel > calimero_app_host::MAX_COMPUTE_FUEL {
                        Some((RUN_FUEL_OVER_MAX, false))
                    } else if fuel > s.payer_quotas(&payer).1 {
                        Some((RUN_FUEL_OVER_QUOTA, false))
                    } else if pool.is_none() {
                        Some((RUN_NO_POOL, false))
                    } else if s.released_runs.get(&app).is_some_and(|r| r.contains(&run)) {
                        Some((RUN_RELEASED, false))
                    } else if s.compute.get(&app).map_or(0, BTreeMap::len)
                        >= calimero_app_host::MAX_ACTIVE_RUNS
                    {
                        Some((RUN_APP_QUEUE_FULL, true))
                    } else {
                        None
                    };
                    if let Some((code, retryable)) = refusal {
                        return Ok(acquire_refused(code, retryable));
                    }
                    let seq = s.next_run_seq;
                    s.next_run_seq += 1;
                    let _prev = s.compute.entry(app.clone()).or_default().insert(
                        run.clone(),
                        RunSlot {
                            payer,
                            fuel,
                            seq,
                            granted: false,
                            refused: false,
                            pool: pool.unwrap_or_default(),
                        },
                    );
                    s.grant_runs();
                }
                let slot = s.run_slot(&app, &run);
                if slot.is_some_and(|r| r.refused) {
                    return Ok(acquire_refused(RUN_FUEL_OVER_QUOTA, false));
                }
                let granted = slot.is_some_and(|r| r.granted);
                Ok(json!({
                    "state": if granted { "granted" } else { "queued" },
                    "code": null,
                    "retryable": false,
                }))
            }
            SystemCmd::ReleaseRun { app, run } => {
                if !s.apps.contains_key(&app) {
                    return Ok(json!({ "released": false }));
                }
                let released = s
                    .compute
                    .get_mut(&app)
                    .is_some_and(|r| r.remove(&run).is_some());
                let ring = s.released_runs.entry(app.clone()).or_default();
                if !ring.contains(&run) {
                    ring.push_back(run.clone());
                }
                while ring.len() > RELEASED_RUNS_KEPT {
                    let _old = ring.pop_front();
                }
                if released {
                    if s.compute.get(&app).is_some_and(BTreeMap::is_empty) {
                        let _gone = s.compute.remove(&app);
                    }
                    s.grant_runs();
                }
                Ok(json!({ "released": released }))
            }
            SystemCmd::SetReplicas {
                app_id, replicas, ..
            } => {
                let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
                app.replicas = replicas;
                Ok(Value::Null)
            }
            SystemCmd::SetAccess {
                app_id,
                access,
                actor,
                owner_in_org,
                owner_in_team,
            } => {
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                if !s.owner_acts_as_logged(
                    app,
                    &actor,
                    OwnerRule::logged(owner_in_org, owner_in_team),
                ) && !s.may_set_access(app, &access, &actor)
                {
                    return Err("only the app's owner, an organization admin, or whoever manages its team's apps (MANAGE_APPLICATION) may change its access");
                }
                if let Access::Team { team } = &access {
                    let in_org = app
                        .org
                        .as_ref()
                        .and_then(|o| s.orgs.get(o))
                        .is_some_and(|o| o.teams.contains_key(team));
                    if !in_org {
                        return Err("no such team in the app's organization");
                    }
                }
                let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
                app.access = access;
                Ok(Value::Null)
            }
            SystemCmd::CreateAccount {
                mut account,
                invite,
                mut refresh,
                at,
                ..
            } => {
                // The first account waits for genesis: the rules it joins
                // under are fixed before anyone relies on them.
                if s.governance.is_none() && s.accounts.is_empty() {
                    return Err("the cluster is starting");
                }
                if s.accounts.contains_key(&account.account_id) {
                    return Err("account exists");
                }
                // Judged before anything changes: a refusal leaves no trace.
                let admission = s.admits(invite.as_ref())?;
                if s.accounts.len() >= s.quotas.max_accounts as usize {
                    return Err("quota exceeded: accounts");
                }
                for c in &account.passkeys {
                    if s.credentials.contains_key(&c.id) {
                        return Err("credential already registered");
                    }
                }
                if refresh.as_ref().is_some_and(|g| s.refresh_known(&g.hash)) {
                    return Err("refresh token already issued");
                }
                for c in &account.passkeys {
                    let _prev = s
                        .credentials
                        .insert(c.id.clone(), account.account_id.clone());
                }
                // The sign-up adds its passkey and signs in with it, at the
                // time the entry carries (none: an earlier release's).
                account.passkey_times = BTreeMap::new();
                if at > 0 {
                    for c in &account.passkeys {
                        let mut times = crate::types::PasskeyTimes {
                            added_at: Some(at),
                            last_sign_in_at: None,
                        };
                        if refresh.is_some() {
                            times.signed_in(at);
                        }
                        let _prev = account.passkey_times.insert(c.id.clone(), times);
                    }
                    if let Some(g) = &mut refresh {
                        g.record.opened_at = at;
                    }
                }
                let signed_in = refresh.as_ref().and_then(|g| sign_in_of(&g.record));
                if let Some(RefreshGrant { hash, record }) = refresh {
                    let _prev = s.refresh.insert(hash, record);
                }
                let id = account.account_id.clone();
                let _prev = s.accounts.insert(id.clone(), account);
                if let Some(entry) = signed_in {
                    s.record_activity(&id, entry);
                }
                if let Some(r) = &invite {
                    let _new = s.used_proofs.insert(crate::proof::invite_redeem_used(
                        &r.invite_key,
                        &id,
                        r.now,
                    ));
                }
                let org = match (admission, invite) {
                    (Admission::OrgInvite, Some(r)) => {
                        // A new account is never a member yet: every use is
                        // spent, as before the free redemption existed.
                        Some(s.redeem_org_invite(&r.invite_key, &id, r.now, false)?)
                    }
                    (Admission::SignUpInvite, Some(r)) => {
                        s.spend_sign_up_invite(&r.invite_key);
                        None
                    }
                    _ => None,
                };
                Ok(match org {
                    Some(org) => json!({ "accountId": id, "org": org }),
                    None => json!({ "accountId": id }),
                })
            }
            SystemCmd::AddPasskey {
                account,
                credential,
                at,
                ..
            } => {
                if s.credentials.contains_key(&credential.id) {
                    return Err("credential already registered");
                }
                let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
                if acc.passkeys.len() >= 10 {
                    return Err("an account holds at most 10 passkeys");
                }
                if at > 0 {
                    let _prev = acc.passkey_times.insert(
                        credential.id.clone(),
                        crate::types::PasskeyTimes {
                            added_at: Some(at),
                            last_sign_in_at: None,
                        },
                    );
                }
                let _prev = s.credentials.insert(credential.id.clone(), account);
                acc.passkeys.push(credential);
                Ok(Value::Null)
            }
            SystemCmd::RemovePasskey {
                account,
                credential_id,
                now,
                ..
            } => {
                let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
                if acc.passkeys.len() <= 1 {
                    return Err("cannot remove the last passkey");
                }
                if !acc.passkeys.iter().any(|c| c.id == credential_id) {
                    return Err("no such passkey");
                }
                acc.passkeys.retain(|c| c.id != credential_id);
                let _gone = acc.passkey_times.remove(&credential_id);
                let _prev = s.credentials.remove(&credential_id);
                // A removed device keeps no session (Xil's review, issue 5).
                let sessions: BTreeSet<String> = s
                    .refresh
                    .values()
                    .filter(|r| {
                        r.account == account && r.passkey.as_deref() == Some(&credential_id)
                    })
                    .map(|r| r.session.clone())
                    .collect();
                for session in sessions {
                    s.revoke_session(&session, now);
                }
                Ok(Value::Null)
            }
            SystemCmd::RevokeAllSessions { actor, now } => {
                let n = s.revoke_account_sessions(&actor, now);
                Ok(json!({ "revoked": n }))
            }
            SystemCmd::RefreshReused { hash, now } => {
                let rec = s
                    .rotated_refresh
                    .get(&hash)
                    .cloned()
                    .ok_or("unknown refresh token")?;
                s.revoke_session(&rec.session, now);
                Ok(json!({ "revoked": rec.session }))
            }
            SystemCmd::SetSignCount {
                credential_id,
                count,
                ..
            } => {
                let account = s
                    .credentials
                    .get(&credential_id)
                    .cloned()
                    .ok_or("unknown credential")?;
                let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
                let cred = acc
                    .passkeys
                    .iter_mut()
                    .find(|c| c.id == credential_id)
                    .ok_or("unknown credential")?;
                // Counters only move forward, whatever order logins committed
                // in; a passkey that does not count (0) has nothing to record.
                if count <= cred.sign_count {
                    return Err("stale signature counter");
                }
                cred.sign_count = count;
                Ok(Value::Null)
            }
            SystemCmd::CreateApiKey {
                hash,
                record,
                owner_in_org,
                owner_in_team,
                ..
            } => {
                if !s.accounts.contains_key(&record.account) {
                    return Err("unknown account");
                }
                if let Some(scope) = &record.upgrades {
                    s.upgrade_key_fits_as_logged(
                        &record,
                        scope,
                        OwnerRule::logged(owner_in_org, owner_in_team),
                    )?;
                }
                let keys = s
                    .api_keys
                    .values()
                    .filter(|k| k.account == record.account)
                    .count();
                if keys >= s.quotas.max_api_keys_per_account as usize {
                    return Err("quota exceeded: API keys");
                }
                let id = record.key_id.clone();
                let _prev = s.api_keys.insert(hash, record);
                Ok(json!({ "keyId": id }))
            }
            SystemCmd::SetAccountRecoveryKey {
                account,
                recovery_key,
                ..
            } => {
                if !s.accounts.contains_key(&account) {
                    return Err("unknown account");
                }
                if s.recovery_keys.contains_key(&recovery_key) {
                    return Err("that recovery code is in use");
                }
                // One code per account: a new one replaces the old.
                s.recovery_keys.retain(|_, a| *a != account);
                let _prev = s.recovery_keys.insert(recovery_key, account);
                Ok(Value::Null)
            }
            SystemCmd::RemoveAccountRecoveryKey { account, .. } => {
                let before = s.recovery_keys.len();
                s.recovery_keys.retain(|_, a| *a != account);
                if s.recovery_keys.len() == before {
                    return Err("no recovery code is set");
                }
                Ok(Value::Null)
            }
            SystemCmd::StartAccountRecovery {
                recovery_key,
                account,
                credential,
                at,
                ..
            } => {
                if s.recovery_keys.get(&recovery_key) != Some(&account) {
                    return Err(UNKNOWN_RECOVERY_CODE);
                }
                if !s.accounts.contains_key(&account) {
                    return Err("unknown account");
                }
                // One at a time: a code holder cannot replace a pending
                // recovery with their own, nor stack a second.
                if s.account_recoveries.contains_key(&account) {
                    return Err(ACCOUNT_RECOVERY_PENDING);
                }
                if s.credentials.contains_key(&credential.id)
                    || s.account_recoveries
                        .values()
                        .any(|p| p.credential.id == credential.id)
                {
                    return Err("credential already registered");
                }
                // Single use, whether it finishes or is cancelled.
                let _spent = s.recovery_keys.remove(&recovery_key);
                let pending = crate::types::PendingAccountRecovery {
                    credential,
                    started_at: at,
                    ready_at: at.saturating_add(crate::types::RECOVERY_DELAY_SECS),
                };
                let out = pending.json(&account);
                let _prev = s.account_recoveries.insert(account, pending);
                Ok(out)
            }
            SystemCmd::CancelAccountRecovery {
                account,
                credential_id,
                ..
            } => {
                if s.account_recoveries
                    .get(&account)
                    .is_none_or(|p| p.credential.id != credential_id)
                {
                    return Err(NO_ACCOUNT_RECOVERY_PENDING);
                }
                let _gone = s.account_recoveries.remove(&account);
                Ok(Value::Null)
            }
            SystemCmd::FinishAccountRecovery {
                account,
                at,
                refresh,
                consent,
                revoke_api_keys,
            } => {
                let p = s
                    .account_recoveries
                    .get(&account)
                    .ok_or(NO_ACCOUNT_RECOVERY_PENDING)?;
                if at < p.ready_at {
                    return Err(RECOVERY_NOT_READY);
                }
                if s.credentials.contains_key(&p.credential.id) {
                    return Err("credential already registered");
                }
                if s.refresh_known(&refresh.hash) {
                    return Err("refresh token already issued");
                }
                let mut credential = p.credential.clone();
                // The finish's assertion moved the passkey's counter: the
                // next sign-in must count past it.
                if let Some(count) = serde_json::from_value::<crate::proof::Consent>(consent)
                    .ok()
                    .and_then(|c| {
                        calimero_identity::assertion_counter(&credential, &c.assertion).ok()
                    })
                {
                    credential.sign_count = credential.sign_count.max(count);
                }
                let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
                // Not held to the 10-passkey cap: a person who lost every
                // passkey of a full account must still get back in. Each
                // recovery spends a code a passkey approved, so this grows
                // by one per code at most.
                acc.passkeys.push(credential.clone());
                // Added and signed in with when the session opened (none: a
                // finish an earlier release logged).
                let opened = refresh.record.opened_at;
                if opened > 0 {
                    let mut times = crate::types::PasskeyTimes {
                        added_at: Some(opened),
                        last_sign_in_at: None,
                    };
                    times.signed_in(opened);
                    let _prev = acc.passkey_times.insert(credential.id.clone(), times);
                }
                let _prev = s.credentials.insert(credential.id, account.clone());
                let _done = s.account_recoveries.remove(&account);
                // Whoever holds the lost devices keeps no session, and no
                // machine credential either.
                let revoked = s.revoke_account_sessions(&account, at);
                let keys_before = s.api_keys.len();
                if revoke_api_keys {
                    s.api_keys.retain(|_, k| k.account != account);
                }
                let revoked_keys = keys_before - s.api_keys.len();
                if let Some(entry) = sign_in_of(&refresh.record) {
                    s.record_activity(&account, entry);
                }
                let _prev = s.refresh.insert(refresh.hash, refresh.record);
                Ok(json!({
                    "accountId": account,
                    "revoked": revoked,
                    "revokedApiKeys": revoked_keys,
                }))
            }
            SystemCmd::RevokeApiKey { key_id, actor, .. } => {
                let before = s.api_keys.len();
                s.api_keys
                    .retain(|_, r| !(r.key_id == key_id && r.account == actor));
                if s.api_keys.len() == before {
                    return Err("no such key");
                }
                Ok(Value::Null)
            }
            SystemCmd::CreateRefresh { hash, record, .. } => {
                if s.refresh_known(&hash) {
                    return Err("refresh token already issued");
                }
                // A sign-in: its passkey's last sign-in is when the session
                // opened (unknown in an earlier release's entry).
                let mut signed_in = false;
                if let (Some(passkey), Some(acc), true) = (
                    &record.passkey,
                    s.accounts.get_mut(&record.account),
                    record.opened_at > 0,
                ) {
                    if acc.passkeys.iter().any(|c| &c.id == passkey) {
                        acc.passkey_times
                            .entry(passkey.clone())
                            .or_default()
                            .signed_in(record.opened_at);
                        signed_in = true;
                    }
                }
                if let Some(entry) = sign_in_of(&record).filter(|_| signed_in) {
                    s.record_activity(&record.account, entry);
                }
                let _prev = s.refresh.insert(hash, record);
                Ok(Value::Null)
            }
            SystemCmd::UseRefresh {
                old_hash,
                new_hash,
                mut record,
                now,
                ..
            } => {
                if s.refresh_known(&new_hash) {
                    return Err("refresh token already issued");
                }
                let old = s.refresh.remove(&old_hash).ok_or("unknown refresh token")?;
                if old.expires_at <= now || old.account != record.account {
                    return Err("refresh token expired");
                }
                if s.revoked_sessions.contains(&old.session) {
                    return Err("session revoked");
                }
                // The session opened when its first token did, whatever the
                // exchange states (an earlier release's states none).
                record.opened_at = old.opened_at;
                let _prev = s.rotated_refresh.insert(old_hash, old);
                let _prev = s.refresh.insert(new_hash, record);
                s.prune_sessions(now);
                Ok(Value::Null)
            }
            SystemCmd::RevokeSession {
                session,
                actor,
                now,
            } => {
                // Only the session's own account revokes it.
                let theirs = !s
                    .refresh
                    .values()
                    .any(|r| r.session == session && r.account != actor);
                if theirs {
                    s.revoke_session(&session, now);
                }
                s.prune_sessions(now);
                Ok(Value::Null)
            }
            SystemCmd::SetQuotas { quotas, .. } => {
                s.quotas = quotas;
                s.grant_runs();
                Ok(Value::Null)
            }
            SystemCmd::SetOrgQuotas { payer, quotas, .. } => {
                if payer.is_empty() || payer.len() > MAX_PAYER_BYTES {
                    return Err("a payer is named, in at most 128 bytes");
                }
                if !s.org_quotas.contains_key(&payer) && s.org_quotas.len() >= MAX_ORG_QUOTAS {
                    return Err("quota exceeded: org quota overrides");
                }
                if quotas == OrgQuotas::default() {
                    let _gone = s.org_quotas.remove(&payer);
                } else {
                    let _prev = s.org_quotas.insert(payer, quotas);
                }
                s.grant_runs();
                Ok(Value::Null)
            }
            SystemCmd::SetPools { pools, .. } => {
                if pools.len() > MAX_POOLS
                    || pools
                        .keys()
                        .any(|n| n.is_empty() || n.len() > MAX_POOL_NAME_BYTES)
                {
                    return Err(BAD_POOLS);
                }
                let in_use = s
                    .pools
                    .keys()
                    .filter(|name| !pools.contains_key(*name))
                    .find(|name| {
                        s.job_nodes()
                            .any(|n| SystemState::node_pool(n) == name.as_str())
                            || s.compute
                                .values()
                                .flat_map(BTreeMap::values)
                                .any(|r| s.slot_pool(r) == name.as_str())
                    });
                if in_use.is_some() {
                    return Err(POOL_IN_USE);
                }
                s.pools = pools;
                s.grant_runs();
                Ok(Value::Null)
            }
            SystemCmd::CreateOrg {
                org_id,
                name,
                actor,
                release,
                recovery_key,
                at,
            } => {
                if !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
                }
                let recovery = recovery_key.map(|k| recovery_key_of(k, at)).transpose()?;
                if s.orgs.contains_key(&org_id) {
                    return Err("organization exists");
                }
                let owned = s
                    .orgs
                    .values()
                    .filter(|o| o.members.get(&actor) == Some(&OrgRole::Owner))
                    .count();
                if owned >= s.quotas.max_orgs_per_account as usize {
                    return Err("quota exceeded: organizations");
                }
                let mut members = BTreeMap::new();
                let _prev = members.insert(actor, OrgRole::Owner);
                let _prev = s.orgs.insert(
                    org_id.clone(),
                    Org {
                        org_id: org_id.clone(),
                        name,
                        members,
                        teams: BTreeMap::new(),
                        default_capabilities: crate::types::caps::DEFAULT,
                        capabilities: BTreeMap::new(),
                        approved_releases: release.into_iter().collect(),
                        outbound_approvals: BTreeMap::new(),
                        metadata: Value::Null,
                        member_metadata: BTreeMap::new(),
                        upgrade_runs: BTreeMap::new(),
                        recovery,
                    },
                );
                Ok(json!({ "orgId": org_id }))
            }
            SystemCmd::Genesis {
                governance,
                release,
                operator_key,
                nodes,
                session_proofs,
            } => {
                if s.governance.is_some() || !s.accounts.is_empty() {
                    return Err("the cluster's governance is fixed at genesis");
                }
                if let Some(k) = &operator_key {
                    hex32(&k.key).ok_or("an operator key is 32 bytes, hex")?;
                }
                s.operator_key = operator_key;
                if governance.releases == Authority::Operator {
                    s.operator_releases = release.into_iter().collect();
                }
                s.governance = Some(governance.clone());
                for node in nodes {
                    let _prev = s.nodes.insert(node.id, node);
                }
                if session_proofs {
                    s.session_proofs_from = Some(index);
                }
                Ok(json!({ "governance": governance }))
            }
            SystemCmd::RequireSessionProofs => {
                if s.session_proofs_from.is_some() {
                    return Err("session proofs are required already");
                }
                s.session_proofs_from = Some(index);
                Ok(Value::Null)
            }
            SystemCmd::SetOperatorRelease {
                version,
                approved,
                proof: _,
            } => {
                if s.governance().releases != Authority::Operator {
                    return Err("this cluster's releases are approved by each app's owner");
                }
                let set = &mut s.operator_releases;
                if approved {
                    let _new = set.insert(version);
                } else {
                    if set.len() == 1 && set.contains(&version) {
                        return Err("the last approved release cannot be withdrawn");
                    }
                    let _gone = set.remove(&version);
                }
                Ok(json!({ "approvedReleases": set }))
            }
            SystemCmd::BindPublisher {
                signer_id,
                org,
                cluster,
                proof,
                actor,
                consent: _,
            } => {
                if signer_id.len() > MAX_BINDING_FIELD_BYTES
                    || cluster.len() > MAX_BINDING_FIELD_BYTES
                {
                    return Err("a signer id or cluster name is at most 128 bytes");
                }
                if !s.holds_approve_releases(&org, &actor) {
                    return Err(
                        "binding a publisher key needs APPROVE_RELEASES in the organization",
                    );
                }
                let generation = s.binding_generation(&signer_id);
                let message = publisher_binding_message(&cluster, &org, &signer_id, generation);
                if !proof.proves(&signer_id, &message) {
                    return Err("the proof is not that key's signature over this binding");
                }
                let _prev = s.publishers.insert(
                    signer_id,
                    PublisherBinding {
                        org,
                        generation: generation + 1,
                    },
                );
                Ok(json!({ "generation": generation + 1 }))
            }
            SystemCmd::SetReleaseDeadline { version, after, .. } => {
                // Closed input: a semver release, so it orders against the
                // approved ones; a bounded number of them.
                if version.len() > MAX_RELEASE_VERSION_BYTES
                    || semver::Version::parse(&version).is_err()
                {
                    return Err("a deadline names a semver release of at most 64 bytes");
                }
                match after {
                    Some(t) => {
                        if !s.release_deadlines.contains_key(&version)
                            && s.release_deadlines.len() >= MAX_RELEASE_DEADLINES
                        {
                            return Err("too many release deadlines; lift an old one first");
                        }
                        let _prev = s.release_deadlines.insert(version, t);
                    }
                    None => {
                        let _gone = s.release_deadlines.remove(&version);
                    }
                }
                Ok(json!({ "deadlines": s.deadlines_json() }))
            }
            SystemCmd::SetReleaseApproval {
                scope,
                version,
                approved,
                actor,
                consent: _,
            } => {
                if s.governance().releases == Authority::Operator {
                    return Err("this cluster's releases are approved by its operator");
                }
                let set = match &scope {
                    ReleaseScope::Cluster => return Err("the operator uses SetOperatorRelease"),
                    ReleaseScope::Org { org } => {
                        if !s.approves_releases(org, &actor) {
                            return Err(
                                "approving releases needs APPROVE_RELEASES in the organization",
                            );
                        }
                        let o = s.orgs.get_mut(org).ok_or("no such organization")?;
                        &mut o.approved_releases
                    }
                    ReleaseScope::Account => {
                        &mut s
                            .accounts
                            .get_mut(&actor)
                            .ok_or("unknown account")?
                            .approved_releases
                    }
                };
                if approved {
                    let _new = set.insert(version);
                } else {
                    // An empty set would leave the apps unrestricted.
                    if set.len() == 1 && set.contains(&version) {
                        return Err("the last approved release cannot be withdrawn");
                    }
                    let _gone = set.remove(&version);
                }
                Ok(json!({ "approvedReleases": set }))
            }
            SystemCmd::SetOutboundApproval {
                scope,
                package,
                selection,
                replace,
                actor,
                consent: _,
            } => {
                let record = s.packages.get(&package).ok_or("no such package")?;
                let signer = record.signer_id.clone();
                let declared = record
                    .latest()
                    .map(|v| v.outbound.clone())
                    .unwrap_or_default();
                let operator = s.governance().outbound == Authority::Operator;
                if operator != (scope == ReleaseScope::Cluster) {
                    return Err(if operator {
                        "this cluster's outbound hosts are approved by its operator"
                    } else {
                        "this cluster's outbound hosts are approved by each app's owner"
                    });
                }
                let approvals = match &scope {
                    ReleaseScope::Cluster => &mut s.operator_outbound,
                    ReleaseScope::Org { org } => {
                        // One approval covers every app of the organization,
                        // so only an org-wide grant reaches it.
                        if !s.manages_apps(org, &actor, None) {
                            return Err(
                                "approving outbound hosts needs MANAGE_APPLICATION in the organization",
                            );
                        }
                        let o = s.orgs.get_mut(org).ok_or("no such organization")?;
                        &mut o.outbound_approvals
                    }
                    ReleaseScope::Account => {
                        &mut s
                            .accounts
                            .get_mut(&actor)
                            .ok_or("unknown account")?
                            .outbound_approvals
                    }
                };
                let existing = approvals.get(&signer).map(|a| calimero_outbound::Approval {
                    prefixes: a.prefixes.clone(),
                    reviewed: a.reviewed.clone(),
                });
                let merged = calimero_outbound::merge_approval(
                    existing.as_ref(),
                    &declared,
                    &selection,
                    replace,
                )
                .map_err(|_| "an approved prefix is not an https URL prefix, or too many")?;
                let approval = crate::types::OutboundApproval {
                    prefixes: merged.prefixes,
                    reviewed: merged.reviewed,
                };
                let _prev = approvals.insert(signer.clone(), approval.clone());
                Ok(json!({ "signer": signer, "approval": approval }))
            }
            SystemCmd::GrantDeployer {
                deployer,
                until,
                at,
                actor,
                ..
            } => {
                s.grants_deployers(&actor)?;
                if !s.exists(&deployer) {
                    return Err("no such account or organization");
                }
                if until.is_some_and(|u| u <= at) {
                    return Err("a deployer grant ends after it is made");
                }
                let key = deployer.key();
                if !s.deployers.contains_key(&key) && s.deployers.len() >= MAX_DEPLOYERS {
                    return Err("too many deployer grants; revoke an old one first");
                }
                let grant = DeployerGrant {
                    until,
                    granted_at: at,
                    granted_by: actor,
                };
                let _prev = s.deployers.insert(key.clone(), grant.clone());
                Ok(json!({ "key": key, "deployer": deployer, "grant": grant }))
            }
            SystemCmd::RevokeDeployer {
                deployer, actor, ..
            } => {
                s.grants_deployers(&actor)?;
                let key = deployer.key();
                let _gone = s.deployers.remove(&key).ok_or("no such deployer grant")?;
                Ok(json!({ "key": key }))
            }
            SystemCmd::SetDeploymentsGranted { proof: _ } => {
                let g = s
                    .governance
                    .as_mut()
                    .ok_or("the cluster has no governance yet")?;
                if g.deployments == Deployments::Granted {
                    return Err("this cluster's deployments are granted already");
                }
                g.deployments = Deployments::Granted;
                Ok(json!({ "governance": g }))
            }
            SystemCmd::SetSignUp { policy, actor, .. } => {
                s.runs_sign_up(&actor)?;
                let g = s
                    .governance
                    .as_mut()
                    .ok_or("the cluster has no governance yet")?;
                g.sign_up = policy;
                Ok(json!({ "governance": g }))
            }
            SystemCmd::CreateSignUpInvite {
                invite_key,
                invite,
                actor,
                ..
            } => {
                s.runs_sign_up(&actor)?;
                if invite.created_by != actor {
                    return Err("a sign-up invite names who issued it");
                }
                hex32(&invite_key).ok_or("an invite key is 32 bytes, hex")?;
                if s.invites.contains_key(&invite_key)
                    || s.sign_up_invites.contains_key(&invite_key)
                {
                    return Err("invite exists");
                }
                if s.sign_up_invites.len() >= MAX_SIGN_UP_INVITES {
                    return Err("quota exceeded: sign-up invites");
                }
                if invite.uses_left == 0 || invite.uses_left > MAX_SIGN_UP_INVITE_USES {
                    return Err("a sign-up invite creates 1 to 10000 accounts");
                }
                if invite.expires_at <= invite.created_at {
                    return Err("a sign-up invite expires after it is made");
                }
                let _prev = s.sign_up_invites.insert(invite_key.clone(), invite);
                Ok(json!({ "inviteKey": invite_key }))
            }
            SystemCmd::RevokeSignUpInvite {
                invite_key, actor, ..
            } => {
                s.runs_sign_up(&actor)?;
                let _gone = s
                    .sign_up_invites
                    .remove(&invite_key)
                    .ok_or("no such sign-up invite")?;
                Ok(json!({ "inviteKey": invite_key }))
            }
            SystemCmd::SetClusterOwner { owner, proof: _ } => {
                if s.governance().deployers != Deployers::Owner {
                    return Err(
                        "this cluster's operator grants its deployers; it has no owner to name",
                    );
                }
                if s.cluster_owner.is_some() {
                    return Err("this cluster's owner is named already");
                }
                if !s.exists(&owner) {
                    return Err("no such account or organization");
                }
                s.cluster_owner = Some(owner.clone());
                Ok(json!({ "owner": owner }))
            }
            SystemCmd::CreateDeployment {
                id,
                name,
                owner,
                actor,
                at,
                consent: _,
            } => {
                if !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
                }
                deployment_segment_fits(&id)?;
                deployment_name_fits(&name)?;
                if !s.exists(&owner) {
                    return Err("no such account or organization");
                }
                if !s.acts_for(&owner, &actor) {
                    return Err("a deployment is created by its owner account, or by MANAGE_APPLICATION in its organization");
                }
                if !s.may_deploy(&actor, at) {
                    return Err(NOT_A_DEPLOYER);
                }
                if s.deployments.contains_key(&id) {
                    return Err("that deployment exists");
                }
                if s.deployments.values().filter(|d| d.owner == owner).count()
                    >= MAX_DEPLOYMENTS_PER_OWNER
                {
                    return Err("quota exceeded: deployments per owner");
                }
                if s.deployments.len() >= MAX_DEPLOYMENTS {
                    return Err("quota exceeded: deployments");
                }
                let d = Deployment {
                    id: id.clone(),
                    name,
                    owner,
                    apps: BTreeMap::new(),
                    created_by: actor,
                    created_at: at,
                };
                let out = json!({ "id": id, "deployment": d });
                let _prev = s.deployments.insert(id, d);
                Ok(out)
            }
            SystemCmd::OfferApp {
                deployment,
                app,
                package,
                version,
                actor,
                at,
                consent: _,
            } => {
                let d = s.deployments.get(&deployment).ok_or("no such deployment")?;
                s.manages_deployment(d, &actor)?;
                if !s.may_deploy(&actor, at) {
                    return Err(NOT_A_DEPLOYER);
                }
                deployment_segment_fits(&app)?;
                // A release the manager may not see is answered as missing.
                let seen = s
                    .packages
                    .get(&package)
                    .and_then(|p| p.versions.get(&version))
                    .is_some_and(|v| s.sees_release(v, &actor));
                if !seen {
                    return Err("install the release before offering it");
                }
                match d.apps.get(&app) {
                    Some(o) if o.package != package => {
                        return Err("an app moves to another release of its own package; withdraw it to offer another package");
                    }
                    Some(_) => {}
                    None if d.apps.len() >= MAX_APPS_PER_DEPLOYMENT => {
                        return Err("quota exceeded: apps per deployment");
                    }
                    None => {}
                }
                let offer = Offer {
                    package,
                    version,
                    added_by: actor,
                    added_at: at,
                };
                let out = json!({ "deployment": deployment, "app": app, "offer": offer });
                if let Some(d) = s.deployments.get_mut(&deployment) {
                    let _prev = d.apps.insert(app, offer);
                }
                Ok(out)
            }
            SystemCmd::AddPackageOrigin {
                package,
                origin,
                rp_id,
                served,
                actor,
                at,
                consent: _,
            } => s.add_package_origin(package, origin, &rp_id, served, actor, at),
            SystemCmd::RemovePackageOrigin {
                package,
                origin,
                actor,
                consent: _,
            } => s.remove_package_origin(&package, &origin, &actor),
            SystemCmd::WithdrawApp {
                deployment,
                app,
                actor,
                consent: _,
            } => {
                let d = s.deployments.get(&deployment).ok_or("no such deployment")?;
                s.manages_deployment(d, &actor)?;
                let offer = d
                    .apps
                    .get(&app)
                    .ok_or("the deployment offers no such app")?;
                let running = s.apps.values().any(|a| {
                    a.deployment.as_deref() == Some(deployment.as_str())
                        && s.ships(&offer.package, &a.bundle)
                });
                if running {
                    return Err("an instance created from this deployment still runs that app; withdraw it once none does");
                }
                if let Some(d) = s.deployments.get_mut(&deployment) {
                    let _gone = d.apps.remove(&app);
                }
                Ok(json!({ "deployment": deployment, "app": app }))
            }
            SystemCmd::DeleteDeployment {
                deployment,
                actor,
                consent: _,
            } => {
                let d = s.deployments.get(&deployment).ok_or("no such deployment")?;
                s.manages_deployment(d, &actor)?;
                if !d.apps.is_empty() {
                    return Err("the deployment still offers apps; withdraw them first");
                }
                if s.apps
                    .values()
                    .any(|a| a.deployment.as_deref() == Some(deployment.as_str()))
                {
                    return Err("an instance created from this deployment still exists");
                }
                let _gone = s.deployments.remove(&deployment);
                Ok(json!({ "deployment": deployment }))
            }
            SystemCmd::DeleteOrg {
                org,
                actor,
                consent: _,
            } => {
                let o = s.orgs.get(&org).ok_or("no such organization")?;
                if o.role(&actor) != Some(OrgRole::Owner) {
                    return Err("only the organization's owner deletes it");
                }
                let owner = Deployer::Org(org.clone());
                if s.deployments.values().any(|d| d.owner == owner) {
                    return Err("the organization owns a deployment; delete it first");
                }
                let _gone = s.orgs.remove(&org);
                s.invites.retain(|_, i| i.org != org);
                let apps: Vec<String> = s
                    .apps
                    .values()
                    .filter(|a| a.org.as_deref() == Some(org.as_str()))
                    .map(|a| a.app_id.clone())
                    .collect();
                for a in &apps {
                    s.remove_app(a);
                }
                Ok(json!({ "apps": apps }))
            }
            SystemCmd::SetOrgMetadata {
                org,
                metadata,
                actor,
            } => {
                metadata_fits(&metadata)?;
                let o = self.org_mut(&org)?;
                if o.role(&actor) < Some(OrgRole::Admin)
                    && !o.may(&actor, None, crate::types::caps::MANAGE_METADATA)
                {
                    return Err("not allowed to change this organization's metadata");
                }
                o.metadata = metadata;
                Ok(Value::Null)
            }
            SystemCmd::SetMemberMetadata {
                org,
                account,
                metadata,
                actor,
            } => {
                metadata_fits(&metadata)?;
                let o = self.org_mut(&org)?;
                if o.role(&account).is_none() {
                    return Err("not a member");
                }
                if actor != account && o.role(&actor) < Some(OrgRole::Admin) {
                    return Err("a member's metadata is theirs or an admin's to set");
                }
                if metadata.is_null() {
                    let _gone = o.member_metadata.remove(&account);
                } else {
                    let _prev = o.member_metadata.insert(account, metadata);
                }
                Ok(Value::Null)
            }
            SystemCmd::SetAppMetadata {
                app_id,
                metadata,
                actor,
                owner_in_org,
                owner_in_team,
            } => {
                metadata_fits(&metadata)?;
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                let admin = app.org.as_ref().is_some_and(|o| s.is_org_admin(o, &actor));
                if !s.owner_acts_as_logged(
                    app,
                    &actor,
                    OwnerRule::logged(owner_in_org, owner_in_team),
                ) && !admin
                {
                    return Err("only the owner or an org admin sets an app's metadata");
                }
                if let Some(a) = s.apps.get_mut(&app_id) {
                    a.metadata = metadata;
                }
                Ok(Value::Null)
            }
            SystemCmd::SetPasskeyLabel {
                account,
                credential,
                label,
            } => {
                if label.chars().count() > MAX_LABEL_CHARS {
                    return Err("a passkey label is at most 64 characters");
                }
                let a = s.accounts.get_mut(&account).ok_or("unknown account")?;
                if !a.passkeys.iter().any(|c| c.id == credential) {
                    return Err("no such passkey");
                }
                if label.is_empty() {
                    let _gone = a.passkey_labels.remove(&credential);
                } else {
                    let _prev = a.passkey_labels.insert(credential, label);
                }
                Ok(Value::Null)
            }
            SystemCmd::SetOrgRole {
                org,
                account,
                role,
                actor,
                ..
            } => {
                let known = s.accounts.contains_key(&account);
                let o = self.org_mut(&org)?;
                let actor_role = o.role(&actor).ok_or("not a member")?;
                let current = o.role(&account);
                if actor_role < OrgRole::Admin {
                    return Err("only admins manage members");
                }
                if (role == OrgRole::Owner || current == Some(OrgRole::Owner))
                    && actor_role != OrgRole::Owner
                {
                    return Err("only the owner changes ownership");
                }
                if !known {
                    return Err("unknown account");
                }
                if role == OrgRole::Owner {
                    // One owner: the previous one becomes an admin.
                    for r in o.members.values_mut() {
                        if *r == OrgRole::Owner {
                            *r = OrgRole::Admin;
                        }
                    }
                }
                let _prev = o.members.insert(account, role);
                Ok(Value::Null)
            }
            SystemCmd::RemoveOrgMember {
                org,
                account,
                actor,
                ..
            } => {
                let o = self.org_mut(&org)?;
                let actor_role = o.role(&actor).ok_or("not a member")?;
                let target = o.role(&account).ok_or("not a member")?;
                let leaving = actor == account;
                if target == OrgRole::Owner {
                    return Err("transfer ownership before removing the owner");
                }
                if !leaving && actor_role < OrgRole::Admin {
                    return Err("only admins remove members");
                }
                let _prev = o.members.remove(&account);
                let _caps = o.capabilities.remove(&account);
                let _meta = o.member_metadata.remove(&account);
                for t in o.teams.values_mut() {
                    let _prev = t.members.remove(&account);
                    let _was = t.read_only.remove(&account);
                    let _caps = t.capabilities.remove(&account);
                }
                Ok(Value::Null)
            }
            SystemCmd::CreateTeam {
                org,
                team_id,
                name,
                parent,
                join,
                actor,
                member_child_teams,
            } => {
                let o = self.org_mut(&org)?;
                if o.teams.contains_key(&team_id) {
                    return Err("team exists");
                }
                // An admin anywhere, a team's lead below it; a member with
                // CREATE_TEAM at the top; a member who writes a team and holds
                // CREATE_CHILD_TEAM there (an app team's default), a
                // restricted team below it (a private channel), which they
                // lead.
                let member_creates = |p: &str| {
                    member_child_teams
                        && join == TeamJoin::Invite
                        && o.writes_team(&actor, p)
                        && o.may(&actor, Some(p), crate::types::caps::CREATE_CHILD_TEAM)
                };
                let allowed = match &parent {
                    Some(p) => {
                        o.teams.contains_key(p) && (o.manages_team(&actor, p) || member_creates(p))
                    }
                    None => {
                        o.role(&actor).is_some_and(|r| r >= OrgRole::Member)
                            && o.may(&actor, None, crate::types::caps::CREATE_TEAM)
                    }
                };
                if !allowed {
                    return Err("not allowed to create this team");
                }
                // Its creator leads it.
                let mut members = BTreeMap::new();
                let _prev = members.insert(actor.clone(), true);
                let _prev = o.teams.insert(
                    team_id.clone(),
                    Team {
                        team_id: team_id.clone(),
                        name,
                        parent,
                        join,
                        members,
                        read_only: Default::default(),
                        capabilities: Default::default(),
                        metadata: Value::Null,
                        removed: Default::default(),
                        default_capabilities: None,
                    },
                );
                Ok(json!({ "teamId": team_id }))
            }
            SystemCmd::SetTeamMember {
                org,
                team,
                account,
                lead,
                role,
                capabilities,
                actor,
                ..
            } => {
                let o = self.org_mut(&org)?;
                if !o.teams.contains_key(&team) {
                    return Err("no such team");
                }
                let role = role.unwrap_or(if lead {
                    TeamRole::Lead
                } else {
                    TeamRole::Member
                });
                // Joining an open team one reaches anyway writes one's own row.
                let joins_self = actor == account
                    && role == TeamRole::Member
                    && o.teams
                        .get(&team)
                        .is_some_and(|t| t.join == TeamJoin::Everyone)
                    && o.in_team(&actor, &team);
                let manages = o.manages_team(&actor, &team)
                    || (role != TeamRole::Lead
                        && o.may(&actor, Some(&team), crate::types::caps::MANAGE_MEMBERS));
                if !joins_self && !manages {
                    return Err("not allowed to manage this team");
                }
                if o.role(&account).is_none() {
                    return Err("only organization members join teams");
                }
                // A new row takes what the member holds organization-wide, and
                // the team's default on top.
                let default = o
                    .capabilities
                    .get(&account)
                    .copied()
                    .unwrap_or(o.default_capabilities)
                    | o.teams
                        .get(&team)
                        .and_then(|t| t.default_capabilities)
                        .unwrap_or(0);
                if let Some(t) = o.teams.get_mut(&team) {
                    let _was = t.removed.remove(&account);
                    let _prev = t.members.insert(account.clone(), role == TeamRole::Lead);
                    if role == TeamRole::ReadOnly {
                        let _new = t.read_only.insert(account.clone());
                    } else {
                        let _was = t.read_only.remove(&account);
                    }
                    match capabilities {
                        Some(c) => {
                            let _prev = t.capabilities.insert(account, c);
                        }
                        None => {
                            let _kept = t.capabilities.entry(account).or_insert(default);
                        }
                    }
                }
                Ok(Value::Null)
            }
            SystemCmd::UpdateTeam {
                org,
                team,
                name,
                join,
                parent,
                metadata,
                actor,
            } => {
                let o = self.org_mut(&org)?;
                if !o.teams.contains_key(&team) {
                    return Err("no such team");
                }
                use crate::types::caps;
                if name.is_some() && !o.manages_team(&actor, &team) {
                    return Err("not allowed to rename this team");
                }
                if join.is_some() && !o.may(&actor, Some(&team), caps::MANAGE_VISIBILITY) {
                    return Err("not allowed to change this team's visibility");
                }
                if let Some(m) = &metadata {
                    metadata_fits(m)?;
                }
                if metadata.is_some() && !o.may(&actor, Some(&team), caps::MANAGE_METADATA) {
                    return Err("not allowed to change this team's metadata");
                }
                // An app team (`metadata.app.package` at the top) installs an
                // app in the organization: only an admin makes, changes or
                // drops one, and a package has one app team here (G9).
                let current = o.teams.get(&team).ok_or("no such team")?;
                let parent_after = parent.clone().unwrap_or_else(|| current.parent.clone());
                let package_before = current.app_package().map(str::to_owned);
                let package_after = if parent_after.is_some() {
                    None
                } else {
                    metadata
                        .as_ref()
                        .unwrap_or(&current.metadata)
                        .get("app")
                        .and_then(|a| a.get("package"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                if package_before != package_after {
                    if !o.role(&actor).is_some_and(|r| r >= OrgRole::Admin) {
                        return Err(ONLY_ADMINS_INSTALL);
                    }
                    if let Some(p) = &package_after {
                        if o.app_team(p).is_some_and(|t| t.team_id != team) {
                            return Err(APP_TEAM_EXISTS);
                        }
                    }
                }
                if let Some(to) = &parent {
                    let may_move = o.manages_team(&actor, &team)
                        && match to {
                            Some(p) => o.teams.contains_key(p) && o.manages_team(&actor, p),
                            None => {
                                o.role(&actor).is_some_and(|r| r >= OrgRole::Admin)
                                    || o.may(&actor, None, caps::CREATE_TEAM)
                            }
                        };
                    if !may_move {
                        return Err("not allowed to move this team there");
                    }
                    if let Some(p) = to {
                        if o.subtree(&team).contains(p) {
                            return Err("a team cannot move under itself");
                        }
                    }
                }
                let t = o.teams.get_mut(&team).ok_or("no such team")?;
                if let Some(n) = name {
                    t.name = n;
                }
                if let Some(j) = join {
                    t.join = j;
                }
                if let Some(p) = parent {
                    t.parent = p;
                }
                if let Some(m) = metadata {
                    t.metadata = m;
                }
                Ok(Value::Null)
            }
            SystemCmd::DeleteTeam {
                org, team, actor, ..
            } => {
                let apps = s.team_apps(&org, &team);
                let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
                if !o.teams.contains_key(&team) {
                    return Err("no such team");
                }
                if !o.manages_team(&actor, &team)
                    && !o.may(&actor, None, crate::types::caps::DELETE_TEAM)
                {
                    return Err("not allowed to delete this team");
                }
                let gone = o.subtree(&team);
                for t in &gone {
                    let _removed = o.teams.remove(t);
                }
                for a in &apps {
                    s.remove_app(a);
                }
                Ok(json!({ "teams": gone, "apps": apps }))
            }
            SystemCmd::SetCapabilities {
                org,
                team,
                account,
                capabilities,
                actor,
                ..
            } => {
                let o = self.org_mut(&org)?;
                if o.role(&account).is_none() {
                    return Err("not a member");
                }
                match team {
                    Some(t) => {
                        if !o.manages_team(&actor, &t)
                            && !o.may(&actor, Some(&t), crate::types::caps::MANAGE_MEMBERS)
                        {
                            return Err("not allowed to manage this team");
                        }
                        let rec = o.teams.get_mut(&t).ok_or("no such team")?;
                        if !rec.members.contains_key(&account) {
                            return Err("not a member of this team");
                        }
                        let _prev = rec.capabilities.insert(account, capabilities);
                    }
                    None => {
                        if !o.role(&actor).is_some_and(|r| r >= OrgRole::Admin) {
                            return Err("only admins manage members");
                        }
                        let _prev = o.capabilities.insert(account, capabilities);
                    }
                }
                Ok(Value::Null)
            }
            SystemCmd::SetDefaultCapabilities {
                org,
                team,
                capabilities,
                actor,
                ..
            } => {
                let o = self.org_mut(&org)?;
                let Some(team) = team else {
                    if !o.role(&actor).is_some_and(|r| r >= OrgRole::Admin) {
                        return Err("only admins manage members");
                    }
                    o.default_capabilities = capabilities;
                    return Ok(Value::Null);
                };
                // A team's default: whoever may set its rows' capabilities.
                if !o.manages_team(&actor, &team)
                    && !o.may(&actor, Some(&team), crate::types::caps::MANAGE_MEMBERS)
                {
                    return Err("not allowed to manage this team");
                }
                let t = o.teams.get_mut(&team).ok_or("no such team")?;
                t.default_capabilities = Some(capabilities);
                Ok(Value::Null)
            }
            SystemCmd::RemoveTeamMember {
                org,
                team,
                account,
                actor,
                subtree,
                ..
            } => {
                let o = self.org_mut(&org)?;
                if actor != account && !o.manages_team(&actor, &team) {
                    return Err("not allowed to manage this team");
                }
                if !o.teams.contains_key(&team) {
                    return Err("no such team");
                }
                // The teams below go too (a private channel under an app's
                // team): a direct row there would keep them in. Whoever
                // manages `team` manages those; a leaver leaves them.
                let below = if subtree {
                    o.subtree(&team)
                } else {
                    vec![team.clone()]
                };
                for id in &below {
                    if let Some(t) = o.teams.get_mut(id) {
                        let _prev = t.members.remove(&account);
                        let _was = t.read_only.remove(&account);
                        let _caps = t.capabilities.remove(&account);
                    }
                }
                let t = o.teams.get_mut(&team).ok_or("no such team")?;
                // A removal, and a leave too, keeps them from
                // coming back by inheritance: an open team would otherwise put a
                // leaver straight back, and a read-only leaver would shed the
                // role. They return only when a lead re-adds them or an invite
                // names the team.
                let _new = t.removed.insert(account);
                Ok(Value::Null)
            }
            SystemCmd::CreateInvite {
                invite_key,
                invite,
                actor,
                teams_seen,
                ..
            } => {
                let o = s.orgs.get(&invite.org).ok_or("no such organization")?;
                let role = o.role(&actor).ok_or("not a member")?;
                // Below admin, only teams the inviter sees: an org-wide
                // INVITE_MEMBERS (which `may` falls back to) does not open a
                // restricted team they are not in.
                if teams_seen
                    && role < OrgRole::Admin
                    && !invite.teams.iter().all(|t| o.sees_team(&actor, t))
                {
                    return Err("an invite names only teams the inviter sees");
                }
                // INVITE_MEMBERS: below admin, the capability for the
                // organization (an invite naming no team) or for each team named
                // (a team's leads hold it there).
                let may_invite = if invite.teams.is_empty() {
                    o.may(&actor, None, crate::types::caps::INVITE_MEMBERS)
                } else {
                    invite
                        .teams
                        .iter()
                        .all(|t| o.may(&actor, Some(t), crate::types::caps::INVITE_MEMBERS))
                };
                if role < OrgRole::Admin && !may_invite {
                    return Err("inviting here needs the INVITE_MEMBERS capability");
                }
                if invite.role > role || (invite.role == OrgRole::Owner) {
                    return Err("cannot invite above your own role");
                }
                if s.sign_up_invites.contains_key(&invite_key) {
                    return Err("invite exists");
                }
                // Its creator is the actor, whatever the entry says.
                let invite = Invite {
                    created_by: actor,
                    ..invite
                };
                let _prev = s.invites.insert(invite_key, invite);
                Ok(Value::Null)
            }
            SystemCmd::RedeemInvite {
                invite_key,
                account,
                now,
                free_when_unchanged,
                ..
            } => {
                if !s.accounts.contains_key(&account) {
                    return Err("unknown account");
                }
                let org = s.redeem_org_invite(&invite_key, &account, now, free_when_unchanged)?;
                Ok(json!({ "org": org }))
            }
            SystemCmd::RevokeInvite {
                org,
                invite_key,
                actor,
                ..
            } => {
                let inv = s.invites.get(&invite_key).ok_or(INVITE_GONE)?;
                if inv.org != org {
                    return Err(INVITE_GONE);
                }
                if !s.sees_invite(inv, &actor) {
                    return Err("only the invite's creator, an admin, or whoever manages every team it names may revoke it");
                }
                let _gone = s.invites.remove(&invite_key);
                Ok(Value::Null)
            }
            SystemCmd::ScopeApps {
                org,
                apps,
                shared,
                nonce,
                actor,
                owner_in_team,
                member_child_teams,
                ..
            } => s.scope_apps(
                &org,
                apps,
                shared,
                &nonce,
                &actor,
                ScopeAsLogged {
                    owner_in_team,
                    member_child_teams,
                },
            ),
            SystemCmd::SetRecoveryKey {
                org,
                public_key,
                at,
                actor,
                ..
            } => {
                let recovery = public_key.map(|k| recovery_key_of(k, at)).transpose()?;
                let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
                if o.role(&actor) != Some(OrgRole::Owner) {
                    return Err(OWNER_ONLY_RECOVERY);
                }
                // A new key, or none: a recovery under way ends with the old one.
                o.recovery = recovery;
                Ok(Value::Null)
            }
            SystemCmd::StartRecovery {
                org, account, at, ..
            } => {
                if !s.accounts.contains_key(&account) {
                    return Err("unknown account");
                }
                let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
                if o.role(&account) == Some(OrgRole::Owner) {
                    return Err("that account owns the organization already");
                }
                let r = o.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
                if r.pending.is_some() {
                    return Err(RECOVERY_PENDING);
                }
                let pending = crate::types::PendingRecovery {
                    account,
                    started_at: at,
                    ready_at: at.saturating_add(crate::types::RECOVERY_DELAY_SECS),
                };
                let out = json!({ "startedAt": pending.started_at, "readyAt": pending.ready_at });
                r.pending = Some(pending);
                Ok(out)
            }
            SystemCmd::CancelRecovery {
                org,
                account,
                actor,
                ..
            } => {
                let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
                if o.role(&actor) != Some(OrgRole::Owner) {
                    return Err(OWNER_ONLY_RECOVERY);
                }
                let r = o.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
                if r.pending.as_ref().map(|p| &p.account) != Some(&account) {
                    return Err(NO_RECOVERY_PENDING);
                }
                r.pending = None;
                Ok(Value::Null)
            }
            SystemCmd::FinishRecovery { org, at, actor, .. } => {
                if !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
                }
                let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
                let r = o.recovery.as_mut().ok_or(NO_RECOVERY_KEY)?;
                let Some(p) = r.pending.as_ref().filter(|p| p.account == actor) else {
                    return Err(NO_RECOVERY_PENDING);
                };
                if at < p.ready_at {
                    return Err(RECOVERY_NOT_READY);
                }
                r.pending = None;
                let previous: Vec<AccountId> = o
                    .members
                    .iter()
                    .filter(|(_, r)| **r == OrgRole::Owner)
                    .map(|(a, _)| a.clone())
                    .collect();
                for a in &previous {
                    let _prev = o.members.remove(a);
                    let _caps = o.capabilities.remove(a);
                    let _meta = o.member_metadata.remove(a);
                    for t in o.teams.values_mut() {
                        let _prev = t.members.remove(a);
                        let _was = t.read_only.remove(a);
                        let _caps = t.capabilities.remove(a);
                    }
                }
                let _prev = o.members.insert(actor.clone(), OrgRole::Owner);
                // The previous owner's apps in the organization reach their
                // owner whatever the membership: they move with it.
                for app in s.apps.values_mut() {
                    if app.org.as_deref() == Some(org.as_str()) && previous.contains(&app.owner) {
                        app.owner.clone_from(&actor);
                    }
                }
                Ok(json!({ "org": org, "previousOwners": previous }))
            }
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
            Err(e) => err(&format!("malformed command: {e}")),
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
mod tests {

    use serde_json::{json, Value};

    use super::{
        Authority, Governance, OrgRole, Quotas, ReleaseScope, SystemCmd, SystemMachine,
        SystemState, INVITE_GONE, OPERATOR, QUOTA_PREFIX,
    };
    use crate::types::RefreshRecord;

    // The tests above `account_with` judge each command's own authority rules
    // (`run_authorized`); the proofs `run` also requires are tested below it.

    fn account(id: &str) -> SystemCmd {
        SystemCmd::CreateAccount {
            at: 0,
            account: serde_json::from_value(
                json!({ "accountId": id, "name": id, "createdAt": 0, "passkeys": [] }),
            )
            .unwrap(),
            invite: None,
            refresh: None,
            consent: serde_json::Value::Null,
        }
    }

    fn genesis(governance: Governance) -> SystemCmd {
        SystemCmd::Genesis {
            session_proofs: true,
            governance,
            release: Some("2.0.0".into()),
            operator_key: None,
            nodes: Vec::new(),
        }
    }

    fn all(who: Authority) -> Governance {
        Governance {
            releases: who,
            app_upgrades: who,
            outbound: who,
            ..Governance::default()
        }
    }

    fn register(m: &mut SystemMachine, id: u64, role: &str) {
        let node = serde_json::from_value(
            json!({ "id": id, "addr": format!("http://n{id}"), "role": role }),
        )
        .unwrap();
        m.run_authorized(SystemCmd::RegisterNode {
            node,
            proof: serde_json::Value::Null,
        })
        .unwrap();
    }

    #[test]
    fn job_nodes_are_listed_apart_from_replicas() {
        let mut m = SystemMachine::default();
        register(&mut m, 1, "replica");
        register(&mut m, 2, "job");
        let ids = |it: Vec<&crate::types::NodeRecord>| it.iter().map(|n| n.id).collect::<Vec<_>>();
        assert_eq!(ids(m.state.replica_nodes().collect()), vec![1]);
        assert_eq!(ids(m.state.job_nodes().collect()), vec![2]);
        assert!(m.state.replica_node(&2).is_none());
        assert!(m.state.replica_node(&1).is_some());
        m.run_authorized(SystemCmd::RemoveNode {
            id: 2,
            proof: serde_json::Value::Null,
        })
        .unwrap();
        assert!(m.state.job_nodes().next().is_none());
    }

    #[test]
    fn re_registering_a_node_never_changes_its_role() {
        let mut m = SystemMachine::default();
        register(&mut m, 1, "replica");
        register(&mut m, 2, "job");
        let node = |id: u64, addr: &str, role: &str| {
            serde_json::from_value(json!({ "id": id, "addr": addr, "role": role })).unwrap()
        };
        assert!(m
            .run_authorized(SystemCmd::RegisterNode {
                node: node(1, "u", "job"),
                proof: serde_json::Value::Null,
            })
            .is_err());
        assert!(m
            .run_authorized(SystemCmd::RegisterNode {
                node: node(2, "u", "replica"),
                proof: serde_json::Value::Null,
            })
            .is_err());
        assert_eq!(m.state.replica_nodes().count(), 1);
        assert_eq!(m.state.job_nodes().count(), 1);
        // The address of a node may change; so may anything after a removal.
        m.run_authorized(SystemCmd::RegisterNode {
            node: node(2, "v", "job"),
            proof: serde_json::Value::Null,
        })
        .unwrap();
        assert_eq!(m.state.nodes[&2].addr, "v");
        m.run_authorized(SystemCmd::RemoveNode {
            id: 2,
            proof: serde_json::Value::Null,
        })
        .unwrap();
        m.run_authorized(SystemCmd::RegisterNode {
            node: node(2, "v", "replica"),
            proof: serde_json::Value::Null,
        })
        .unwrap();
        assert_eq!(m.state.replica_nodes().count(), 2);
    }

    #[test]
    fn addresses_are_compared_by_their_normal_form() {
        use super::normalize_addr as n;
        let one = n("http://127.0.0.1:2528/");
        for other in [
            "http://127.0.0.1:2528",
            "HTTP://127.0.0.1:2528/",
            "http://127.0.0.1:2528//",
        ] {
            assert_eq!(n(other), one, "{other}");
        }
        assert_eq!(n("http://Example.com:80/"), n("http://example.com"));
        assert_ne!(n("http://127.0.0.1:2529"), one);
        assert_ne!(n("https://127.0.0.1:2528"), one);
        assert_eq!(n("https://Example.com:443/"), n("https://example.com"));
        assert_eq!(n("http://[::1]:80/"), n("HTTP://[0:0:0:0:0:0:0:1]"));
        assert_ne!(n("http://[::1]:2528"), n("http://[::2]:2528"));
        // No origin to compare: the lowercased text, so distinct stay distinct.
        assert_ne!(n("a.example:80"), n("b.example:80"));
        assert_eq!(n("A.example:80/"), n("a.example:80"));
    }

    #[test]
    fn a_node_cannot_take_an_address_another_node_holds() {
        let mut m = SystemMachine::default();
        register(&mut m, 1, "replica");
        let node = |id: u64, addr: &str| {
            serde_json::from_value(json!({ "id": id, "addr": addr, "role": "job" })).unwrap()
        };
        assert!(m
            .run_authorized(SystemCmd::RegisterNode {
                node: node(2, "HTTP://n1/"),
                proof: serde_json::Value::Null,
            })
            .is_err());
        m.run_authorized(SystemCmd::RegisterNode {
            node: node(2, "http://n2"),
            proof: serde_json::Value::Null,
        })
        .unwrap();
        // The holder may re-register its own address in another spelling.
        let own = serde_json::from_value(json!({ "id": 1, "addr": "http://N1/" })).unwrap();
        m.run_authorized(SystemCmd::RegisterNode {
            node: own,
            proof: serde_json::Value::Null,
        })
        .unwrap();
    }

    #[test]
    fn a_nodes_epoch_moves_with_every_registration_and_removal() {
        let mut m = SystemMachine::default();
        let epoch = |m: &SystemMachine| m.state.node_epoch.get(&2).copied().unwrap_or(0);
        register(&mut m, 2, "job");
        let first = epoch(&m);
        register(&mut m, 2, "job");
        assert!(epoch(&m) > first);
        let second = epoch(&m);
        m.run_authorized(SystemCmd::RemoveNode {
            id: 2,
            proof: serde_json::Value::Null,
        })
        .unwrap();
        assert!(epoch(&m) > second);
    }

    #[test]
    fn a_node_record_without_a_role_is_a_replica_and_an_unknown_role_is_refused() {
        let n: crate::types::NodeRecord =
            serde_json::from_value(json!({ "id": 1, "addr": "u" })).unwrap();
        assert_eq!(n.role, crate::types::NodeRole::Replica);
        assert!(serde_json::from_value::<crate::types::NodeRecord>(
            json!({ "id": 1, "addr": "u", "role": "gpu" })
        )
        .is_err());
    }

    /// The rules are written once, before anyone has an account, and an
    /// operator who restarts the cluster with other ones changes nothing.
    #[test]
    fn governance_is_fixed_at_genesis() {
        let mut m = SystemMachine::default();
        assert!(
            m.run_authorized(account("alice")).is_err(),
            "an account before genesis"
        );
        let _ok = m.run_authorized(genesis(all(Authority::Operator))).unwrap();
        let _ok = m.run_authorized(account("alice")).unwrap();
        assert!(m.run_authorized(genesis(all(Authority::Owner))).is_err());
        assert_eq!(m.state.governance(), all(Authority::Operator));

        // Even before any account, a second genesis is refused.
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        assert!(m.run_authorized(genesis(all(Authority::Operator))).is_err());
        assert_eq!(m.state.governance(), all(Authority::Owner));

        // State from before governance reads as owner-decided throughout.
        assert_eq!(SystemState::default().governance(), all(Authority::Owner));
    }

    /// Each decision (releases, app upgrades, outbound hosts) belongs to
    /// exactly one party, chosen independently at deployment: where the
    /// operator holds it the owner cannot take it, and the other way round.
    #[test]
    fn each_decision_belongs_to_the_party_the_deployment_chose() {
        let both = [Authority::Owner, Authority::Operator];
        for releases in both {
            for app_upgrades in both {
                for outbound in both {
                    let g = Governance {
                        releases,
                        app_upgrades,
                        outbound,
                        ..Governance::default()
                    };
                    check(&g);
                }
            }
        }
    }

    /// Leaving an open team sticks. Inheritance would put the
    /// leaver straight back, so leaving would change nothing; they come back
    /// only by an explicit act, a lead re-adding them, never by themselves.
    #[test]
    fn leaving_an_open_team_sticks_until_someone_lets_you_back_in() {
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob"] {
            let _ok = m.run_authorized(account(who)).unwrap();
        }
        let _prev = m.state.orgs.insert(
            "org".into(),
            serde_json::from_value(json!({
                "orgId": "org", "name": "Docs",
                "members": { "alice": "owner", "bob": "member" },
                "teams": { "open": {
                    "teamId": "open", "name": "Open", "join": "everyone",
                    "members": { "alice": true, "bob": false }
                } }
            }))
            .unwrap(),
        );
        let reaches = |m: &SystemMachine| m.state.orgs["org"].in_team("bob", "open");
        let join = |actor: &str| SystemCmd::SetTeamMember {
            org: "org".into(),
            team: "open".into(),
            account: "bob".into(),
            lead: false,
            role: None,
            capabilities: None,
            actor: actor.into(),
            consent: serde_json::Value::Null,
        };
        assert!(reaches(&m), "a direct row");

        let _ok = m
            .run_authorized(SystemCmd::RemoveTeamMember {
                org: "org".into(),
                team: "open".into(),
                account: "bob".into(),
                actor: "bob".into(),
                consent: serde_json::Value::Null,
                subtree: true,
            })
            .unwrap();
        assert!(!reaches(&m), "inheritance does not put a leaver back");
        assert!(
            m.run_authorized(join("bob")).is_err(),
            "nor may they rejoin themselves"
        );

        let _ok = m.run_authorized(join("alice")).unwrap();
        assert!(reaches(&m), "a lead re-adds them");
    }

    /// `MANAGE_APPLICATION`: in an organization, an admin or a member
    /// holding the capability (org-wide, or on the app's team) upgrades its
    /// apps and approves its outbound hosts, as the org owner does. A personal
    /// app stays its owner's, and a team-only grant reaches only that team.
    #[test]
    fn manage_application_lets_a_member_upgrade_and_approve_outbound() {
        const MANAGE_APPLICATION: u32 = 1 << 4;
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob", "carol", "dave", "erin", "frank"] {
            let _ok = m.run_authorized(account(who)).unwrap();
        }
        let _prev = m.state.orgs.insert(
            "org".into(),
            serde_json::from_value(json!({
                "orgId": "org", "name": "Docs",
                "members": {
                    "alice": "owner", "bob": "member", "carol": "member",
                    "dave": "admin", "frank": "member"
                },
                "capabilities": { "bob": MANAGE_APPLICATION },
                "teams": { "t": {
                    "teamId": "t", "name": "T", "join": "invite",
                    "members": { "frank": false },
                    "capabilities": { "frank": MANAGE_APPLICATION }
                } }
            }))
            .unwrap(),
        );
        for hash in ["b1", "b2"] {
            let _prev = m.state.bundles.insert(
                hash.into(),
                serde_json::from_value(json!({ "hash": hash, "size": 1, "uploadedBy": "alice" }))
                    .unwrap(),
            );
        }
        let app = |id: &str, org: Option<&str>, access: serde_json::Value| {
            serde_json::from_value::<crate::types::AppRecord>(json!({
                "appId": id, "name": id, "group": 2, "bundle": "b1", "owner": "alice",
                "org": org, "access": access, "replicas": [1, 2, 3], "createdAt": 0
            }))
            .unwrap()
        };
        for (id, a) in [
            (
                "in-org",
                app("in-org", Some("org"), json!({ "kind": "org" })),
            ),
            (
                "in-team",
                app(
                    "in-team",
                    Some("org"),
                    json!({ "kind": "team", "team": "t" }),
                ),
            ),
            (
                "personal",
                app("personal", None, json!({ "kind": "owner" })),
            ),
        ] {
            let _prev = m.state.apps.insert(id.into(), a);
        }
        let upgrade = |app: &str, actor: &str| SystemCmd::UpgradeApp {
            compute_bundle: None,
            app_id: app.into(),
            bundle: "b2".into(),
            by_operator: false,
            actor: actor.into(),
            at: 0,
            run: None,
            consent: serde_json::Value::Null,
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        };
        for (app, actor, may) in [
            ("in-org", "bob", true),
            ("in-team", "bob", true),
            ("in-org", "dave", true),
            ("in-org", "alice", true),
            ("in-org", "carol", false),
            ("in-org", "erin", false),
            ("in-team", "frank", true),
            ("in-org", "frank", false),
            ("personal", "bob", false),
            ("personal", "dave", false),
        ] {
            assert_eq!(
                m.run_authorized(upgrade(app, actor)).is_ok(),
                may,
                "{actor} upgrading {app}"
            );
        }

        let _prev = m.state.packages.insert(
            "p".into(),
            serde_json::from_value(json!({
                "name": "p", "signerId": "did:key:zP", "owner": "alice",
                "versions": { "1.0.0": {
                    "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                    "services": { "": "b1" }, "outbound": ["https://api.example.com/v1/"],
                    "installedBy": "alice", "installedAt": 0
                } }
            }))
            .unwrap(),
        );
        let outbound = |actor: &str| SystemCmd::SetOutboundApproval {
            scope: ReleaseScope::Org { org: "org".into() },
            package: "p".into(),
            selection: calimero_primitives::outbound::OutboundSelection::Manifest,
            replace: false,
            actor: actor.into(),
            consent: serde_json::Value::Null,
        };
        // An approval covers the whole organization, so a team-only grant
        // does not reach it.
        for (actor, may) in [
            ("bob", true),
            ("dave", true),
            ("alice", true),
            ("carol", false),
            ("frank", false),
            ("erin", false),
        ] {
            assert_eq!(
                m.run_authorized(outbound(actor)).is_ok(),
                may,
                "{actor} approving"
            );
        }
    }

    fn check(g: &Governance) {
        let op = |a: Authority| a == Authority::Operator;
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(g.clone())).unwrap();
        let _ok = m.run_authorized(account("alice")).unwrap();
        if let Some(a) = m.state.accounts.get_mut("alice") {
            let _new = a.approved_releases.insert("2.0.0".into());
        }
        for hash in ["b1", "b2"] {
            let _prev = m.state.bundles.insert(
                hash.into(),
                serde_json::from_value(json!({ "hash": hash, "size": 1, "uploadedBy": "alice" }))
                    .unwrap(),
            );
        }
        let _prev = m.state.apps.insert(
            "app".into(),
            serde_json::from_value(json!({
                "appId": "app", "name": "app", "group": 2, "bundle": "b1",
                "owner": "alice", "replicas": [1, 2, 3], "createdAt": 0
            }))
            .unwrap(),
        );
        let _prev = m.state.packages.insert(
            "p".into(),
            serde_json::from_value(json!({
                "name": "p", "signerId": "did:key:zP", "owner": "alice",
                "versions": { "1.0.0": {
                    "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                    "services": { "": "b1" }, "outbound": ["https://api.example.com/v1/"],
                    "installedBy": "alice", "installedAt": 0
                } }
            }))
            .unwrap(),
        );

        // App upgrades.
        let upgrade = |by_operator: bool, actor: &str| SystemCmd::UpgradeApp {
            compute_bundle: None,
            app_id: "app".into(),
            bundle: "b2".into(),
            by_operator,
            actor: actor.into(),
            at: 0,
            run: None,
            consent: serde_json::Value::Null,
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        };
        let by_operator = m.run_authorized(upgrade(true, "")).is_ok();
        let by_owner = m.run_authorized(upgrade(false, "alice")).is_ok();
        assert_eq!(by_operator, op(g.app_upgrades), "{g:?}: operator upgrade");
        assert_eq!(by_owner, !op(g.app_upgrades), "{g:?}: owner upgrade");
        assert!(
            m.run_authorized(upgrade(false, "mallory")).is_err(),
            "{g:?}"
        );

        // Release approvals.
        let ops_release = m.run_authorized(SystemCmd::SetOperatorRelease {
            version: "2.1.0".into(),
            approved: true,
            proof: serde_json::Value::Null,
        });
        let owner_release = m.run_authorized(SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Account,
            version: "2.1.0".into(),
            approved: true,
            actor: "alice".into(),
            consent: serde_json::Value::Null,
        });
        assert_eq!(
            ops_release.is_ok(),
            op(g.releases),
            "{g:?}: operator release"
        );
        assert_eq!(
            owner_release.is_ok(),
            !op(g.releases),
            "{g:?}: owner release"
        );
        let app = m.state.apps["app"].clone();
        assert!(m.state.release_may_hold(&app, Some("2.0.0")), "{g:?}");
        assert!(m.state.release_may_hold(&app, Some("2.1.0")), "{g:?}");
        assert!(!m.state.release_may_hold(&app, Some("9.9.9")), "{g:?}");

        // Outbound approvals.
        let outbound = |scope: ReleaseScope, actor: &str| SystemCmd::SetOutboundApproval {
            scope,
            package: "p".into(),
            selection: calimero_primitives::outbound::OutboundSelection::Manifest,
            replace: false,
            actor: actor.into(),
            consent: serde_json::Value::Null,
        };
        let ops_outbound = m
            .run_authorized(outbound(ReleaseScope::Cluster, ""))
            .is_ok();
        let owner_outbound = m
            .run_authorized(outbound(ReleaseScope::Account, "alice"))
            .is_ok();
        assert_eq!(ops_outbound, op(g.outbound), "{g:?}: operator outbound");
        assert_eq!(owner_outbound, !op(g.outbound), "{g:?}: owner outbound");
        let approved = m
            .state
            .outbound_approval(&app, "did:key:zP")
            .map(|a| a.prefixes.clone());
        assert_eq!(
            approved,
            Some(vec!["https://api.example.com/v1/".to_owned()]),
            "{g:?}: the app reads the approval its deployment's party made"
        );
    }

    /// "Move this team to the top" is `parent: Some(None)`; it was lost on its
    /// way through the log, where `null` read back as absent.
    #[test]
    fn moving_a_team_to_the_top_survives_the_log() {
        let update = |parent| SystemCmd::UpdateTeam {
            org: "o".into(),
            team: "t".into(),
            name: None,
            join: None,
            parent,
            metadata: None,
            actor: "a".into(),
        };
        let round = |cmd: &SystemCmd| -> SystemCmd {
            serde_json::from_slice(&serde_json::to_vec(cmd).unwrap()).unwrap()
        };
        assert!(matches!(
            round(&update(Some(None))),
            SystemCmd::UpdateTeam {
                parent: Some(None),
                ..
            }
        ));
        assert!(matches!(
            round(&update(None)),
            SystemCmd::UpdateTeam { parent: None, .. }
        ));
        assert!(matches!(
            round(&update(Some(Some("p".into())))),
            SystemCmd::UpdateTeam {
                parent: Some(Some(_)),
                ..
            }
        ));
    }

    /// A release recorded before `installers` existed (system log entries and
    /// snapshots are JSON) reads with none and writes back byte for byte as
    /// before; installing the same release again records the new installer,
    /// who then sees it, and the record restores from a snapshot with them.
    #[test]
    fn a_release_from_before_installers_reads_and_records_who_installs_it_again() {
        use super::{PackageVersion, OPERATOR};

        // `PackageVersion` as the previous release wrote it.
        let old = json!({
            "version": "1.0.0", "mpk": "ab", "mpkSize": 3, "source": "upload",
            "services": { "": "cd" }, "metadata": null, "outbound": [],
            "installedBy": "alice", "installedAt": 7,
        });
        let release: PackageVersion = serde_json::from_value(old.clone()).unwrap();
        assert!(release.installers.is_empty());
        assert_eq!(serde_json::to_value(&release).unwrap(), old);

        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob", "carol"] {
            let _ok = m.run_authorized(account(who)).unwrap();
        }
        let install = |by: &str| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: PackageVersion {
                installed_by: by.into(),
                ..release.clone()
            },
        };
        let _ok = m.run_authorized(install("alice")).unwrap();
        let snapshot = serde_json::to_value(&m.state).unwrap();
        assert_eq!(snapshot["packages"]["p"]["versions"]["1.0.0"], old);
        let seen =
            |s: &SystemState, who: &str| s.sees_release(&s.packages["p"].versions["1.0.0"], who);
        assert!(seen(&m.state, "alice"));
        assert!(!seen(&m.state, "bob"), "another account, no shared org");
        assert!(!seen(&m.state, OPERATOR));

        // Installed again: by Bob (twice), then by the operator, which puts
        // it in the cluster's catalog; Alice stays first.
        let _ok = m.run_authorized(install("bob")).unwrap();
        let _ok = m.run_authorized(install("bob")).unwrap();
        assert!(seen(&m.state, "bob"));
        assert!(!seen(&m.state, "carol") && !seen(&m.state, OPERATOR));
        let _ok = m.run_authorized(install(OPERATOR)).unwrap();
        let v = &m.state.packages["p"].versions["1.0.0"];
        assert_eq!(v.installed_by, "alice");
        assert_eq!(
            v.installers.iter().collect::<Vec<_>>(),
            vec!["bob", OPERATOR]
        );
        assert!(seen(&m.state, OPERATOR) && seen(&m.state, "carol"));

        // Other bytes under the same version are still refused.
        let other = SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: PackageVersion {
                mpk: "ef".into(),
                installed_by: "carol".into(),
                ..release.clone()
            },
        };
        assert!(m.run_authorized(other).is_err());

        // A snapshot carries the installers; an old one restores without.
        let restored: SystemState =
            serde_json::from_value(serde_json::to_value(&m.state).unwrap()).unwrap();
        assert!(seen(&restored, "bob"));
        let old_state: SystemState = serde_json::from_value(snapshot).unwrap();
        assert!(old_state.packages["p"].versions["1.0.0"]
            .installers
            .is_empty());
        assert!(seen(&old_state, "alice") && !seen(&old_state, "bob"));
    }

    /// A release's compute component is recorded as the compute part of its service: no app
    /// runs it, the app's record carries it next to `bundle` on create and upgrade, and
    /// uninstalling the release takes it with the service.
    #[test]
    fn a_compute_component_is_recorded_beside_its_service_and_never_run_as_an_app() {
        use super::{BundleSource, PackageVersion};
        use calimero_bundle::UpgradeAuthority;
        use serde_json::Value;
        use std::collections::{BTreeMap, BTreeSet};
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        let _ok = m.run_authorized(account("alice")).unwrap();
        let release = |version: &str, main: &str, job: Option<&str>| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: PackageVersion {
                components: Default::default(),
                pipelines: Default::default(),
                version: version.into(),
                mpk: format!("mpk{version}"),
                mpk_size: 1,
                source: BundleSource::Upload,
                services: BTreeMap::from([(String::new(), main.to_owned())]),
                compute: job
                    .map(|j| BTreeMap::from([(String::new(), j.to_owned())]))
                    .unwrap_or_default(),
                part_sizes: BTreeMap::new(),
                metadata: Value::Null,
                outbound: Vec::new(),
                installed_by: "alice".into(),
                installed_at: 0,
                installers: BTreeSet::new(),
                upgrades: UpgradeAuthority::Owner,
            },
        };
        let _ok = m
            .run_authorized(release("1.0.0", "m1", Some("j1")))
            .unwrap();
        let _ok = m.run_authorized(release("1.1.0", "m2", None)).unwrap();
        let s = &m.state;
        assert!(s.bundles["j1"].package.as_ref().unwrap().compute);
        assert!(!s.bundles["m1"].package.as_ref().unwrap().compute);
        assert!(s.is_compute_bundle("j1") && !s.is_compute_bundle("m1"));
        assert!(!s.sees_bundle("j1", "alice") && s.sees_bundle("m1", "alice"));
        assert!(s.ships_compute("m1", Some("j1")) && s.ships_compute("m1", None));
        assert!(!s.ships_compute("m2", Some("j1")) && !s.ships_compute("m1", Some("m1")));

        let e = m
            .run_unproven(instance("bad", "alice", "j1", None))
            .unwrap_err();
        assert!(e.contains("compute component"), "{e}");
        let with_compute = |id: &str, compute: &str| SystemCmd::CreateApp {
            app: Box::new(
                serde_json::from_value(json!({
                    "appId": id, "name": id, "group": 0, "bundle": "m1", "owner": "alice",
                    "replicas": [1], "createdAt": 10, "computeBundle": compute,
                }))
                .unwrap(),
            ),
        };
        let e = m.run_unproven(with_compute("bad", "j2")).unwrap_err();
        assert!(e.contains("not shipped"), "{e}");
        let _ok = m.run_unproven(with_compute("app", "j1")).unwrap();
        assert_eq!(m.state.apps["app"].compute_bundle.as_deref(), Some("j1"));

        let upgrade = |bundle: &str, compute: Option<&str>| SystemCmd::UpgradeApp {
            compute_bundle: compute.map(str::to_owned),
            app_id: "app".into(),
            bundle: bundle.into(),
            by_operator: false,
            actor: "alice".into(),
            at: 0,
            run: None,
            consent: Value::Null,
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        };
        let e = m.run_unproven(upgrade("j1", None)).unwrap_err();
        assert!(e.contains("compute component"), "{e}");
        let e = m.run_unproven(upgrade("m2", Some("j1"))).unwrap_err();
        assert!(e.contains("not shipped"), "{e}");
        assert_eq!(m.state.apps["app"].bundle, "m1");
        let _ok = m.run_unproven(upgrade("m2", None)).unwrap();
        assert_eq!(m.state.apps["app"].compute_bundle, None);

        let uninstall = |version: &str| SystemCmd::UninstallPackage {
            package: "p".into(),
            version: Some(version.into()),
            actor: "alice".into(),
        };
        let _ok = m.run_unproven(uninstall("1.0.0")).unwrap();
        assert!(!m.state.bundles.contains_key("m1") && !m.state.bundles.contains_key("j1"));
    }

    /// A release with a compute component counts each part's own size against the bundle
    /// quota, not the whole `.mpk` once per part.
    #[test]
    fn a_compute_release_is_charged_each_parts_own_size() {
        use super::{BundleSource, PackageVersion};
        use calimero_bundle::UpgradeAuthority;
        use serde_json::Value;
        use std::collections::{BTreeMap, BTreeSet};
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        let _ok = m.run_authorized(account("alice")).unwrap();
        m.state.quotas.max_bundle_bytes_per_account = 37;
        let release = |version: &str, main: &str, job: &str| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: PackageVersion {
                components: Default::default(),
                pipelines: Default::default(),
                version: version.into(),
                mpk: format!("mpk{version}"),
                mpk_size: 40,
                source: BundleSource::Upload,
                services: BTreeMap::from([(String::new(), main.to_owned())]),
                compute: BTreeMap::from([(String::new(), job.to_owned())]),
                part_sizes: BTreeMap::from([(main.to_owned(), 30), (job.to_owned(), 7)]),
                metadata: Value::Null,
                outbound: Vec::new(),
                installed_by: "alice".into(),
                installed_at: 0,
                installers: BTreeSet::new(),
                upgrades: UpgradeAuthority::Owner,
            },
        };
        let _ok = m.run_authorized(release("1.0.0", "m1", "j1")).unwrap();
        assert_eq!(m.state.bundles["m1"].size, 30);
        assert_eq!(m.state.bundles["j1"].size, 7);
        let e = m.run_authorized(release("1.1.0", "m2", "j2")).unwrap_err();
        assert!(e.contains("quota exceeded"), "{e}");
    }

    /// An uninstall that would remove a compute component an app still runs is refused, like
    /// one that would remove its main component.
    #[test]
    fn a_release_whose_compute_component_an_app_still_uses_is_not_uninstalled() {
        use super::{BundleSource, PackageVersion};
        use calimero_bundle::UpgradeAuthority;
        use serde_json::Value;
        use std::collections::{BTreeMap, BTreeSet};
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        let _ok = m.run_authorized(account("alice")).unwrap();
        let install = |version: &str, main: &str, job: &str| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: PackageVersion {
                components: Default::default(),
                pipelines: Default::default(),
                version: version.into(),
                mpk: format!("mpk{version}"),
                mpk_size: 1,
                source: BundleSource::Upload,
                services: BTreeMap::from([(String::new(), main.to_owned())]),
                compute: BTreeMap::from([(String::new(), job.to_owned())]),
                part_sizes: BTreeMap::new(),
                metadata: Value::Null,
                outbound: Vec::new(),
                installed_by: "alice".into(),
                installed_at: 0,
                installers: BTreeSet::new(),
                upgrades: UpgradeAuthority::Owner,
            },
        };
        let _ok = m.run_authorized(install("1.0.0", "m1", "j1")).unwrap();
        let _ok = m.run_authorized(install("1.1.0", "m1", "j2")).unwrap();
        // The app runs the main component both releases share, and the first one's compute.
        let app = SystemCmd::CreateApp {
            app: Box::new(
                serde_json::from_value(json!({
                    "appId": "app", "name": "app", "group": 0, "bundle": "m1", "owner": "alice",
                    "replicas": [1], "createdAt": 10, "computeBundle": "j1",
                }))
                .unwrap(),
            ),
        };
        let _ok = m.run_unproven(app).unwrap();
        let uninstall = |version: &str| SystemCmd::UninstallPackage {
            package: "p".into(),
            version: Some(version.into()),
            actor: "alice".into(),
        };
        let e = m.run_unproven(uninstall("1.0.0")).unwrap_err();
        assert!(e.contains("still runs"), "{e}");
        assert!(m.state.bundles.contains_key("j1"));
        let _ok = m.run_unproven(uninstall("1.1.0")).unwrap();
        assert!(m.state.bundles.contains_key("m1") && !m.state.bundles.contains_key("j2"));
    }

    /// An account `id` holding the passkey `pk`.
    fn account_with(id: &str, pk: &mut calimero_identity::testing::SoftAuthenticator) -> SystemCmd {
        let rp = calimero_identity::RelyingParty {
            id: "localhost".into(),
            origins: vec!["http://localhost:3000".into()],
            require_uv: false,
        };
        let challenge = calimero_identity::random::<32>();
        let credential =
            calimero_identity::verify_registration(&rp, &challenge, &pk.register(&challenge))
                .unwrap();
        let account: crate::types::Account = serde_json::from_value(json!({
            "accountId": id, "name": id, "createdAt": 0,
            "passkeys": [credential], "approvedReleases": ["1.0.0"],
        }))
        .unwrap();
        let args = super::account_args(&account, None);
        SystemCmd::CreateAccount {
            at: 0,
            consent: consent(pk, &format!("account:{id}"), "__create_account", &args),
            account,
            invite: None,
            refresh: None,
        }
    }

    fn consent(
        pk: &mut calimero_identity::testing::SoftAuthenticator,
        subject: &str,
        method: &str,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let statement = crate::proof::Statement::new(subject, method, args);
        let challenge = statement.challenge().unwrap();
        serde_json::to_value(crate::proof::Consent {
            statement,
            assertion: pk.assert(&challenge),
            at: 0,
        })
        .unwrap()
    }

    fn approve_with(actor: &str, version: &str, consent: serde_json::Value) -> SystemCmd {
        SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Account,
            version: version.into(),
            approved: true,
            actor: actor.into(),
            consent,
        }
    }

    /// A cluster with alice and bob, each with a passkey.
    fn two_accounts() -> (
        SystemMachine,
        calimero_identity::testing::SoftAuthenticator,
        calimero_identity::testing::SoftAuthenticator,
    ) {
        let mut m = SystemMachine::default();
        let _ok = m.run(genesis(all(Authority::Owner))).unwrap();
        let mut alice = calimero_identity::testing::SoftAuthenticator::new(
            "localhost",
            "http://localhost:3000",
        );
        let mut bob = calimero_identity::testing::SoftAuthenticator::new(
            "localhost",
            "http://localhost:3000",
        );
        let _ok = m.run(account_with("alice", &mut alice)).unwrap();
        let _ok = m.run(account_with("bob", &mut bob)).unwrap();
        (m, alice, bob)
    }

    /// A decision applies only with its actor's passkey
    /// consent over exactly that decision, and only once.
    #[test]
    fn a_decision_applies_with_its_makers_consent_once() {
        let (mut m, mut alice, mut bob) = two_accounts();
        let args = json!({ "version": "2.0.0" });
        for (why, c) in [
            ("none", serde_json::Value::Null),
            ("an assertion alone, as before", {
                let mut c = consent(&mut alice, "account:alice", "__approve_release", &args);
                c["assertion"].take()
            }),
            (
                "bob's passkey",
                consent(&mut bob, "account:alice", "__approve_release", &args),
            ),
            (
                "bob's subject",
                consent(&mut alice, "account:bob", "__approve_release", &args),
            ),
            (
                "a withdrawal",
                consent(&mut alice, "account:alice", "__withdraw_release", &args),
            ),
            (
                "another release",
                consent(
                    &mut alice,
                    "account:alice",
                    "__approve_release",
                    &json!({ "version": "3.0.0" }),
                ),
            ),
        ] {
            assert!(m.run(approve_with("alice", "2.0.0", c)).is_err(), "{why}");
        }
        assert_eq!(m.state.accounts["alice"].approved_releases.len(), 1);

        let real = consent(&mut alice, "account:alice", "__approve_release", &args);
        let _ok = m.run(approve_with("alice", "2.0.0", real.clone())).unwrap();
        assert!(m.state.accounts["alice"]
            .approved_releases
            .contains("2.0.0"));
        // Withdrawn, then the old consent logged again: refused.
        let w = consent(&mut alice, "account:alice", "__withdraw_release", &args);
        let _ok = m
            .run(SystemCmd::SetReleaseApproval {
                scope: ReleaseScope::Account,
                version: "2.0.0".into(),
                approved: false,
                actor: "alice".into(),
                consent: w,
            })
            .unwrap();
        assert_eq!(
            m.run(approve_with("alice", "2.0.0", real)).unwrap_err(),
            "that consent was used already"
        );
        assert!(!m.state.accounts["alice"]
            .approved_releases
            .contains("2.0.0"));
    }

    /// An organization-wide run is consented to once; each app's upgrade
    /// is then judged by the run, never by a consent of its own.
    #[test]
    fn an_upgrade_names_its_consent_or_its_run() {
        let (mut m, mut alice, _bob) = two_accounts();
        let upgrade = |consent: serde_json::Value, run: Option<&str>| SystemCmd::UpgradeApp {
            compute_bundle: None,
            app_id: "app".into(),
            bundle: "b2".into(),
            by_operator: false,
            actor: "alice".into(),
            at: 0,
            run: run.map(str::to_owned),
            consent,
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        };
        assert!(super::authorize(&m.state, &upgrade(serde_json::Value::Null, None)).is_err());
        let c = consent(
            &mut alice,
            "app",
            "__upgrade",
            &json!({ "bundle": "b2", "migrate": null }),
        );
        assert!(super::authorize(&m.state, &upgrade(c, None))
            .unwrap()
            .is_some());
        let c = consent(&mut alice, "app", "__upgrade", &json!({ "bundle": "b3" }));
        assert!(super::authorize(&m.state, &upgrade(c, None)).is_err());
        // A run nobody started names nothing.
        assert!(super::authorize(&m.state, &upgrade(serde_json::Value::Null, Some("r1"))).is_err());
        let _ = &mut m;
    }

    type Soft = calimero_identity::testing::SoftAuthenticator;

    /// The operator's signing key in these tests, and what genesis records.
    fn operator_signer() -> (ed25519_dalek::SigningKey, crate::proof::OperatorKey) {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let public = crate::proof::OperatorKey {
            key: hex::encode(key.verifying_key().to_bytes()),
            cluster: "test".into(),
        };
        (key, public)
    }

    /// The operator's signature over `method` on `subject` with `args`.
    fn signed(subject: &str, method: &str, args: &serde_json::Value) -> serde_json::Value {
        use ed25519_dalek::Signer;
        let (key, public) = operator_signer();
        let statement = crate::proof::Statement::new(subject, method, args);
        let signature = key.sign(&statement.operator_message(&public.cluster));
        serde_json::to_value(crate::proof::OperatorProof {
            statement,
            signature: hex::encode(signature.to_bytes()),
        })
        .unwrap()
    }

    /// A cluster with the operator's key, alice (owner of `org`), bob (a
    /// member) and carol (outside it), each with a passkey, a restricted team
    /// `t`, and an app on nodes 1-3.
    fn proven_cluster() -> (SystemMachine, Soft, Soft, Soft) {
        let mut m = SystemMachine::default();
        let _ok = m
            .run(SystemCmd::Genesis {
                session_proofs: true,
                governance: all(Authority::Owner),
                release: Some("2.0.0".into()),
                operator_key: Some(operator_signer().1),
                nodes: [1, 2, 3, 4]
                    .map(|id| crate::types::NodeRecord {
                        id,
                        addr: format!("http://n{id}/"),
                        zone: String::new(),
                        role: crate::types::NodeRole::Replica,
                        pool: String::new(),
                    })
                    .to_vec(),
            })
            .unwrap();
        let mut pks = Vec::new();
        for who in ["alice", "bob", "carol"] {
            let mut pk = Soft::new("localhost", "http://localhost:3000");
            let _ok = m.run(account_with(who, &mut pk)).unwrap();
            pks.push(pk);
        }
        let _prev = m.state.orgs.insert(
            "org".into(),
            serde_json::from_value(json!({
                "orgId": "org", "name": "Docs",
                "members": { "alice": "owner", "bob": "member" },
                "teams": { "t": { "teamId": "t", "name": "T", "join": "invite", "members": {} } }
            }))
            .unwrap(),
        );
        let _prev = m.state.apps.insert(
            "app".into(),
            serde_json::from_value(json!({
                "appId": "app", "name": "app", "group": 2, "bundle": "b1",
                "owner": "alice", "replicas": [1, 2, 3], "createdAt": 0
            }))
            .unwrap(),
        );
        let carol = pks.pop().unwrap();
        let bob = pks.pop().unwrap();
        let alice = pks.pop().unwrap();
        (m, alice, bob, carol)
    }

    /// `cmd` applies only with `pk`'s consent over exactly `args`, and once:
    /// refused without one, with one over `other`, and on replay.
    fn needs_consent(
        m: &mut SystemMachine,
        pk: &mut Soft,
        (subject, method): (&str, &str),
        args: &serde_json::Value,
        other: &serde_json::Value,
        cmd: impl Fn(serde_json::Value) -> SystemCmd,
    ) {
        assert!(
            m.run(cmd(serde_json::Value::Null)).is_err(),
            "{method}: none"
        );
        let wrong = consent(pk, subject, method, other);
        assert!(m.run(cmd(wrong)).is_err(), "{method}: other args");
        let real = consent(pk, subject, method, args);
        if let Err(e) = m.run(cmd(real.clone())) {
            panic!("{method}: {e}");
        }
        assert_eq!(
            m.run(cmd(real)).unwrap_err(),
            "that consent was used already",
            "{method}: replay"
        );
    }

    /// As [`needs_consent`], for the operator's signature.
    fn needs_signature(
        m: &mut SystemMachine,
        (subject, method): (&str, &str),
        args: &serde_json::Value,
        other: &serde_json::Value,
        cmd: impl Fn(serde_json::Value) -> SystemCmd,
    ) {
        assert!(
            m.run(cmd(serde_json::Value::Null)).is_err(),
            "{method}: none"
        );
        assert!(
            m.run(cmd(signed(subject, method, other))).is_err(),
            "{method}: other args"
        );
        let mut forged = signed(subject, method, args);
        forged["signature"] = json!(hex::encode([0_u8; 64]));
        assert!(m.run(cmd(forged)).is_err(), "{method}: not the operator's");
        let real = signed(subject, method, args);
        if let Err(e) = m.run(cmd(real.clone())) {
            panic!("{method}: {e}");
        }
        assert_eq!(
            m.run(cmd(real)).unwrap_err(),
            "that consent was used already",
            "{method}: replay"
        );
    }

    /// Credentials an account gets (passkeys, API keys) apply only with a
    /// passkey it already has, over exactly that credential, once.
    #[test]
    fn credentials_need_the_accounts_own_passkey() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        let mut phone = Soft::new("localhost", "http://localhost:3000");
        let rp = calimero_identity::RelyingParty {
            id: "localhost".into(),
            origins: vec!["http://localhost:3000".into()],
            require_uv: false,
        };
        let challenge = calimero_identity::random::<32>();
        let cred =
            calimero_identity::verify_registration(&rp, &challenge, &phone.register(&challenge))
                .unwrap();
        let add = |consent| SystemCmd::AddPasskey {
            at: 0,
            account: "alice".into(),
            credential: cred.clone(),
            consent,
        };
        let args = json!({ "credentialId": cred.id, "publicKey": cred.public_key });
        // Neither the new passkey itself nor another account's approves it.
        for pk in [&mut phone, &mut bob] {
            let c = consent(pk, "account:alice", "__add_passkey", &args);
            assert!(m.run(add(c)).is_err());
        }
        let other = json!({ "credentialId": cred.id, "publicKey": "AAAA" });
        needs_consent(
            &mut m,
            &mut alice,
            ("account:alice", "__add_passkey"),
            &args,
            &other,
            add,
        );
        assert_eq!(m.state.accounts["alice"].passkeys.len(), 2);

        let remove = |consent| SystemCmd::RemovePasskey {
            account: "alice".into(),
            credential_id: cred.id.clone(),
            now: 1,
            consent,
        };
        needs_consent(
            &mut m,
            &mut alice,
            ("account:alice", "__remove_passkey"),
            &json!({ "credentialId": cred.id }),
            &json!({ "credentialId": "another" }),
            remove,
        );
        assert_eq!(m.state.accounts["alice"].passkeys.len(), 1);

        let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
            "keyId": "k1", "account": "alice", "label": "ci", "apps": ["app"],
            "methods": null, "readOnly": true, "allowConfirm": false, "expiresAt": 99,
        }))
        .unwrap();
        let args = super::api_key_args("h1", &record);
        let mut wider = args.clone();
        wider["readOnly"] = json!(false);
        let create = |consent| SystemCmd::CreateApiKey {
            hash: "h1".into(),
            record: record.clone(),
            consent,
            owner_in_org: true,
            owner_in_team: true,
        };
        // Another key's hash under the same consent: refused.
        let c = consent(&mut alice, "account:alice", "__create_api_key", &args);
        assert!(m
            .run(SystemCmd::CreateApiKey {
                hash: "h2".into(),
                record: record.clone(),
                consent: c,
                owner_in_org: true,
                owner_in_team: true,
            })
            .is_err());
        needs_consent(
            &mut m,
            &mut alice,
            ("account:alice", "__create_api_key"),
            &args,
            &wider,
            create,
        );
        assert!(m.state.api_keys.contains_key("h1"));
    }

    /// A cluster begun before session proofs replays what it logged then as
    /// it applied it, proofless, and requires the proofs from the entry its
    /// first leader on this release wrote; genesis requires them from the
    /// start. The mark is written once and only tightens.
    #[test]
    fn session_commands_logged_before_their_proofs_apply_as_they_did() {
        let mut m = SystemMachine::default();
        m.state.governance = Some(Governance::default());
        m.index = 3;
        let _ok = m.run(account("alice")).unwrap();
        m.index = 5;
        let _ok = m.run(SystemCmd::RequireSessionProofs).unwrap();
        assert_eq!(m.state.session_proofs_from, Some(5));
        assert!(m.run(SystemCmd::RequireSessionProofs).is_err(), "once");
        m.index = 6;
        assert!(m.run(account("bob")).is_err(), "proofless after the mark");
        // A restart from a snapshot before the mark replays what came before it.
        m.index = 4;
        let _ok = m.run(account("carol")).unwrap();

        let mut fresh = SystemMachine {
            index: 1,
            ..SystemMachine::default()
        };
        let _ok = fresh.run(genesis(all(Authority::Owner))).unwrap();
        assert_eq!(fresh.state.session_proofs_from, Some(1));
        fresh.index = 2;
        assert!(
            fresh.run(account("alice")).is_err(),
            "required from genesis"
        );
        // State from before reads without the mark and writes as it did.
        let before = serde_json::to_value(SystemState::default()).unwrap();
        assert!(before.get("sessionProofsFrom").is_none());
    }

    /// A refresh token for `account`'s `passkey`, as the API makes one: the
    /// token, its hash, and its record.
    fn refresh_token(
        account: &str,
        passkey: &str,
        session: &str,
    ) -> (String, String, crate::types::RefreshRecord) {
        let token = format!(
            "rt_{}",
            calimero_identity::b64url(&calimero_identity::random::<32>())
        );
        let hash = calimero_identity::b64url(&calimero_identity::sha256(token.as_bytes()));
        let record = crate::types::RefreshRecord {
            account: account.into(),
            session: session.into(),
            expires_at: 99,
            passkey: Some(passkey.into()),
            app: None,
            origin: None,
            key: crate::proof::refresh_key(&token),
            opened_at: 0,
        };
        (token, hash, record)
    }

    /// A sign-in's refresh token applies only with that sign-in's assertion,
    /// by the passkey its record names, over a challenge stating that token;
    /// once.
    #[test]
    fn a_sign_ins_refresh_token_needs_its_passkeys_assertion() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        let (_token, hash, record) = refresh_token("alice", &alice.credential_id(), "s1");
        let create = |consent| SystemCmd::CreateRefresh {
            hash: hash.clone(),
            record: record.clone(),
            consent,
        };
        let args = super::sign_in_args(&hash, &record);
        // Bob's passkey does not sign alice in, nor does a record naming bob's
        // passkey take alice's assertion.
        let c = consent(&mut bob, "session", "__sign_in", &args);
        assert!(m.run(create(c)).is_err());
        let c = consent(&mut alice, "session", "__sign_in", &args);
        let mut bobs = record.clone();
        bobs.passkey = Some(bob.credential_id());
        assert!(m
            .run(SystemCmd::CreateRefresh {
                hash: hash.clone(),
                record: bobs,
                consent: c,
            })
            .is_err());
        // An assertion over another token: refused.
        let (_t, other_hash, other) = refresh_token("alice", &alice.credential_id(), "s1");
        let c = consent(
            &mut alice,
            "session",
            "__sign_in",
            &super::sign_in_args(&other_hash, &other),
        );
        assert!(m.run(create(c)).is_err());
        needs_consent(
            &mut m,
            &mut alice,
            ("session", "__sign_in"),
            &args,
            &json!({ "refresh": hash, "key": "00", "session": "s1", "expiresAt": 99 }),
            create,
        );
        assert_eq!(m.state.refresh[&hash].account, "alice");
    }

    /// A refresh token is exchanged only with its own key's signature, for
    /// the session it already belongs to.
    #[test]
    fn a_refresh_token_is_exchanged_with_its_own_key() {
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        let (token, hash, record) = refresh_token("alice", &alice.credential_id(), "s1");
        let c = consent(
            &mut alice,
            "session",
            "__sign_in",
            &super::sign_in_args(&hash, &record),
        );
        let _ok = m
            .run(SystemCmd::CreateRefresh {
                hash: hash.clone(),
                record: record.clone(),
                consent: c,
            })
            .unwrap();
        let (next, next_hash, next_record) = refresh_token("alice", &alice.credential_id(), "s1");
        let message = |r: &crate::types::RefreshRecord| {
            crate::proof::refresh_use_message(
                &hash,
                &next_hash,
                &r.key,
                &r.session,
                r.expires_at,
                5,
            )
        };
        let exchange =
            |record: crate::types::RefreshRecord, signature: String| SystemCmd::UseRefresh {
                old_hash: hash.clone(),
                new_hash: next_hash.clone(),
                record,
                now: 5,
                signature,
            };
        for (why, cmd) in [
            ("unsigned", exchange(next_record.clone(), String::new())),
            (
                "signed with another token",
                exchange(
                    next_record.clone(),
                    crate::proof::sign_refresh_use(&next, &message(&next_record)),
                ),
            ),
            (
                "into another session",
                exchange(
                    crate::types::RefreshRecord {
                        session: "s2".into(),
                        ..next_record.clone()
                    },
                    crate::proof::sign_refresh_use(
                        &token,
                        &message(&crate::types::RefreshRecord {
                            session: "s2".into(),
                            ..next_record.clone()
                        }),
                    ),
                ),
            ),
            (
                "signed over another expiry",
                exchange(
                    next_record.clone(),
                    crate::proof::sign_refresh_use(
                        &token,
                        &message(&crate::types::RefreshRecord {
                            expires_at: 1,
                            ..next_record.clone()
                        }),
                    ),
                ),
            ),
            // A session keeps the registered origin it was opened on, so
            // removing the origin still ends it after an exchange.
            (
                "onto another origin",
                exchange(
                    crate::types::RefreshRecord {
                        origin: Some("https://elsewhere.example.com".into()),
                        ..next_record.clone()
                    },
                    crate::proof::sign_refresh_use(&token, &message(&next_record)),
                ),
            ),
        ] {
            assert!(m.run(cmd).is_err(), "{why}");
        }
        assert!(m.state.refresh.contains_key(&hash));
        let signed = crate::proof::sign_refresh_use(&token, &message(&next_record));
        let _ok = m
            .run(exchange(next_record.clone(), signed.clone()))
            .unwrap();
        assert!(m.state.refresh.contains_key(&next_hash));
        assert!(m.run(exchange(next_record, signed)).is_err(), "replayed");

        // A token from before keys is not exchanged at all.
        let (old, old_hash, mut keyless) = refresh_token("alice", &alice.credential_id(), "s3");
        keyless.key = String::new();
        let _prev = m.state.refresh.insert(old_hash.clone(), keyless.clone());
        let (_n, new_hash, new_record) = refresh_token("alice", &alice.credential_id(), "s3");
        let message = crate::proof::refresh_use_message(
            &old_hash,
            &new_hash,
            &new_record.key,
            "s3",
            new_record.expires_at,
            5,
        );
        assert_eq!(
            m.run(SystemCmd::UseRefresh {
                old_hash,
                new_hash,
                record: new_record,
                now: 5,
                signature: crate::proof::sign_refresh_use(&old, &message),
            })
            .unwrap_err(),
            "this refresh token has no key; sign in again"
        );
    }

    /// Passkey and session times come from the entries: a sign-up's and an
    /// added passkey's `at`, a sign-in's `opened_at`. The latest sign-in wins
    /// whatever order they commit in, and an exchange keeps when its session
    /// opened. Entries an earlier release logged carry none, and leave the
    /// times unknown on every replica instead of making one up.
    #[test]
    fn passkey_and_session_times_come_from_their_entries() {
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        let cred = alice.credential_id();
        // `proven_cluster`'s sign-ups are an earlier release's: no `at`.
        assert!(m.state.accounts["alice"].passkey_times.is_empty());
        let mut sign_in = |m: &mut SystemMachine, session: &str, opened_at: u64| {
            let (token, hash, record) = refresh_token("alice", &cred, session);
            let record = crate::types::RefreshRecord {
                opened_at,
                ..record
            };
            let c = consent(
                &mut alice,
                "session",
                "__sign_in",
                &super::sign_in_args(&hash, &record),
            );
            let _ok = m
                .run(SystemCmd::CreateRefresh {
                    hash: hash.clone(),
                    record,
                    consent: c,
                })
                .unwrap();
            (token, hash)
        };
        let _old = sign_in(&mut m, "s0", 0);
        assert!(
            m.state.accounts["alice"].passkey_times.is_empty(),
            "a sign-in an earlier release logged moves nothing"
        );
        let (token, hash) = sign_in(&mut m, "s1", 40);
        let _later = sign_in(&mut m, "s2", 50);
        let _earlier = sign_in(&mut m, "s3", 45);
        assert_eq!(
            m.state.accounts["alice"].passkey_times[&cred],
            crate::types::PasskeyTimes {
                added_at: None,
                last_sign_in_at: Some(50),
            },
            "added before it was kept; the latest sign-in wins"
        );
        assert_eq!(m.state.refresh[&hash].opened_at, 40);

        // An exchange that states no opening (an earlier release's) keeps
        // the session's.
        let (_next, next_hash, next_record) = refresh_token("alice", &cred, "s1");
        let signed = crate::proof::sign_refresh_use(
            &token,
            &crate::proof::refresh_use_message(
                &hash,
                &next_hash,
                &next_record.key,
                "s1",
                next_record.expires_at,
                5,
            ),
        );
        let _ok = m
            .run(SystemCmd::UseRefresh {
                old_hash: hash,
                new_hash: next_hash.clone(),
                record: next_record,
                now: 5,
                signature: signed,
            })
            .unwrap();
        assert_eq!(m.state.refresh[&next_hash].opened_at, 40);
        assert_eq!(
            m.state.accounts["alice"].passkey_times[&cred].last_sign_in_at,
            Some(50),
            "a refresh is no sign-in"
        );

        // A sign-up on this release: added and signed in at its `at`, and
        // its session opened then.
        let mut dave = Soft::new("localhost", "http://localhost:3000");
        let SystemCmd::CreateAccount { account, .. } = account_with("dave", &mut dave) else {
            unreachable!()
        };
        let (_t, dave_hash, record) = refresh_token("dave", &dave.credential_id(), "d1");
        let grant = super::RefreshGrant {
            hash: dave_hash.clone(),
            record,
        };
        let args = super::account_args(&account, Some(&grant));
        let _ok = m
            .run(SystemCmd::CreateAccount {
                consent: consent(&mut dave, "account:dave", "__create_account", &args),
                account,
                invite: None,
                refresh: Some(grant),
                at: 70,
            })
            .unwrap();
        assert_eq!(
            m.state.accounts["dave"].passkey_times[&dave.credential_id()],
            crate::types::PasskeyTimes {
                added_at: Some(70),
                last_sign_in_at: Some(70),
            }
        );
        assert_eq!(m.state.refresh[&dave_hash].opened_at, 70);
    }

    /// An account and a refresh record as an earlier release wrote them read
    /// with their times unknown and write back byte for byte, and so does
    /// the account a sign-up's passkey signs (`account_args`), so a replica
    /// on either release checks the same consent.
    #[test]
    fn accounts_and_sessions_from_before_the_times_read_and_write_as_they_did() {
        let account = json!({
            "accountId": "alice", "name": "alice", "createdAt": 3,
            "passkeys": [{ "id": "c1", "public_key": { "alg": "Es256", "key": "pk" }, "sign_count": 0 }],
            "approvedReleases": ["1.0.0"], "outboundApprovals": {},
            "passkeyLabels": { "c1": "laptop" }, "aliases": {},
        });
        let read: crate::types::Account = serde_json::from_value(account.clone()).unwrap();
        assert!(read.passkey_times.is_empty());
        assert_eq!(serde_json::to_value(&read).unwrap(), account);
        let record = json!({
            "account": "alice", "session": "s1", "expiresAt": 99, "passkey": "c1", "key": "ab",
        });
        let read: crate::types::RefreshRecord = serde_json::from_value(record.clone()).unwrap();
        assert_eq!(read.opened_at, 0);
        assert_eq!(serde_json::to_value(&read).unwrap(), record);
        // An entry an earlier release logged replays as the same command.
        let entry = json!({ "op": "addPasskey", "account": "alice",
            "credential": { "id": "c2", "public_key": { "alg": "Es256", "key": "pk" }, "sign_count": 0 }, "consent": null });
        let cmd: SystemCmd = serde_json::from_value(entry.clone()).unwrap();
        assert!(matches!(cmd, SystemCmd::AddPasskey { at: 0, .. }));
        assert_eq!(serde_json::to_value(&cmd).unwrap(), entry);
    }

    /// An account's activity comes from the entries' own times: a sign-in's
    /// session opening (with the app and origin it is bound to) and a
    /// step-up's consent time, under the passkey that gave it. A repeated
    /// sign-in moves the newest entry's time; a refused command, a consent
    /// with no time (an earlier release's) and an earlier release's sign-in
    /// add nothing; the oldest entries go past the cap; nobody else's
    /// activity moves.
    #[test]
    fn activity_records_sign_ins_and_step_ups_from_their_times() {
        use crate::types::{Activity, ActivityEntry, ACTIVITY_KEPT};
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        let cred = alice.credential_id();
        assert!(m
            .state
            .activity
            .get("alice")
            .cloned()
            .unwrap_or_default()
            .is_empty());
        let mut session = 0;
        let mut sign_in =
            |m: &mut SystemMachine, alice: &mut Soft, opened_at: u64, app: Option<&str>| {
                session += 1;
                let (_token, hash, record) = refresh_token("alice", &cred, &format!("s{session}"));
                let record = crate::types::RefreshRecord {
                    opened_at,
                    app: app.map(str::to_owned),
                    origin: app.map(|a| format!("https://{a}.example.com")),
                    ..record
                };
                let c = consent(
                    alice,
                    "session",
                    "__sign_in",
                    &super::sign_in_args(&hash, &record),
                );
                let _ok = m
                    .run(SystemCmd::CreateRefresh {
                        hash,
                        record,
                        consent: c,
                    })
                    .unwrap();
            };
        let timed = |c: serde_json::Value, at: u64| {
            let mut c = c;
            c["at"] = json!(at);
            c
        };
        let signed_in = |at: u64, app: Option<&str>| ActivityEntry {
            at,
            passkey: cred.clone(),
            what: Activity::SignIn {
                app: app.map(str::to_owned),
                origin: app.map(|a| format!("https://{a}.example.com")),
            },
        };

        sign_in(&mut m, &mut alice, 0, None);
        assert!(
            m.state
                .activity
                .get("alice")
                .cloned()
                .unwrap_or_default()
                .is_empty(),
            "an earlier release's sign-in adds nothing"
        );
        sign_in(&mut m, &mut alice, 40, Some("com.notes"));
        sign_in(&mut m, &mut alice, 45, Some("com.notes"));
        assert_eq!(
            m.state.activity.get("alice").cloned().unwrap_or_default(),
            [signed_in(45, Some("com.notes"))],
            "a repeated sign-in moves the newest entry's time"
        );

        // A step-up: its method and subject, never its arguments.
        let args = json!({ "version": "2.0.0" });
        let wrong = timed(
            consent(&mut alice, "account:bob", "__approve_release", &args),
            50,
        );
        assert!(m.run(approve_with("alice", "2.0.0", wrong)).is_err());
        let real = timed(
            consent(&mut alice, "account:alice", "__approve_release", &args),
            50,
        );
        let _ok = m.run(approve_with("alice", "2.0.0", real)).unwrap();
        let approved = ActivityEntry {
            at: 50,
            passkey: cred.clone(),
            what: Activity::Approval {
                method: "__approve_release".into(),
                subject: "account:alice".into(),
            },
        };
        assert_eq!(
            m.state.activity.get("alice").cloned().unwrap_or_default(),
            [signed_in(45, Some("com.notes")), approved.clone()],
            "a refused command adds nothing"
        );
        // A consent with no time (an earlier release's) adds nothing.
        let untimed = consent(&mut alice, "account:alice", "__withdraw_release", &args);
        let _ok = m
            .run(SystemCmd::SetReleaseApproval {
                scope: ReleaseScope::Account,
                version: "2.0.0".into(),
                approved: false,
                actor: "alice".into(),
                consent: untimed,
            })
            .unwrap();
        assert_eq!(
            m.state
                .activity
                .get("alice")
                .cloned()
                .unwrap_or_default()
                .len(),
            2
        );
        // Stored as the route lists it, and left out when empty.
        assert_eq!(
            serde_json::to_value(&approved).unwrap(),
            json!({ "at": 50, "passkey": cred, "kind": "approval",
                "method": "__approve_release", "subject": "account:alice" })
        );
        assert!(serde_json::to_value(SystemState::default())
            .unwrap()
            .get("activity")
            .is_none());

        // Past the cap the oldest go first.
        for i in 0..ACTIVITY_KEPT {
            let app = if i % 2 == 0 { "com.a" } else { "com.b" };
            sign_in(&mut m, &mut alice, 100 + i as u64, Some(app));
        }
        let kept = &m.state.activity.get("alice").cloned().unwrap_or_default();
        assert_eq!(kept.len(), ACTIVITY_KEPT);
        assert!(!kept.contains(&approved), "the oldest went first");
        assert_eq!(kept.back().unwrap().at, 100 + ACTIVITY_KEPT as u64 - 1);
        assert!(!m.state.activity.contains_key("bob"));
        assert!(!m.state.activity.contains_key("carol"));
    }

    /// An account is created only by its own new passkey, over the whole
    /// account and the session its sign-up opens; never with a refresh token
    /// for another account.
    #[test]
    fn an_account_is_created_by_its_own_passkey() {
        let (mut m, _alice, mut bob, _carol) = proven_cluster();
        let mut dave = Soft::new("localhost", "http://localhost:3000");
        let SystemCmd::CreateAccount { account, .. } = account_with("dave", &mut dave) else {
            unreachable!()
        };
        let (_token, hash, record) = refresh_token("dave", &dave.credential_id(), "s1");
        let grant = super::RefreshGrant { hash, record };
        let create = |refresh: Option<super::RefreshGrant>, consent| SystemCmd::CreateAccount {
            at: 0,
            account: account.clone(),
            invite: None,
            refresh,
            consent,
        };
        let args = super::account_args(&account, Some(&grant));
        let mut elsewhere = account.clone();
        elsewhere.approved_releases = ["9.9.9".to_owned()].into_iter().collect();
        let (_t, h, alices) = refresh_token("alice", &dave.credential_id(), "s1");
        let for_alice = super::RefreshGrant {
            hash: h,
            record: alices,
        };
        for (why, cmd) in [
            (
                "no consent",
                create(Some(grant.clone()), serde_json::Value::Null),
            ),
            (
                "another passkey's",
                create(
                    Some(grant.clone()),
                    consent(&mut bob, "account:dave", "__create_account", &args),
                ),
            ),
            (
                "over another release",
                create(
                    Some(grant.clone()),
                    consent(
                        &mut dave,
                        "account:dave",
                        "__create_account",
                        &super::account_args(&elsewhere, Some(&grant)),
                    ),
                ),
            ),
            (
                "a refresh token for alice",
                create(
                    Some(for_alice.clone()),
                    consent(
                        &mut dave,
                        "account:dave",
                        "__create_account",
                        &super::account_args(&account, Some(&for_alice)),
                    ),
                ),
            ),
        ] {
            assert!(m.run(cmd).is_err(), "{why}");
        }
        assert!(!m.state.accounts.contains_key("dave"));
        let real = consent(&mut dave, "account:dave", "__create_account", &args);
        let _ok = m.run(create(Some(grant.clone()), real)).unwrap();
        assert!(m.state.accounts.contains_key("dave"));
        assert_eq!(m.state.refresh[&grant.hash].account, "dave");
    }

    /// An API key is revoked only with its account's passkey consent over
    /// that key, once.
    #[test]
    fn an_api_key_is_revoked_with_its_accounts_passkey() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
            "keyId": "k1", "account": "alice", "label": "ci", "apps": null,
            "methods": null, "readOnly": false, "allowConfirm": false, "expiresAt": 99,
        }))
        .unwrap();
        let _prev = m.state.api_keys.insert("h1".into(), record.clone());
        let _prev = m.state.api_keys.insert(
            "h2".into(),
            crate::types::ApiKeyRecord {
                key_id: "k2".into(),
                ..record
            },
        );
        let revoke = |consent| SystemCmd::RevokeApiKey {
            key_id: "k1".into(),
            actor: "alice".into(),
            consent,
        };
        let args = json!({ "keyId": "k1" });
        let c = consent(&mut bob, "account:alice", "__revoke_api_key", &args);
        assert!(m.run(revoke(c)).is_err(), "bob's passkey");
        needs_consent(
            &mut m,
            &mut alice,
            ("account:alice", "__revoke_api_key"),
            &args,
            &json!({ "keyId": "k2" }),
            revoke,
        );
        assert!(!m.state.api_keys.contains_key("h1"));
        assert!(m.state.api_keys.contains_key("h2"));
    }

    /// A passkey's counter moves only to what an assertion of that passkey
    /// carries.
    #[test]
    fn a_counter_moves_with_the_assertion_that_moved_it() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        let id = alice.credential_id();
        let set = |count, assertion: &calimero_identity::Assertion| SystemCmd::SetSignCount {
            credential_id: id.clone(),
            count,
            assertion: serde_json::to_value(assertion).unwrap(),
        };
        let theirs = calimero_identity::Assertion {
            credential_id: id.clone(),
            ..bob.assert(&[1; 32])
        };
        let a = alice.assert(&[1; 32]);
        let carried = calimero_identity::assertion_counter(
            &m.state.account_of_credential(&id).unwrap().1.clone(),
            &a,
        )
        .unwrap();
        assert!(m
            .run(SystemCmd::SetSignCount {
                credential_id: id.clone(),
                count: u32::MAX,
                assertion: serde_json::Value::Null,
            })
            .is_err());
        assert!(m.run(set(u32::MAX, &a)).is_err(), "past what it carries");
        assert!(m.run(set(carried, &theirs)).is_err(), "bob's signature");
        let _ok = m.run(set(carried, &a)).unwrap();
        assert_eq!(m.state.accounts["alice"].passkeys[0].sign_count, carried);
        assert!(m.run(set(carried, &a)).is_err(), "replayed");
    }

    /// Membership changes apply only with the actor's passkey consent over
    /// exactly that change, once; a member leaving consents too.
    #[test]
    fn memberships_need_the_actors_passkey() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        needs_consent(
            &mut m,
            &mut alice,
            ("org:org", "__set_org_role"),
            &json!({ "account": "bob", "role": "admin" }),
            &json!({ "account": "bob", "role": "member" }),
            |consent| SystemCmd::SetOrgRole {
                org: "org".into(),
                account: "bob".into(),
                role: OrgRole::Admin,
                actor: "alice".into(),
                consent,
            },
        );
        assert_eq!(m.state.orgs["org"].members["bob"], OrgRole::Admin);
        needs_consent(
            &mut m,
            &mut alice,
            ("org:org", "__set_team_member"),
            &json!({ "team": "t", "account": "bob", "lead": false, "role": null, "capabilities": null }),
            &json!({ "team": "t", "account": "bob", "lead": true, "role": null, "capabilities": null }),
            |consent| SystemCmd::SetTeamMember {
                org: "org".into(),
                team: "t".into(),
                account: "bob".into(),
                lead: false,
                role: None,
                capabilities: None,
                actor: "alice".into(),
                consent,
            },
        );
        assert!(m.state.orgs["org"].in_team("bob", "t"));
        needs_consent(
            &mut m,
            &mut alice,
            ("org:org", "__set_capabilities"),
            &json!({ "team": "t", "account": "bob", "capabilities": 3 }),
            &json!({ "team": null, "account": "bob", "capabilities": 3 }),
            |consent| SystemCmd::SetCapabilities {
                org: "org".into(),
                team: Some("t".into()),
                account: "bob".into(),
                capabilities: 3,
                actor: "alice".into(),
                consent,
            },
        );
        needs_consent(
            &mut m,
            &mut alice,
            ("org:org", "__set_default_capabilities"),
            &json!({ "capabilities": 5 }),
            &json!({ "capabilities": 7 }),
            |consent| SystemCmd::SetDefaultCapabilities {
                org: "org".into(),
                team: None,
                capabilities: 5,
                actor: "alice".into(),
                consent,
            },
        );
        assert_eq!(m.state.orgs["org"].default_capabilities, 5);
        needs_consent(
            &mut m,
            &mut alice,
            ("org:org", "__remove_team_member"),
            &json!({ "team": "t", "account": "bob" }),
            &json!({ "team": "t", "account": "alice" }),
            |consent| SystemCmd::RemoveTeamMember {
                org: "org".into(),
                team: "t".into(),
                account: "bob".into(),
                actor: "alice".into(),
                consent,
                subtree: true,
            },
        );
        // Leaving takes the leaver's own consent, not the owner's.
        let leave = |consent| SystemCmd::RemoveOrgMember {
            org: "org".into(),
            account: "bob".into(),
            actor: "bob".into(),
            consent,
        };
        let args = json!({ "account": "bob" });
        let c = consent(&mut alice, "org:org", "__remove_member", &args);
        assert!(m.run(leave(c)).is_err());
        needs_consent(
            &mut m,
            &mut bob,
            ("org:org", "__remove_member"),
            &args,
            &json!({ "account": "alice" }),
            leave,
        );
        assert!(!m.state.orgs["org"].members.contains_key("bob"));
    }

    /// An invite is minted with the actor's consent over its key and terms,
    /// and redeemed only with a signature by the token's key, for the
    /// account it names, once: a node that saw only the log cannot.
    #[test]
    fn invites_are_redeemed_with_the_tokens_key() {
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        let token = "inv_secret";
        let key = crate::proof::invite_key(token);
        let invite: crate::types::Invite = serde_json::from_value(json!({
            "org": "org", "role": "member", "teams": [], "expiresAt": 100, "usesLeft": 2,
        }))
        .unwrap();
        let args = super::invite_args(&key, &invite);
        let mut admin = args.clone();
        admin["role"] = json!("admin");
        needs_consent(
            &mut m,
            &mut alice,
            ("org:org", "__create_invite"),
            &args,
            &admin,
            |consent| SystemCmd::CreateInvite {
                invite_key: key.clone(),
                invite: invite.clone(),
                actor: "alice".into(),
                consent,
                teams_seen: true,
            },
        );
        let redeem = |signature: String| SystemCmd::RedeemInvite {
            invite_key: key.clone(),
            account: "carol".into(),
            now: 10,
            signature,
            free_when_unchanged: true,
        };
        assert!(m.run(redeem(String::new())).is_err(), "no signature");
        let forged = crate::proof::sign_invite_redeem("inv_guess", "carol", 10);
        assert!(m.run(redeem(forged)).is_err(), "another token's key");
        let for_bob = crate::proof::sign_invite_redeem(token, "bob", 10);
        assert!(m.run(redeem(for_bob)).is_err(), "another account");
        let earlier = crate::proof::sign_invite_redeem(token, "carol", 9);
        assert!(m.run(redeem(earlier)).is_err(), "another time");
        assert!(!m.state.orgs["org"].members.contains_key("carol"));
        let real = crate::proof::sign_invite_redeem(token, "carol", 10);
        let _ok = m.run(redeem(real.clone())).unwrap();
        assert!(m.state.orgs["org"].members.contains_key("carol"));
        assert!(m.run(redeem(real)).is_err(), "replay");
        assert_eq!(m.state.invites[&key].uses_left, 1);
    }

    /// An invite of `org` kept under `token`'s key: `role`, `teams`, `uses`.
    fn keep_invite(m: &mut SystemMachine, token: &str, role: &str, teams: &[&str], uses: u32) {
        let invite: crate::types::Invite = serde_json::from_value(json!({
            "org": "org", "role": role, "teams": teams, "expiresAt": 100, "usesLeft": uses,
        }))
        .unwrap();
        let _prev = m
            .state
            .invites
            .insert(crate::proof::invite_key(token), invite);
    }

    /// `account`'s redemption of `token` at `now`, as this release's API logs it.
    fn redemption(token: &str, account: &str, now: u64) -> SystemCmd {
        SystemCmd::RedeemInvite {
            invite_key: crate::proof::invite_key(token),
            account: account.into(),
            now,
            signature: crate::proof::sign_invite_redeem(token, account, now),
            free_when_unchanged: true,
        }
    }

    /// The owner opening their own one-use link changes nothing for them, so
    /// it spends nothing: the person it was made for still joins with it, and
    /// only that redemption removes it. Before, the owner's spent the only
    /// use and the invitee was told the invite was unknown.
    #[test]
    fn a_member_redeeming_an_invite_leaves_its_use_for_the_invitee() {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        keep_invite(&mut m, "inv_link", "member", &[], 1);
        let key = crate::proof::invite_key("inv_link");
        let out = m.run(redemption("inv_link", "alice", 10)).unwrap();
        assert_eq!(out, json!({ "org": "org" }));
        assert_eq!(m.state.orgs["org"].members["alice"], OrgRole::Owner);
        assert_eq!(m.state.invites[&key].uses_left, 1, "the owner spent none");
        // A member at the invite's role spends none either.
        let _same = m.run(redemption("inv_link", "bob", 11)).unwrap();
        assert_eq!(m.state.invites[&key].uses_left, 1, "bob spent none");
        let out = m.run(redemption("inv_link", "carol", 12)).unwrap();
        assert_eq!(out, json!({ "org": "org" }));
        assert_eq!(m.state.orgs["org"].members["carol"], OrgRole::Member);
        assert!(
            !m.state.invites.contains_key(&key),
            "the invitee used it up"
        );
        let e = m.run(redemption("inv_link", "carol", 13)).unwrap_err();
        assert_eq!(e, INVITE_GONE);
    }

    /// A member spends a use when the invite gives them something: a higher
    /// role, or a team they are not in yet.
    #[test]
    fn a_member_spends_a_use_when_the_invite_raises_or_adds() {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        keep_invite(&mut m, "inv_admin", "admin", &[], 2);
        let _ok = m.run(redemption("inv_admin", "bob", 10)).unwrap();
        assert_eq!(m.state.orgs["org"].members["bob"], OrgRole::Admin);
        let admin = crate::proof::invite_key("inv_admin");
        assert_eq!(m.state.invites[&admin].uses_left, 1, "a raised role");
        keep_invite(&mut m, "inv_team", "member", &["t"], 2);
        let _ok = m.run(redemption("inv_team", "bob", 11)).unwrap();
        assert!(m.state.orgs["org"].teams["t"].members.contains_key("bob"));
        let team = crate::proof::invite_key("inv_team");
        assert_eq!(m.state.invites[&team].uses_left, 1, "an added team");
        // In the team now, and an admin: the same invite gives bob nothing.
        let _same = m.run(redemption("inv_team", "bob", 12)).unwrap();
        assert_eq!(m.state.invites[&team].uses_left, 1, "nothing new");
    }

    /// A redemption an earlier release logged (no `free_when_unchanged`)
    /// replays as that release applied it: the owner's spends the only use
    /// and removes the invite, so the invitee's later redemption is refused
    /// on a replica that replays the log after a restart as on one that
    /// applied it then.
    #[test]
    fn a_redemption_logged_before_free_redemptions_replays_as_it_did() {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        keep_invite(&mut m, "inv_link", "member", &[], 1);
        let key = crate::proof::invite_key("inv_link");
        // The entry as rc.10 logged it: no flag.
        let logged = json!({
            "op": "redeemInvite",
            "invite_key": &key,
            "account": "alice",
            "now": 10,
            "signature": crate::proof::sign_invite_redeem("inv_link", "alice", 10),
        });
        let old: SystemCmd = serde_json::from_value(logged).unwrap();
        let out = m.run(old).unwrap();
        assert_eq!(out, json!({ "org": "org" }));
        assert!(!m.state.invites.contains_key(&key), "spent, as it was");
        let e = m.run(redemption("inv_link", "carol", 11)).unwrap_err();
        assert_eq!(e, INVITE_GONE);
        assert!(!m.state.orgs["org"].members.contains_key("carol"));
        // This release logs the flag, so its entries replay the new rule.
        let new = serde_json::to_value(redemption("inv_link", "alice", 10)).unwrap();
        assert_eq!(new["free_when_unchanged"], json!(true), "{new}");
    }

    // ------------------------------------------- teams and leaving members

    use super::DELETE_TEAM_NEEDS_CONSENT;
    use crate::types::{AppRecord, Audience};

    /// `proven_cluster` plus a restricted team `c` under `t`, bob with a
    /// direct row in both, and an app `inapp` on `c`.
    fn nested_teams() -> (SystemMachine, Soft, Soft) {
        let (mut m, alice, bob, _carol) = proven_cluster();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.teams.insert(
            "c".into(),
            serde_json::from_value(json!({
                "teamId": "c", "name": "C", "parent": "t", "join": "invite", "members": {}
            }))
            .unwrap(),
        );
        for team in ["t", "c"] {
            let _prev = o
                .teams
                .get_mut(team)
                .unwrap()
                .members
                .insert("bob".into(), false);
        }
        let _prev = m.state.apps.insert(
            "inapp".into(),
            serde_json::from_value(json!({
                "appId": "inapp", "name": "inapp", "group": 3, "bundle": "b1",
                "owner": "alice", "org": "org", "access": { "kind": "team", "team": "c" },
                "replicas": [1, 2, 3], "createdAt": 0
            }))
            .unwrap(),
        );
        (m, alice, bob)
    }

    /// A removal from a team takes the direct rows below it too.
    #[test]
    fn a_team_removal_takes_the_rows_below_it() {
        let (mut m, mut alice, _bob) = nested_teams();
        let c = consent(
            &mut alice,
            "org:org",
            "__remove_team_member",
            &json!({ "team": "t", "account": "bob" }),
        );
        let _ok = m
            .run(SystemCmd::RemoveTeamMember {
                org: "org".into(),
                team: "t".into(),
                account: "bob".into(),
                actor: "alice".into(),
                consent: c,
                subtree: true,
            })
            .unwrap();
        let o = &m.state.orgs["org"];
        assert!(!o.in_team("bob", "t") && !o.in_team("bob", "c"));
        assert!(!m.state.may_reach(&m.state.apps["inapp"], Some("bob")));
    }

    /// Deleting a team that holds apps needs the deleter's passkey over
    /// exactly those apps; one without apps goes without it.
    #[test]
    fn deleting_a_team_with_apps_needs_consent_over_them() {
        let (mut m, mut alice, _bob) = nested_teams();
        let delete = |team: &str, consent| SystemCmd::DeleteTeam {
            org: "org".into(),
            team: team.into(),
            actor: "alice".into(),
            consent,
            apps_need_consent: true,
        };
        assert_eq!(
            m.run(delete("t", serde_json::Value::Null)).unwrap_err(),
            DELETE_TEAM_NEEDS_CONSENT
        );
        let fewer = consent(
            &mut alice,
            "org:org",
            "__delete_team",
            &json!({ "team": "t", "apps": [] }),
        );
        assert!(m.run(delete("t", fewer)).is_err(), "another list of apps");
        let real = consent(
            &mut alice,
            "org:org",
            "__delete_team",
            &json!({ "team": "t", "apps": ["inapp"] }),
        );
        let out = m.run(delete("t", real)).unwrap();
        assert_eq!(out["apps"], json!(["inapp"]));
        assert!(!m.state.apps.contains_key("inapp"));

        let (mut m, _alice, _bob) = nested_teams();
        let _gone = m.state.apps.remove("inapp");
        let out = m.run(delete("c", serde_json::Value::Null)).unwrap();
        assert_eq!(out["apps"], json!([]));
    }

    /// Below admin, an invite names only teams the inviter sees, whatever
    /// INVITE_MEMBERS they hold organization-wide.
    #[test]
    fn an_invite_names_only_teams_the_inviter_sees() {
        let (mut m, _alice, mut bob) = nested_teams();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.teams.insert(
            "x".into(),
            serde_json::from_value(json!({
                "teamId": "x", "name": "X", "join": "invite", "members": {}
            }))
            .unwrap(),
        );
        let _prev = o
            .capabilities
            .insert("bob".into(), crate::types::caps::INVITE_MEMBERS);
        let mint = |m: &mut SystemMachine, bob: &mut Soft, token: &str, team: &str| {
            let key = crate::proof::invite_key(token);
            let invite: crate::types::Invite = serde_json::from_value(json!({
                "org": "org", "role": "member", "teams": [team], "expiresAt": 100, "usesLeft": 1,
            }))
            .unwrap();
            let c = consent(
                bob,
                "org:org",
                "__create_invite",
                &super::invite_args(&key, &invite),
            );
            m.run(SystemCmd::CreateInvite {
                invite_key: key,
                invite,
                actor: "bob".into(),
                consent: c,
                teams_seen: true,
            })
        };
        assert!(
            mint(&mut m, &mut bob, "inv_t", "t").is_ok(),
            "a team he is in"
        );
        assert_eq!(
            mint(&mut m, &mut bob, "inv_x", "x").unwrap_err(),
            "an invite names only teams the inviter sees"
        );
    }

    /// Commands an earlier release logged, without this release's flags,
    /// replay as that release applied them: a team removal takes one row, an
    /// invite skips the sight check, a team deletion needs no passkey. The
    /// entries this release logs carry the flags.
    #[test]
    fn team_commands_logged_before_these_rules_replay_as_they_did() {
        let (mut m, mut alice, mut bob) = nested_teams();
        let c = consent(
            &mut alice,
            "org:org",
            "__remove_team_member",
            &json!({ "team": "t", "account": "bob" }),
        );
        let old: SystemCmd = serde_json::from_value(json!({
            "op": "removeTeamMember", "org": "org", "team": "t", "account": "bob",
            "actor": "alice", "consent": c,
        }))
        .unwrap();
        let _ok = m.run(old).unwrap();
        assert!(
            m.state.orgs["org"].in_team("bob", "c"),
            "the row below stays"
        );

        let _prev = m
            .state
            .orgs
            .get_mut("org")
            .unwrap()
            .capabilities
            .insert("bob".into(), crate::types::caps::INVITE_MEMBERS);
        let key = crate::proof::invite_key("inv_old");
        let invite: crate::types::Invite = serde_json::from_value(json!({
            "org": "org", "role": "member", "teams": ["t"], "expiresAt": 100, "usesLeft": 1,
        }))
        .unwrap();
        let c = consent(
            &mut bob,
            "org:org",
            "__create_invite",
            &super::invite_args(&key, &invite),
        );
        let old: SystemCmd = serde_json::from_value(json!({
            "op": "createInvite", "invite_key": key, "invite": invite, "actor": "bob",
            "consent": c,
        }))
        .unwrap();
        let _ok = m.run(old).unwrap();
        assert!(m.state.invites.contains_key(&key), "minted as it was");

        let old: SystemCmd = serde_json::from_value(json!({
            "op": "deleteTeam", "org": "org", "team": "t", "actor": "alice",
        }))
        .unwrap();
        let out = m.run(old).unwrap();
        assert_eq!(out["apps"], json!(["inapp"]), "deleted without a passkey");

        let new = |cmd: SystemCmd| serde_json::to_value(cmd).unwrap();
        let removal = new(SystemCmd::RemoveTeamMember {
            org: "org".into(),
            team: "t".into(),
            account: "bob".into(),
            actor: "alice".into(),
            consent: serde_json::Value::Null,
            subtree: true,
        });
        assert_eq!(removal["subtree"], json!(true), "{removal}");
        let deletion = new(SystemCmd::DeleteTeam {
            org: "org".into(),
            team: "t".into(),
            actor: "alice".into(),
            consent: serde_json::Value::Null,
            apps_need_consent: true,
        });
        assert_eq!(deletion["apps_need_consent"], json!(true), "{deletion}");
    }

    /// An account an organization's app names (its owner, an `Accounts`
    /// list) reaches it, and receives its events, only while a member.
    #[test]
    fn an_org_app_is_reached_only_by_current_members() {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        let listed: AppRecord = serde_json::from_value(json!({
            "appId": "dm", "name": "dm", "group": 4, "bundle": "b1", "owner": "bob",
            "org": "org", "access": { "kind": "accounts", "accounts": ["alice", "bob", "carol"] },
            "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap();
        let s = &m.state;
        assert!(s.may_reach(&listed, Some("alice")) && s.may_reach(&listed, Some("bob")));
        assert!(
            !s.may_reach(&listed, Some("carol")),
            "listed, never a member"
        );
        let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
        let s = &m.state;
        assert!(!s.may_reach(&listed, Some("bob")), "listed owner, removed");
        assert!(!s.receives(&listed, &Audience::All, Some("alice"), Some("bob")));
        assert!(s.receives(&listed, &Audience::All, Some("bob"), Some("alice")));
        // Outside an organization, the list alone decides.
        let personal = AppRecord {
            org: None,
            ..listed
        };
        assert!(m.state.may_reach(&personal, Some("carol")));
    }

    /// A member who writes a team whose default capabilities hold
    /// `CREATE_CHILD_TEAM` (an app team's) creates a restricted team below
    /// it and leads it; never an open one, never from a read-only row,
    /// without the capability, outside the team, or in an entry an earlier
    /// release logged. Removed from the parent, they leave the child too.
    #[test]
    fn a_member_of_a_team_creates_a_private_team_below_it() {
        use crate::types::{caps, TeamJoin};

        let (mut m, _alice, _bob, _carol) = proven_cluster();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.members.insert("carol".into(), OrgRole::Member);
        let t = o.teams.get_mut("t").unwrap();
        let _prev = t.members.insert("bob".into(), false);
        t.default_capabilities = Some(caps::CREATE_CHILD_TEAM);
        let create = |id: &str, who: &str, join: TeamJoin, flag: bool| SystemCmd::CreateTeam {
            org: "org".into(),
            team_id: id.into(),
            name: id.into(),
            parent: Some("t".into()),
            join,
            actor: who.into(),
            member_child_teams: flag,
        };
        let refused = "not allowed to create this team";
        assert_eq!(
            m.run_authorized(create("old", "bob", TeamJoin::Invite, false)),
            Err(refused),
            "as logged before"
        );
        assert_eq!(
            m.run_authorized(create("open", "bob", TeamJoin::Everyone, true)),
            Err(refused)
        );
        assert_eq!(
            m.run_authorized(create("c", "carol", TeamJoin::Invite, true)),
            Err(refused),
            "not in the team"
        );
        let _ok = m
            .run_authorized(create("dm", "bob", TeamJoin::Invite, true))
            .unwrap();
        let o = &m.state.orgs["org"];
        assert_eq!(o.teams["dm"].parent.as_deref(), Some("t"));
        assert!(o.manages_team("bob", "dm") && !o.manages_team("bob", "t"));

        // Read-only, or without the capability: no.
        let t = m
            .state
            .orgs
            .get_mut("org")
            .unwrap()
            .teams
            .get_mut("t")
            .unwrap();
        let _new = t.read_only.insert("bob".into());
        assert_eq!(
            m.run_authorized(create("ro", "bob", TeamJoin::Invite, true)),
            Err(refused)
        );
        let t = m
            .state
            .orgs
            .get_mut("org")
            .unwrap()
            .teams
            .get_mut("t")
            .unwrap();
        let _was = t.read_only.remove("bob");
        t.default_capabilities = Some(caps::INVITE_MEMBERS);
        assert_eq!(
            m.run_authorized(create("nocap", "bob", TeamJoin::Invite, true)),
            Err(refused)
        );

        // Removed from the parent, Bob is out of the team he made below it.
        let _ok = m
            .run_authorized(SystemCmd::RemoveTeamMember {
                org: "org".into(),
                team: "t".into(),
                account: "bob".into(),
                actor: "alice".into(),
                consent: Value::Null,
                subtree: true,
            })
            .unwrap();
        let o = &m.state.orgs["org"];
        assert!(!o.in_team("bob", "dm") && !o.manages_team("bob", "dm"));
    }

    // ------------------------------------- an org app's owner, while a member

    use std::collections::{BTreeMap, BTreeSet};

    use super::OwnerRule;
    use crate::types::{Access, Team, TeamJoin};

    /// `proven_cluster` with carol a member too and an app `oa` of the
    /// organization that bob owns.
    fn org_app_of_bob() -> (SystemMachine, Soft, Soft) {
        let (mut m, alice, bob, _carol) = proven_cluster();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.members.insert("carol".into(), OrgRole::Member);
        let _prev = m.state.apps.insert(
            "oa".into(),
            serde_json::from_value(json!({
                "appId": "oa", "name": "oa", "group": 5, "bundle": "b1", "owner": "bob",
                "org": "org", "access": { "kind": "owner" }, "replicas": [1, 2, 3], "createdAt": 0
            }))
            .unwrap(),
        );
        (m, alice, bob)
    }

    fn set_access(app: &str, actor: &str) -> SystemCmd {
        SystemCmd::SetAccess {
            app_id: app.into(),
            access: Access::Public,
            actor: actor.into(),
            owner_in_org: true,
            owner_in_team: true,
        }
    }

    fn set_app_metadata(app: &str, actor: &str) -> SystemCmd {
        SystemCmd::SetAppMetadata {
            app_id: app.into(),
            metadata: json!({ "pinned": true }),
            actor: actor.into(),
            owner_in_org: true,
            owner_in_team: true,
        }
    }

    fn delete_app(app: &str, actor: &str) -> SystemCmd {
        SystemCmd::DeleteApp {
            app_id: app.into(),
            actor: actor.into(),
            owner_in_org: true,
            owner_in_team: true,
        }
    }

    fn transfer_app(pk: &mut Soft, app: &str, actor: &str, to: &str) -> SystemCmd {
        SystemCmd::TransferApp {
            app_id: app.into(),
            to: to.into(),
            actor: actor.into(),
            consent: consent(pk, app, "__transfer", &json!({ "to": to })),
            owner_in_org: true,
            owner_in_team: true,
        }
    }

    // ------------------------------------------ moving an app to another organization

    /// `proven_cluster` with a package `p` (bundle `b1`), carol a guest of
    /// `org`, the app team `pt` of `p` there (closed) with bob and carol in
    /// it, an app `oa` of `p` that bob owns on it, and a second organization
    /// `new` that alice owns, holding an open app team of another package
    /// (so it is standalone).
    fn moving() -> (SystemMachine, Soft, Soft) {
        let (mut m, alice, bob, _carol) = proven_cluster();
        let s = &mut m.state;
        let _prev = s.packages.insert(
            "p".into(),
            serde_json::from_value(json!({
                "name": "p", "signerId": "did:key:zp", "owner": "alice",
                "versions": { "1.0.0": {
                    "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                    "services": { "": "b1" }, "metadata": { "name": "Pad" },
                    "installedBy": "alice", "installedAt": 0
                } }
            }))
            .unwrap(),
        );
        let _prev = s.bundles.insert(
            "b1".into(),
            serde_json::from_value(json!({
                "hash": "b1", "size": 1, "uploadedBy": "alice",
                "package": { "package": "p", "version": "1.0.0" }
            }))
            .unwrap(),
        );
        let o = s.orgs.get_mut("org").unwrap();
        let _prev = o.members.insert("carol".into(), OrgRole::Guest);
        let _prev = o.teams.insert(
            "pt".into(),
            serde_json::from_value(json!({
                "teamId": "pt", "name": "Pad", "join": "invite",
                "members": { "bob": false, "carol": false },
                "metadata": { "app": { "package": "p", "name": "Pad" } }
            }))
            .unwrap(),
        );
        let _prev = s.orgs.insert(
            "new".into(),
            serde_json::from_value(json!({
                "orgId": "new", "name": "New", "members": { "alice": "owner" },
                "teams": { "qt": {
                    "teamId": "qt", "name": "Q", "join": "everyone", "members": {},
                    "metadata": { "app": { "package": "q", "name": "Q" } }
                } }
            }))
            .unwrap(),
        );
        let _prev = s.apps.insert(
            "oa".into(),
            serde_json::from_value(json!({
                "appId": "oa", "name": "oa", "group": 5, "bundle": "b1", "owner": "bob",
                "org": "org", "access": { "kind": "team", "team": "pt" },
                "replicas": [1, 2, 3], "createdAt": 0
            }))
            .unwrap(),
        );
        (m, alice, bob)
    }

    fn move_app(app: &str, to: &str, actor: &str, bring: bool, consent: Value) -> SystemCmd {
        SystemCmd::MoveApp {
            app_id: app.into(),
            to_org: to.into(),
            actor: actor.into(),
            consent,
            bring_members: bring,
            nonce: "n".into(),
        }
    }

    fn granted_run(payer: &str) -> std::collections::BTreeMap<String, super::RunSlot> {
        std::collections::BTreeMap::from([(
            "r".to_owned(),
            super::RunSlot {
                payer: payer.into(),
                fuel: 1,
                seq: 1,
                granted: true,
                refused: false,
                pool: String::new(),
            },
        )])
    }

    /// A move takes its manager's passkey consent over exactly its target
    /// and whether members come along, once.
    #[test]
    fn moving_an_app_needs_its_managers_consent_over_the_target() {
        let (mut m, mut alice, _bob) = moving();
        needs_consent(
            &mut m,
            &mut alice,
            ("oa", "__move"),
            &super::move_app_args("new", true),
            &super::move_app_args("new", false),
            |c| move_app("oa", "new", "alice", true, c),
        );
        assert_eq!(m.state.apps["oa"].org.as_deref(), Some("new"));
    }

    /// The app moves with its data's bill, its access and the people who
    /// reach it: the target pays its files and runs, a new open app team of
    /// its package holds it (alice, who moved it, leads it), bob comes in as
    /// a member and carol, a guest, as a guest, each with a row there, and
    /// the invites into the team it left are revoked once no app of the
    /// source is on it; an organization-wide invite stays.
    #[test]
    fn a_moved_app_takes_its_payer_access_and_people_to_the_target() {
        let (mut m, mut alice, _bob) = moving();
        m.state.set_app_file_bytes("oa", 100);
        let _prev = m.state.compute.insert("oa".into(), granted_run("org"));
        keep_invite(&mut m, "into-pad", "member", &["pt"], 1);
        keep_invite(&mut m, "org-wide", "member", &[], 1);

        let args = super::move_app_args("new", true);
        let out = m
            .run(move_app(
                "oa",
                "new",
                "alice",
                true,
                consent(&mut alice, "oa", "__move", &args),
            ))
            .unwrap();
        assert_eq!(out["created"], true, "{out}");
        assert_eq!(out["added"], json!(["bob", "carol"]), "{out}");
        assert_eq!(out["revokedInvites"], 1, "{out}");
        let s = &m.state;
        let app = &s.apps["oa"];
        assert_eq!(app.org.as_deref(), Some("new"));
        let crate::types::Access::Team { team } = &app.access else {
            panic!("{:?}", app.access);
        };
        let o = &s.orgs["new"];
        let t = &o.teams[team];
        assert_eq!(t.app_package(), Some("p"));
        assert_eq!(t.name, "Pad");
        assert_eq!(t.join, crate::types::TeamJoin::Everyone);
        assert_eq!(t.members.get("alice"), Some(&true));
        assert_eq!(t.members.get("bob"), Some(&false));
        assert_eq!(t.members.get("carol"), Some(&false));
        assert_eq!(o.role("alice"), Some(OrgRole::Owner));
        assert_eq!(o.role("bob"), Some(OrgRole::Member));
        assert_eq!(o.role("carol"), Some(OrgRole::Guest));
        for who in ["alice", "bob", "carol"] {
            assert!(s.may_reach(app, Some(who)), "{who}");
        }
        assert_eq!((s.file_bytes("org"), s.file_bytes("new")), (0, 100));
        assert_eq!(s.compute["oa"]["r"].payer, "new");
        assert!(!s
            .invites
            .contains_key(&crate::proof::invite_key("into-pad")));
        assert!(s
            .invites
            .contains_key(&crate::proof::invite_key("org-wide")));
        // Its source keeps its own team and people.
        assert!(s.orgs["org"].teams.contains_key("pt"));
        assert_eq!(s.orgs["org"].role("bob"), Some(OrgRole::Member));
    }

    /// Without `bring_members` nobody joins the target, so an app whose
    /// owner is not a member there is refused: an organization's app is owned
    /// by one of its members, and only the owner's own consented transfer
    /// changes who that is. Once the owner is a member it moves, and those
    /// outside the target stop reaching it.
    #[test]
    fn an_app_moves_without_its_people_only_to_where_its_owner_is() {
        let (mut m, _alice, _bob) = moving();
        let e = m
            .run_unproven(move_app("oa", "new", "alice", false, Value::Null))
            .unwrap_err();
        assert_eq!(e, super::MOVE_OWNER_NOT_IN_TARGET);
        let _prev = m
            .state
            .orgs
            .get_mut("new")
            .unwrap()
            .members
            .insert("bob".into(), OrgRole::Member);
        let out = m
            .run_unproven(move_app("oa", "new", "alice", false, Value::Null))
            .unwrap();
        assert_eq!(out["added"], json!([]), "{out}");
        let s = &m.state;
        assert_eq!(s.orgs["new"].role("carol"), None);
        assert!(!s.may_reach(&s.apps["oa"], Some("carol")));
        assert!(s.may_reach(&s.apps["oa"], Some("bob")));

        // Into a target whose app teams are all closed, without its people:
        // the owner still reaches (and manages) it, with a row of their own.
        let (mut m, _alice, _bob) = moving();
        let target = m.state.orgs.get_mut("new").unwrap();
        target.teams.get_mut("qt").unwrap().join = crate::types::TeamJoin::Invite;
        let _prev = target.members.insert("bob".into(), OrgRole::Member);
        let out = m
            .run_unproven(move_app("oa", "new", "alice", false, Value::Null))
            .unwrap();
        let team = out["access"]["team"].as_str().unwrap().to_owned();
        let s = &m.state;
        assert_eq!(
            s.orgs["new"].teams[&team].join,
            crate::types::TeamJoin::Invite
        );
        assert_eq!(s.orgs["new"].teams[&team].members.get("bob"), Some(&false));
        assert!(s.may_reach(&s.apps["oa"], Some("bob")));
        assert!(s.owner_acts(&s.apps["oa"], "bob"));
    }

    /// Who moves: the app's manager where it is now, as an admin of the
    /// target. A guest, an owner removed from (or who left) the source, an
    /// admin of the target demoted to member, and a member of the target
    /// who manages nothing are each refused, on the state of the moment, and
    /// the app stays where it was.
    #[test]
    fn only_the_apps_manager_as_the_targets_admin_moves_it() {
        let (mut m, _alice, _bob) = moving();
        let set_role = |m: &mut SystemMachine, org: &str, who: &str, role: Option<OrgRole>| {
            let o = m.state.orgs.get_mut(org).unwrap();
            match role {
                Some(r) => {
                    let _prev = o.members.insert(who.into(), r);
                }
                None => {
                    let _gone = o.members.remove(who);
                }
            }
        };
        let refused = |m: &mut SystemMachine, actor: &str, why: &str| {
            let e = m
                .run_unproven(move_app("oa", "new", actor, true, Value::Null))
                .unwrap_err();
            assert_eq!(e, why, "{actor}");
            assert_eq!(m.state.apps["oa"].org.as_deref(), Some("org"), "{actor}");
        };
        // Carol, a guest of the source, an admin of the target: no manager.
        set_role(&mut m, "new", "carol", Some(OrgRole::Admin));
        refused(&mut m, "carol", super::ONLY_MANAGERS_MOVE);
        // Bob owns it, but is only a member of the target.
        set_role(&mut m, "new", "bob", Some(OrgRole::Member));
        refused(&mut m, "bob", super::ONLY_ADMINS_MOVE_IN);
        // Made an admin there, then removed from the source (or gone of his
        // own accord: the same row gone): no longer its owner in effect.
        set_role(&mut m, "new", "bob", Some(OrgRole::Admin));
        set_role(&mut m, "org", "bob", None);
        refused(&mut m, "bob", super::ONLY_MANAGERS_MOVE);
        // Back as a member: he moves it.
        set_role(&mut m, "org", "bob", Some(OrgRole::Member));
        let _ok = m
            .run_unproven(move_app("oa", "new", "bob", true, Value::Null))
            .unwrap();
        assert_eq!(m.state.apps["oa"].org.as_deref(), Some("new"));
        // Moving it to where it is is refused.
        assert!(m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .is_err());
    }

    /// The target pays from the move on, so its quotas must hold what the
    /// app holds: its files, and its granted runs (count and fuel). A move
    /// past either is refused and changes nothing.
    #[test]
    fn a_move_past_the_targets_quota_is_refused() {
        let (mut m, _alice, _bob) = moving();
        m.state.quotas.max_file_bytes_per_org = 150;
        m.state.set_app_file_bytes("oa", 100);
        let _prev = m.state.apps.insert("na".into(), {
            let mut a = m.state.apps["oa"].clone();
            a.app_id = "na".into();
            a.org = Some("new".into());
            a.access = crate::types::Access::Org;
            a.owner = "alice".into();
            a
        });
        m.state.set_app_file_bytes("na", 60);
        let e = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap_err();
        assert!(e.starts_with(QUOTA_PREFIX), "{e}");
        m.state.set_app_file_bytes("na", 50);

        m.state.quotas.max_concurrent_runs_per_org = 1;
        let _prev = m.state.compute.insert("oa".into(), granted_run("org"));
        let _prev = m.state.compute.insert("na".into(), granted_run("new"));
        let e = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap_err();
        assert!(e.starts_with(QUOTA_PREFIX), "{e}");
        let s = &m.state;
        assert_eq!(s.apps["oa"].org.as_deref(), Some("org"));
        assert_eq!((s.file_bytes("org"), s.file_bytes("new")), (100, 50));
        assert_eq!(s.orgs["new"].role("bob"), None, "nobody joined");

        m.state.quotas.max_concurrent_runs_per_org = 2;
        let _ok = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap();
        assert_eq!(m.state.file_bytes("new"), 150);
    }

    /// An app on a team other than its package's app team (a team below it,
    /// or a plain team) is refused: those rows, leads and capabilities are
    /// the source's grants, which the target's admins never saw. So is a
    /// move to an organization that approves none of the releases holding
    /// the app now.
    #[test]
    fn a_move_off_a_sub_team_or_onto_other_releases_is_refused() {
        let (mut m, _alice, _bob) = moving();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.teams.insert(
            "sub".into(),
            serde_json::from_value(json!({
                "teamId": "sub", "name": "Sub", "parent": "pt", "join": "invite",
                "members": { "bob": true }
            }))
            .unwrap(),
        );
        for team in ["sub", "t"] {
            m.state.apps.get_mut("oa").unwrap().access =
                crate::types::Access::Team { team: team.into() };
            let e = m
                .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
                .unwrap_err();
            assert_eq!(e, super::MOVE_NEEDS_APP_TEAM, "{team}");
        }
        m.state.apps.get_mut("oa").unwrap().access = crate::types::Access::Org;
        m.state
            .orgs
            .get_mut("org")
            .unwrap()
            .approved_releases
            .insert("2.0.0".into());
        m.state
            .orgs
            .get_mut("new")
            .unwrap()
            .approved_releases
            .insert("3.0.0".into());
        let e = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap_err();
        assert_eq!(e, super::MOVE_RELEASES_DISJOINT);
        m.state
            .orgs
            .get_mut("new")
            .unwrap()
            .approved_releases
            .insert("2.0.0".into());
        let _ok = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap();
    }

    /// The target's mode decides the access: a shared target (some app team
    /// closed) gets a closed app team, as its others are; a legacy target
    /// (no app team) keeps organization access and gets no team, so moving
    /// an app in never changes how it holds its other apps; a target that
    /// has the package's app team already uses it.
    #[test]
    fn a_moved_app_follows_the_targets_mode() {
        let (mut m, _alice, _bob) = moving();
        m.state
            .orgs
            .get_mut("new")
            .unwrap()
            .teams
            .get_mut("qt")
            .unwrap()
            .join = crate::types::TeamJoin::Invite;
        let out = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap();
        let team = out["access"]["team"].as_str().unwrap().to_owned();
        let t = &m.state.orgs["new"].teams[&team];
        assert_eq!(t.join, crate::types::TeamJoin::Invite);
        assert_eq!(
            t.default_capabilities,
            Some(crate::types::caps::CREATE_CHILD_TEAM)
        );
        assert!(m.state.may_reach(&m.state.apps["oa"], Some("bob")));

        // A second instance goes onto the same team.
        let _prev = m.state.apps.insert("ob".into(), {
            let mut a = m.state.apps["oa"].clone();
            a.app_id = "ob".into();
            a.org = Some("org".into());
            a.access = crate::types::Access::Team { team: "pt".into() };
            a
        });
        let out = m
            .run_unproven(move_app("ob", "new", "alice", true, Value::Null))
            .unwrap();
        assert_eq!(out["created"], false, "{out}");
        assert_eq!(out["access"]["team"], team, "{out}");

        // Into a legacy organization: organization access, no team.
        let _prev = m.state.orgs.insert(
            "leg".into(),
            serde_json::from_value(json!({
                "orgId": "leg", "name": "Leg", "members": { "alice": "owner" }, "teams": {}
            }))
            .unwrap(),
        );
        let out = m
            .run_unproven(move_app("ob", "leg", "alice", true, Value::Null))
            .unwrap();
        assert_eq!(out["access"], json!({ "kind": "org" }), "{out}");
        assert!(m.state.orgs["leg"].teams.is_empty());
        assert!(m.state.may_reach(&m.state.apps["ob"], Some("bob")));

        // One app team closed alone (another still open): the team made for
        // the moved app is open, as the other apps' are.
        let (mut m, _alice, _bob) = moving();
        let target = m.state.orgs.get_mut("new").unwrap();
        target.teams.get_mut("qt").unwrap().join = crate::types::TeamJoin::Invite;
        let _prev = target.teams.insert(
            "rt".into(),
            serde_json::from_value(json!({
                "teamId": "rt", "name": "R", "join": "everyone", "members": {},
                "metadata": { "app": { "package": "r", "name": "R" } }
            }))
            .unwrap(),
        );
        let out = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap();
        let team = out["access"]["team"].as_str().unwrap().to_owned();
        let t = &m.state.orgs["new"].teams[&team];
        assert_eq!(t.join, crate::types::TeamJoin::Everyone);
        assert_eq!(
            t.default_capabilities,
            Some(crate::types::caps::INVITE_MEMBERS | crate::types::caps::CREATE_CHILD_TEAM)
        );
        assert_eq!(
            m.state.orgs["new"].teams["rt"].join,
            crate::types::TeamJoin::Everyone
        );
    }

    /// A personal app moves into an organization its owner administers,
    /// keeping its access, and its files go to the organization's bill. It
    /// is its owner's alone to move, and only into an organization they
    /// administer.
    #[test]
    fn a_personal_app_moves_into_an_organization_its_owner_administers() {
        let (mut m, _alice, _bob) = moving();
        m.state.set_app_file_bytes("app", 7);
        let before = m.state.apps["app"].access.clone();
        let _prev = m
            .state
            .orgs
            .get_mut("new")
            .unwrap()
            .members
            .insert("bob".into(), OrgRole::Admin);
        let e = m
            .run_unproven(move_app("app", "new", "bob", false, Value::Null))
            .unwrap_err();
        assert_eq!(e, super::ONLY_MANAGERS_MOVE);
        let e = m
            .run_unproven(move_app("app", "org", "alice", false, Value::Null))
            .map(|_| ());
        assert!(e.is_ok(), "alice owns org: {e:?}");
        assert_eq!(m.state.apps["app"].org.as_deref(), Some("org"));
        assert_eq!(m.state.apps["app"].access, before);
        assert_eq!(m.state.file_bytes("org"), 7);
        assert_eq!(m.state.file_bytes("account:alice"), 0);

        // Bob's own app, into an organization where he is only a member.
        let _prev = m.state.apps.insert("bp".into(), {
            let mut a = m.state.apps["app"].clone();
            a.app_id = "bp".into();
            a.org = None;
            a.owner = "bob".into();
            a
        });
        let e = m
            .run_unproven(move_app("bp", "org", "bob", false, Value::Null))
            .unwrap_err();
        assert_eq!(e, super::ONLY_ADMINS_MOVE_IN);
        let _ok = m
            .run_unproven(move_app("bp", "new", "bob", false, Value::Null))
            .unwrap();
    }

    /// A `MoveApp` entry that names neither `bring_members` nor `nonce`
    /// reads as a move without members, and the state it applies to has no
    /// new field: an app's record moved reads and writes as any other.
    #[test]
    fn a_move_entry_without_its_optional_fields_reads_as_a_plain_move() {
        let cmd: SystemCmd = serde_json::from_value(json!({
            "op": "moveApp", "app_id": "oa", "to_org": "new", "actor": "alice"
        }))
        .unwrap();
        let SystemCmd::MoveApp {
            bring_members,
            nonce,
            consent,
            ..
        } = &cmd
        else {
            panic!("{cmd:?}");
        };
        assert!(!bring_members);
        assert!(nonce.is_empty());
        assert!(consent.is_null());
        let (mut m, _alice, _bob) = moving();
        let _prev = m
            .state
            .orgs
            .get_mut("new")
            .unwrap()
            .members
            .insert("bob".into(), OrgRole::Member);
        let _ok = m.run_unproven(cmd).unwrap();
        let record = serde_json::to_value(&m.state.apps["oa"]).unwrap();
        let back: AppRecord = serde_json::from_value(record.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), record);
    }

    /// An organization's app is managed by its owner only while they are a
    /// member of the organization (`member` or above); once removed, or made
    /// a guest, they change its access, metadata or owner, delete it and
    /// upgrade it no more.
    #[test]
    fn an_org_apps_owner_manages_it_only_while_a_member() {
        let (mut m, _alice, mut bob) = org_app_of_bob();
        assert!(m.state.owner_acts(&m.state.apps["oa"], "bob"));
        assert!(m.state.may_upgrade(&m.state.apps["oa"], "bob"));
        let _ok = m.run(set_app_metadata("oa", "bob")).unwrap();

        for demoted in [Some(OrgRole::Guest), None] {
            let o = m.state.orgs.get_mut("org").unwrap();
            match demoted {
                Some(role) => {
                    let _prev = o.members.insert("bob".into(), role);
                }
                None => {
                    let _gone = o.members.remove("bob");
                }
            }
            let app = &m.state.apps["oa"];
            assert!(!m.state.owner_acts(app, "bob"), "{demoted:?}");
            assert!(!m.state.manages_app(app, "bob"), "{demoted:?}");
            assert!(!m.state.may_upgrade(app, "bob"), "{demoted:?}");
            assert!(m.run(set_access("oa", "bob")).is_err(), "{demoted:?}");
            assert!(m.run(set_app_metadata("oa", "bob")).is_err(), "{demoted:?}");
            assert!(m.run(delete_app("oa", "bob")).is_err(), "{demoted:?}");
            let transfer = transfer_app(&mut bob, "oa", "bob", "carol");
            assert!(m.run(transfer).is_err(), "{demoted:?}");
        }
        let app = &m.state.apps["oa"];
        assert_eq!((app.owner.as_str(), &app.access), ("bob", &Access::Owner));
    }

    /// An app on a team is reached and managed by its owner only while they
    /// are in that team: an owner removed from it, who left it, or who lost
    /// it when it closed, reaches it, changes it, deletes it and upgrades it
    /// no more, and the organization's admins transfer it. Back in the team,
    /// they own it again. An entry an earlier release logged (no
    /// `owner_in_team`) replays judging the owner without the team.
    #[test]
    fn an_apps_owner_reaches_and_manages_an_app_on_a_team_only_while_in_it() {
        let (mut m, mut alice, mut bob) = org_app_of_bob();
        let team = |members: &[&str], join: TeamJoin| Team {
            team_id: "t".into(),
            name: "chat".into(),
            parent: None,
            join,
            members: members.iter().map(|a| ((*a).to_owned(), false)).collect(),
            read_only: BTreeSet::new(),
            capabilities: BTreeMap::new(),
            metadata: json!({ "app": { "package": "com.chat" } }),
            removed: BTreeSet::new(),
            default_capabilities: None,
        };
        let set = |m: &mut SystemMachine, t: Team| {
            let _prev = m
                .state
                .orgs
                .get_mut("org")
                .unwrap()
                .teams
                .insert("t".into(), t);
        };
        set(&mut m, team(&["bob"], TeamJoin::Invite));
        m.state.apps.get_mut("oa").unwrap().access = Access::Team { team: "t".into() };
        let app = m.state.apps["oa"].clone();
        assert!(m.state.may_reach(&app, Some("bob")));
        assert!(m.state.owner_acts(&app, "bob"));
        let _ok = m.run(set_app_metadata("oa", "bob")).unwrap();

        // Kicked or left (no row), closed without them (no row, no longer
        // open), or barred from the open team (`removed`).
        let mut barred = team(&[], TeamJoin::Everyone);
        let _new = barred.removed.insert("bob".into());
        for out in [team(&[], TeamJoin::Invite), barred] {
            set(&mut m, out);
            let app = &m.state.apps["oa"];
            assert!(!m.state.may_reach(app, Some("bob")));
            assert!(!m.state.owner_acts(app, "bob"));
            assert!(!m.state.manages_app(app, "bob"));
            assert!(!m.state.may_upgrade(app, "bob"));
            assert!(m.run(set_access("oa", "bob")).is_err());
            assert!(m.run(set_app_metadata("oa", "bob")).is_err());
            assert!(m.run(delete_app("oa", "bob")).is_err());
            assert!(m.run(transfer_app(&mut bob, "oa", "bob", "carol")).is_err());
        }
        // An entry an earlier release logged replays as it judged it.
        let old = |cmd: SystemCmd| {
            let mut v = serde_json::to_value(cmd).unwrap();
            assert_eq!(v["owner_in_team"], json!(true), "{v}");
            let _flag = v.as_object_mut().unwrap().remove("owner_in_team");
            serde_json::from_value::<SystemCmd>(v).unwrap()
        };
        assert!(m
            .state
            .may_upgrade_as_logged(&m.state.apps["oa"], "bob", OwnerRule::InOrg));
        let _ok = m.run(old(set_app_metadata("oa", "bob"))).unwrap();
        // Open to the organization again: Bob, a member, is in it.
        set(&mut m, team(&[], TeamJoin::Everyone));
        assert!(m.state.may_reach(&m.state.apps["oa"], Some("bob")));
        assert!(m.state.owner_acts(&m.state.apps["oa"], "bob"));
        set(&mut m, team(&["carol"], TeamJoin::Invite));
        let _ok = m
            .run(transfer_app(&mut alice, "oa", "alice", "carol"))
            .unwrap();
        assert!(m.state.owner_acts(&m.state.apps["oa"], "carol"));
    }

    /// The organization's admins clean up an app whose owner left: they
    /// transfer it to a member or delete it. A member who neither owns nor
    /// administers it does neither.
    #[test]
    fn an_org_admin_transfers_or_deletes_an_app_whose_owner_left() {
        let (mut m, mut alice, _bob) = org_app_of_bob();
        let mut carol = Soft::new("localhost", "http://localhost:3000");
        let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
        let _prev = m.state.apps.insert("ob".into(), {
            let mut b = m.state.apps["oa"].clone();
            b.app_id = "ob".into();
            b
        });
        assert!(m
            .run(transfer_app(&mut carol, "oa", "carol", "carol"))
            .is_err());
        assert!(m.run(delete_app("oa", "carol")).is_err());

        let _ok = m
            .run(transfer_app(&mut alice, "oa", "alice", "carol"))
            .unwrap();
        assert_eq!(m.state.apps["oa"].owner, "carol");
        assert!(m.state.owner_acts(&m.state.apps["oa"], "carol"));
        let _ok = m.run(delete_app("ob", "alice")).unwrap();
        assert!(!m.state.apps.contains_key("ob"));
    }

    /// A member owner of an organization's app, and the owner of a personal
    /// app, manage it as before.
    #[test]
    fn a_member_owner_and_a_personal_owner_manage_their_apps() {
        let (mut m, mut alice, mut bob) = org_app_of_bob();
        let _ok = m.run(set_access("oa", "bob")).unwrap();
        let _ok = m.run(set_app_metadata("oa", "bob")).unwrap();
        let _ok = m.run(transfer_app(&mut bob, "oa", "bob", "carol")).unwrap();
        assert_eq!(m.state.apps["oa"].owner, "carol");

        // `app` is alice's own, outside any organization.
        assert!(m.state.owner_acts(&m.state.apps["app"], "alice"));
        assert!(!m.state.manages_app(&m.state.apps["app"], "bob"));
        let _ok = m.run(set_access("app", "alice")).unwrap();
        let _ok = m.run(set_app_metadata("app", "alice")).unwrap();
        let _ok = m
            .run(transfer_app(&mut alice, "app", "alice", "bob"))
            .unwrap();
        let _ok = m.run(delete_app("app", "bob")).unwrap();
        assert!(!m.state.apps.contains_key("app"));
    }

    /// App commands an earlier release logged, without `owner_in_org`,
    /// replay as that release judged them: ownership alone, so an owner who
    /// had left still changed the app then, and an admin who did not own it
    /// did not transfer it. The entries this release logs carry the flag.
    #[test]
    fn app_commands_logged_before_the_membership_rule_replay_as_they_did() {
        let (mut m, mut alice, mut bob) = org_app_of_bob();
        let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
        let old = |cmd: SystemCmd| {
            let mut v = serde_json::to_value(cmd).unwrap();
            assert_eq!(v["owner_in_org"], json!(true), "{v}");
            let _flag = v.as_object_mut().unwrap().remove("owner_in_org");
            let _flag = v.as_object_mut().unwrap().remove("owner_in_team");
            serde_json::from_value::<SystemCmd>(v).unwrap()
        };
        assert!(!m.state.may_upgrade(&m.state.apps["oa"], "bob"));
        assert!(m
            .state
            .may_upgrade_as_logged(&m.state.apps["oa"], "bob", OwnerRule::Ownership));
        let refused = m
            .run(old(transfer_app(&mut alice, "oa", "alice", "carol")))
            .unwrap_err();
        assert_eq!(refused, "only the owner may transfer");
        let _ok = m.run(old(set_app_metadata("oa", "bob"))).unwrap();
        let _ok = m.run(old(set_access("oa", "bob"))).unwrap();
        assert_eq!(m.state.apps["oa"].access, Access::Public);
        let _ok = m
            .run(old(transfer_app(&mut bob, "oa", "bob", "carol")))
            .unwrap();
        assert_eq!(m.state.apps["oa"].owner, "carol");
        let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("carol");
        let _ok = m.run(old(delete_app("oa", "carol"))).unwrap();
        assert!(!m.state.apps.contains_key("oa"));
    }

    // ---------------------------------------------------- account recovery

    use super::{
        RefreshGrant, ACCOUNT_RECOVERY_PENDING, NO_ACCOUNT_RECOVERY_PENDING, UNKNOWN_RECOVERY_CODE,
    };

    /// A canonical recovery code and its public key.
    fn recovery_code() -> (String, String) {
        let code =
            calimero_identity::normalize_recovery_code(&calimero_identity::new_recovery_code())
                .unwrap();
        let key = crate::proof::recovery_key(&code);
        (code, key)
    }

    /// Set `account`'s recovery code with `pk`'s consent.
    fn set_code(m: &mut SystemMachine, pk: &mut Soft, account: &str, key: &str) {
        let c = consent(
            pk,
            &format!("account:{account}"),
            "__set_recovery",
            &json!({ "recoveryKey": key }),
        );
        let _ok = m
            .run(SystemCmd::SetAccountRecoveryKey {
                account: account.into(),
                recovery_key: key.into(),
                consent: c,
            })
            .unwrap();
    }

    /// A passkey registered on a new device, as the start registers one.
    fn new_device() -> (Soft, calimero_identity::Credential) {
        let mut device = Soft::new("localhost", "http://localhost:3000");
        let rp = calimero_identity::RelyingParty {
            id: "localhost".into(),
            origins: vec!["http://localhost:3000".into()],
            require_uv: true,
        };
        let challenge = calimero_identity::random::<32>();
        let cred =
            calimero_identity::verify_registration(&rp, &challenge, &device.register(&challenge))
                .unwrap();
        (device, cred)
    }

    fn start_with(
        key: &str,
        account: &str,
        credential: &calimero_identity::Credential,
        at: u64,
        signature: String,
    ) -> SystemCmd {
        SystemCmd::StartAccountRecovery {
            recovery_key: key.into(),
            account: account.into(),
            credential: credential.clone(),
            at,
            signature,
        }
    }

    /// The finish of `account`'s recovery at `at`, opening `record` with
    /// `pk`'s sign-in assertion over it.
    fn finish_with(
        pk: &mut Soft,
        account: &str,
        at: u64,
        (hash, record): (&str, &crate::types::RefreshRecord),
    ) -> SystemCmd {
        SystemCmd::FinishAccountRecovery {
            account: account.into(),
            at,
            refresh: RefreshGrant {
                hash: hash.into(),
                record: record.clone(),
            },
            consent: consent(
                pk,
                "session",
                "__sign_in",
                &super::sign_in_args(hash, record),
            ),
            revoke_api_keys: true,
        }
    }

    /// A recovery code is set with the account's own passkey, replaced by
    /// the next, and removed with the passkey too.
    #[test]
    fn a_recovery_code_is_set_and_removed_with_the_accounts_passkey() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        let (_code, key) = recovery_code();
        // Another account's passkey does not set alice's code.
        let c = consent(
            &mut bob,
            "account:alice",
            "__set_recovery",
            &json!({ "recoveryKey": key }),
        );
        assert!(m
            .run(SystemCmd::SetAccountRecoveryKey {
                account: "alice".into(),
                recovery_key: key.clone(),
                consent: c,
            })
            .is_err());
        needs_consent(
            &mut m,
            &mut alice,
            ("account:alice", "__set_recovery"),
            &json!({ "recoveryKey": key }),
            &json!({ "recoveryKey": "00" }),
            |consent| SystemCmd::SetAccountRecoveryKey {
                account: "alice".into(),
                recovery_key: key.clone(),
                consent,
            },
        );
        assert_eq!(
            m.state.recovery_keys.get(&key).map(String::as_str),
            Some("alice")
        );
        // A new code replaces the old: one per account.
        let (_code, second) = recovery_code();
        set_code(&mut m, &mut alice, "alice", &second);
        assert_eq!(m.state.recovery_keys.len(), 1);
        assert!(m.state.recovery_keys.contains_key(&second));
        needs_consent(
            &mut m,
            &mut alice,
            ("account:alice", "__remove_recovery"),
            &json!({}),
            &json!({ "all": true }),
            |consent| SystemCmd::RemoveAccountRecoveryKey {
                account: "alice".into(),
                consent,
            },
        );
        assert!(m.state.recovery_keys.is_empty());
    }

    /// The start carries the code key's signature over the account and the
    /// new passkey: a forged one, one for another account or another
    /// passkey, and a replay are refused, and the code is spent. One
    /// recovery at a time; only a passkey the account has cancels it.
    #[test]
    fn an_account_recovery_starts_only_with_the_codes_signature_and_the_account_cancels() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        let (code, key) = recovery_code();
        set_code(&mut m, &mut alice, "alice", &key);
        let (mut device, cred) = new_device();
        let start =
            |account: &str, signature: String| start_with(&key, account, &cred, 100, signature);

        // Forged: none, another code's key, a signature over another time.
        assert!(m.run(start("alice", String::new())).is_err(), "none");
        let (guess, _) = recovery_code();
        let forged = crate::proof::sign_recover(&guess, "alice", &cred, 100);
        assert!(m.run(start("alice", forged)).is_err(), "another code");
        let later = crate::proof::sign_recover(&code, "alice", &cred, 101);
        assert!(m.run(start("alice", later)).is_err(), "another time");
        // Wrong account: alice's code does not start bob's recovery, nor
        // does a signature for bob start alice's.
        let for_bob = crate::proof::sign_recover(&code, "bob", &cred, 100);
        assert_eq!(
            m.run(start("bob", for_bob.clone())).unwrap_err(),
            UNKNOWN_RECOVERY_CODE
        );
        assert!(m.run(start("alice", for_bob)).is_err(), "signed for bob");
        // Another passkey than the one signed for.
        let (_other_device, other) = new_device();
        let for_other = crate::proof::sign_recover(&code, "alice", &other, 100);
        assert!(m
            .run(start_with(&key, "alice", &cred, 100, for_other))
            .is_err());

        let real = crate::proof::sign_recover(&code, "alice", &cred, 100);
        let out = m.run(start("alice", real.clone())).unwrap();
        assert_eq!(
            out["readyAt"],
            json!(100 + crate::types::RECOVERY_DELAY_SECS)
        );
        assert!(m.state.recovery_keys.is_empty(), "the code is spent");
        assert_eq!(
            m.state.accounts["alice"].passkeys.len(),
            1,
            "no passkey yet"
        );
        // Replay: the code is spent, and the signature used.
        assert!(m.run(start("alice", real)).is_err(), "replay");
        // A second code does not start a second recovery while one is pending.
        let (code2, key2) = recovery_code();
        set_code(&mut m, &mut alice, "alice", &key2);
        let (_d2, cred2) = new_device();
        let second = crate::proof::sign_recover(&code2, "alice", &cred2, 101);
        assert_eq!(
            m.run(start_with(&key2, "alice", &cred2, 101, second))
                .unwrap_err(),
            ACCOUNT_RECOVERY_PENDING
        );

        // Cancel: not bob's passkey, not the pending passkey itself, not for
        // another passkey; alice's, once.
        let cancel = |consent| SystemCmd::CancelAccountRecovery {
            account: "alice".into(),
            credential_id: cred.id.clone(),
            consent,
        };
        let args = json!({ "credentialId": cred.id });
        let by_bob = consent(&mut bob, "account:alice", "__cancel_recovery", &args);
        assert!(m.run(cancel(by_bob)).is_err(), "another account");
        let by_new = consent(&mut device, "account:alice", "__cancel_recovery", &args);
        assert!(m.run(cancel(by_new)).is_err(), "the pending passkey");
        let wrong = consent(
            &mut alice,
            "account:alice",
            "__cancel_recovery",
            &json!({ "credentialId": cred2.id }),
        );
        assert!(m
            .run(SystemCmd::CancelAccountRecovery {
                account: "alice".into(),
                credential_id: cred2.id.clone(),
                consent: wrong,
            })
            .is_err());
        needs_consent(
            &mut m,
            &mut alice,
            ("account:alice", "__cancel_recovery"),
            &args,
            &json!({ "credentialId": "another" }),
            cancel,
        );
        assert!(m.state.account_recoveries.is_empty());
        // Cancelled, so nothing to finish.
        let (_t, hash, record) = refresh_token("alice", &cred.id, "s2");
        assert_eq!(
            m.run(finish_with(
                &mut device,
                "alice",
                u64::MAX,
                (&hash, &record)
            ))
            .unwrap_err(),
            NO_ACCOUNT_RECOVERY_PENDING
        );
    }

    /// The finish waits out the delay, judged at its own time; only the
    /// passkey the start logged signs it, over the session it opens. It adds
    /// that passkey, ends every session the account had and opens the new
    /// one, once.
    #[test]
    fn an_account_recovery_finishes_after_the_delay_with_the_new_passkey() {
        let (mut m, mut alice, mut bob, _carol) = proven_cluster();
        // Alice's lost device still has a session.
        let (_t, old_hash, old) = refresh_token("alice", &alice.credential_id(), "s1");
        let c = consent(
            &mut alice,
            "session",
            "__sign_in",
            &super::sign_in_args(&old_hash, &old),
        );
        let _ok = m
            .run(SystemCmd::CreateRefresh {
                hash: old_hash.clone(),
                record: old,
                consent: c,
            })
            .unwrap();
        let (code, key) = recovery_code();
        set_code(&mut m, &mut alice, "alice", &key);
        let (mut device, cred) = new_device();
        let signature = crate::proof::sign_recover(&code, "alice", &cred, 200);
        let _ok = m
            .run(start_with(&key, "alice", &cred, 200, signature))
            .unwrap();
        let ready = 200 + crate::types::RECOVERY_DELAY_SECS;
        let (_t, hash, record) = refresh_token("alice", &cred.id, "s2");

        // Before the delay: refused, and the assertion is not spent.
        let early = finish_with(&mut device, "alice", ready - 1, (&hash, &record));
        assert_eq!(m.run(early).unwrap_err(), RECOVERY_NOT_READY);
        // Not alice's old passkey, nor bob's: only the one the start logged.
        assert!(m
            .run(finish_with(&mut alice, "alice", ready, (&hash, &record)))
            .is_err());
        assert!(m
            .run(finish_with(&mut bob, "alice", ready, (&hash, &record)))
            .is_err());
        // Not a session naming another passkey or bound to a package.
        let (_t, h2, mut other) = refresh_token("alice", &alice.credential_id(), "s3");
        assert!(m
            .run(finish_with(&mut device, "alice", ready, (&h2, &other)))
            .is_err());
        other.passkey = Some(cred.id.clone());
        other.app = Some("pkg".into());
        assert!(m
            .run(finish_with(&mut device, "alice", ready, (&h2, &other)))
            .is_err());
        // Bob's account has no recovery under way.
        let (_t, h3, for_bob) = refresh_token("bob", &cred.id, "s4");
        assert_eq!(
            m.run(finish_with(&mut device, "bob", ready, (&h3, &for_bob)))
                .unwrap_err(),
            NO_ACCOUNT_RECOVERY_PENDING
        );
        // A sign-in assertion over another session does not finish this one.
        let (_t, h4, r4) = refresh_token("alice", &cred.id, "s5");
        let mut crossed = finish_with(&mut device, "alice", ready, (&h4, &r4));
        if let SystemCmd::FinishAccountRecovery { refresh, .. } = &mut crossed {
            *refresh = RefreshGrant {
                hash: hash.clone(),
                record: record.clone(),
            };
        }
        assert!(m.run(crossed).is_err());

        let finish = finish_with(&mut device, "alice", ready, (&hash, &record));
        let out = m.run(finish.clone()).unwrap();
        assert_eq!(out["revoked"], json!(1));
        let a = &m.state.accounts["alice"];
        assert_eq!(a.passkeys.len(), 2);
        assert!(
            a.passkeys
                .iter()
                .any(|c| c.id == cred.id && c.sign_count > 0),
            "the finish's counter is kept"
        );
        assert_eq!(m.state.credentials[&cred.id], "alice");
        assert!(m.state.account_recoveries.is_empty());
        assert!(
            !m.state.refresh.contains_key(&old_hash),
            "the old session ended"
        );
        assert!(m.state.revoked_sessions.contains("s1"));
        assert_eq!(m.state.refresh[&hash].session, "s2", "the new session");
        // Replay: nothing is pending any more.
        assert_eq!(m.run(finish).unwrap_err(), NO_ACCOUNT_RECOVERY_PENDING);
        // The new passkey signs in as any of the account's.
        let (_t, h5, r5) = refresh_token("alice", &cred.id, "s6");
        let c = consent(
            &mut device,
            "session",
            "__sign_in",
            &super::sign_in_args(&h5, &r5),
        );
        let _ok = m
            .run(SystemCmd::CreateRefresh {
                hash: h5,
                record: r5,
                consent: c,
            })
            .unwrap();
    }

    /// An API key of `account`, kept under `hash`.
    fn api_key(m: &mut SystemMachine, hash: &str, account: &str) {
        let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
            "keyId": hash, "account": account, "label": "ci", "apps": null,
            "methods": null, "readOnly": false, "allowConfirm": false, "expiresAt": 99,
        }))
        .unwrap();
        let _prev = m.state.api_keys.insert(hash.into(), record);
    }

    /// Start alice's recovery with a new device's passkey: the finish that
    /// device signs once the delay passed.
    fn ready_recovery(m: &mut SystemMachine, alice: &mut Soft) -> SystemCmd {
        let (code, key) = recovery_code();
        set_code(m, alice, "alice", &key);
        let (mut device, cred) = new_device();
        let signature = crate::proof::sign_recover(&code, "alice", &cred, 200);
        let _ok = m
            .run(start_with(&key, "alice", &cred, 200, signature))
            .unwrap();
        let ready = 200 + crate::types::RECOVERY_DELAY_SECS;
        let (_t, hash, record) = refresh_token("alice", &cred.id, "s2");
        finish_with(&mut device, "alice", ready, (&hash, &record))
    }

    /// Whoever stole the lost passkey keeps no machine credential: the
    /// finish revokes every API key of the account, and no other account's.
    #[test]
    fn an_account_recovery_revokes_the_accounts_api_keys() {
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        api_key(&mut m, "a1", "alice");
        api_key(&mut m, "a2", "alice");
        api_key(&mut m, "b1", "bob");
        let finish = ready_recovery(&mut m, &mut alice);
        let out = m.run(finish).unwrap();
        assert_eq!(out["revokedApiKeys"], json!(2), "{out}");
        assert_eq!(
            m.state.api_keys.keys().collect::<Vec<_>>(),
            ["b1"],
            "alice's keys are gone, bob's stays"
        );
    }

    /// A finish an earlier release logged (no `revoke_api_keys`) replays as
    /// that release applied it: the keys stay, so a replica that replays it
    /// after a restart agrees with one that applied it then.
    #[test]
    fn a_finish_logged_before_key_revocation_replays_as_it_did() {
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        api_key(&mut m, "a1", "alice");
        let finish = ready_recovery(&mut m, &mut alice);
        let mut logged = serde_json::to_value(&finish).unwrap();
        assert_eq!(
            logged.as_object_mut().unwrap().remove("revoke_api_keys"),
            Some(json!(true))
        );
        let old: SystemCmd = serde_json::from_value(logged).unwrap();
        let out = m.run(old).unwrap();
        assert_eq!(out["revokedApiKeys"], json!(0), "{out}");
        assert!(m.state.api_keys.contains_key("a1"));
        assert!(m.state.account_recoveries.is_empty(), "it still finished");
    }

    /// State written before account recovery reads with no codes and no
    /// recovery under way, and writes back the same bytes; state holding
    /// both reads back as written.
    #[test]
    fn state_from_before_account_recovery_reads_and_writes_as_before() {
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        let before = serde_json::to_value(&m.state).unwrap();
        assert!(before.get("recoveryKeys").is_none());
        assert!(before.get("accountRecoveries").is_none());
        let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
        assert!(restored.recovery_keys.is_empty());
        assert!(restored.account_recoveries.is_empty());
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
        assert_eq!(
            serde_json::to_vec(&restored).unwrap(),
            serde_json::to_vec(&m.state).unwrap()
        );

        let (code, key) = recovery_code();
        set_code(&mut m, &mut alice, "alice", &key);
        let (_device, cred) = new_device();
        let signature = crate::proof::sign_recover(&code, "alice", &cred, 5);
        let _ok = m
            .run(start_with(&key, "alice", &cred, 5, signature))
            .unwrap();
        let (_code, key2) = recovery_code();
        set_code(&mut m, &mut alice, "alice", &key2);
        let now = serde_json::to_value(&m.state).unwrap();
        assert_eq!(now["recoveryKeys"][&key2], json!("alice"));
        assert_eq!(
            now["accountRecoveries"]["alice"]["readyAt"],
            json!(5 + crate::types::RECOVERY_DELAY_SECS)
        );
        let restored: SystemState = serde_json::from_value(now.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), now);
    }

    /// Nodes and quotas apply only with the operator's signature over
    /// exactly that change, once.
    #[test]
    fn the_fleet_and_quotas_need_the_operators_signature() {
        let (mut m, ..) = proven_cluster();
        assert_eq!(m.state.nodes.len(), 4, "genesis registers the peers");
        let node = crate::types::NodeRecord {
            id: 9,
            addr: "http://n9/".into(),
            zone: "z".into(),
            role: crate::types::NodeRole::Replica,
            pool: String::new(),
        };
        needs_signature(
            &mut m,
            ("cluster", "__register_node"),
            &json!({ "id": 9, "addr": "http://n9/", "zone": "z" }),
            &json!({ "id": 9, "addr": "http://evil/", "zone": "z" }),
            |proof| SystemCmd::RegisterNode {
                node: node.clone(),
                proof,
            },
        );
        assert!(m.state.nodes.contains_key(&9));
        // A job node's role is part of what the operator signs.
        let job = crate::types::NodeRecord {
            id: 10,
            addr: "http://n10/".into(),
            zone: "z".into(),
            role: crate::types::NodeRole::Job,
            pool: String::new(),
        };
        needs_signature(
            &mut m,
            ("cluster", "__register_node"),
            &json!({ "id": 10, "addr": "http://n10/", "zone": "z", "role": "job" }),
            &json!({ "id": 10, "addr": "http://n10/", "zone": "z" }),
            |proof| SystemCmd::RegisterNode {
                node: job.clone(),
                proof,
            },
        );
        assert_eq!(m.state.nodes[&10].role, crate::types::NodeRole::Job);
        needs_signature(
            &mut m,
            ("cluster", "__remove_node"),
            &json!({ "id": 9 }),
            &json!({ "id": 4 }),
            |proof| SystemCmd::RemoveNode { id: 9, proof },
        );
        assert!(!m.state.nodes.contains_key(&9));
        let mut quotas = m.state.quotas.clone();
        quotas.max_accounts = 7;
        let mut other = quotas.clone();
        other.max_accounts = 7000;
        needs_signature(
            &mut m,
            ("cluster", "__set_quotas"),
            &json!({ "quotas": quotas }),
            &json!({ "quotas": other }),
            |proof| SystemCmd::SetQuotas {
                quotas: quotas.clone(),
                proof,
            },
        );
        assert_eq!(m.state.quotas.max_accounts, 7);
    }

    /// A move is signed once, over its target: the union step applies with
    /// the signature and leaves it unspent, the target spends it, and the
    /// union replayed after the move is refused.
    #[test]
    fn a_replica_move_is_signed_once_over_its_target() {
        let (mut m, ..) = proven_cluster();
        let set = |replicas: &[u64], proof: &serde_json::Value| SystemCmd::SetReplicas {
            app_id: "app".into(),
            replicas: replicas.to_vec(),
            proof: proof.clone(),
        };
        let proof = signed("app", "__set_replicas", &json!({ "replicas": [2, 3, 4] }));
        assert!(m.run(set(&[2, 3, 4], &serde_json::Value::Null)).is_err());
        let other = signed("app", "__set_replicas", &json!({ "replicas": [1, 3, 4] }));
        assert!(m.run(set(&[2, 3, 4], &other)).is_err(), "another target");
        let elsewhere = signed("other", "__set_replicas", &json!({ "replicas": [2, 3, 4] }));
        assert!(m.run(set(&[2, 3, 4], &elsewhere)).is_err(), "another app");
        assert!(
            m.run(set(&[1, 2, 3, 4, 9], &proof)).is_err(),
            "not the union"
        );

        let _ok = m.run(set(&[1, 2, 3, 4], &proof)).unwrap();
        assert_eq!(m.state.apps["app"].replicas, [1, 2, 3, 4]);
        let _ok = m.run(set(&[4, 3, 2], &proof)).unwrap();
        assert_eq!(m.state.apps["app"].replicas, [4, 3, 2]);
        assert_eq!(
            m.run(set(&[1, 2, 3, 4], &proof)).unwrap_err(),
            "that consent was used already",
            "the union step replayed after the move"
        );
        assert!(
            m.run(set(&[2, 3, 4], &proof)).is_err(),
            "the final step replayed"
        );
        assert_eq!(m.state.apps["app"].replicas, [4, 3, 2]);
    }

    /// An organization of `who` (account -> role), each holding `caps`
    /// organization-wide, with `releases` approved.
    fn org(members: &[(&str, &str)], caps: &[(&str, u32)], releases: &[&str]) -> crate::types::Org {
        let members: serde_json::Map<String, serde_json::Value> = members
            .iter()
            .map(|(a, r)| ((*a).to_owned(), json!(r)))
            .collect();
        let caps: serde_json::Map<String, serde_json::Value> = caps
            .iter()
            .map(|(a, c)| ((*a).to_owned(), json!(c)))
            .collect();
        serde_json::from_value(json!({
            "orgId": "org", "name": "Docs", "members": members, "capabilities": caps,
            "approvedReleases": releases, "teams": {}
        }))
        .unwrap()
    }

    fn approve(org: &str, version: &str, actor: &str) -> SystemCmd {
        SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Org { org: org.into() },
            version: version.into(),
            approved: true,
            actor: actor.into(),
            consent: serde_json::Value::Null,
        }
    }

    /// `APPROVE_RELEASES`: the owner and admins hold it (as `MANAGE_APPLICATION`),
    /// a member only when granted organization-wide; a team grant, a guest,
    /// someone outside, a revoked grant and a removed member do not.
    #[test]
    fn approving_releases_is_a_capability() {
        use crate::types::caps::APPROVE_RELEASES;
        let mut m = SystemMachine::default();
        let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob", "carol", "dave", "erin", "frank", "gina"] {
            let _ok = m.run_unproven(account(who)).unwrap();
        }
        let mut o = org(
            &[
                ("alice", "owner"),
                ("bob", "member"),
                ("carol", "member"),
                ("dave", "admin"),
                ("frank", "member"),
                ("gina", "guest"),
            ],
            &[("bob", APPROVE_RELEASES), ("gina", APPROVE_RELEASES)],
            &["2.0.0"],
        );
        let _prev = o.teams.insert(
            "t".into(),
            serde_json::from_value(json!({
                "teamId": "t", "name": "T", "join": "invite",
                "members": { "frank": false }, "capabilities": { "frank": APPROVE_RELEASES }
            }))
            .unwrap(),
        );
        let _prev = m.state.orgs.insert("org".into(), o);

        for (actor, may) in [
            ("alice", true),
            ("dave", true),
            ("bob", true),
            ("carol", false),
            ("frank", false),
            ("gina", false),
            ("erin", false),
        ] {
            assert_eq!(
                m.state.approves_releases("org", actor),
                may,
                "{actor} holds it"
            );
            let r = m.run_unproven(approve("org", &format!("2.1.{}", actor.len()), actor));
            assert_eq!(r.is_ok(), may, "{actor} approving: {r:?}");
        }
        assert_eq!(
            m.state.release_approvers("org"),
            vec!["alice".to_owned(), "bob".into(), "dave".into()]
        );

        // Revoked: refused at apply, whatever the API saw before.
        let _ok = m
            .run_unproven(SystemCmd::SetCapabilities {
                org: "org".into(),
                team: None,
                account: "bob".into(),
                capabilities: 0,
                actor: "alice".into(),
                consent: serde_json::Value::Null,
            })
            .unwrap();
        assert!(m.run_unproven(approve("org", "2.2.0", "bob")).is_err());
        // Removed: gone with the membership.
        let _ok = m
            .run_unproven(SystemCmd::SetCapabilities {
                org: "org".into(),
                team: None,
                account: "bob".into(),
                capabilities: APPROVE_RELEASES,
                actor: "alice".into(),
                consent: serde_json::Value::Null,
            })
            .unwrap();
        let _ok = m
            .run_unproven(SystemCmd::RemoveOrgMember {
                org: "org".into(),
                account: "bob".into(),
                actor: "dave".into(),
                consent: serde_json::Value::Null,
            })
            .unwrap();
        assert!(m.run_unproven(approve("org", "2.2.0", "bob")).is_err());

        // An account's own releases stay its own.
        let own = SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Account,
            version: "2.4.0".into(),
            approved: true,
            actor: "erin".into(),
            consent: serde_json::Value::Null,
        };
        let _ok = m.run_unproven(own).unwrap();
        assert!(m.state.accounts["erin"].approved_releases.contains("2.4.0"));

        // Where the operator holds release approvals, nobody in an org does.
        let mut m = SystemMachine::default();
        let _ok = m.run_unproven(genesis(all(Authority::Operator))).unwrap();
        let _ok = m.run_unproven(account("alice")).unwrap();
        let _prev = m
            .state
            .orgs
            .insert("org".into(), org(&[("alice", "owner")], &[], &["2.0.0"]));
        assert!(!m.state.approves_releases("org", "alice"));
        assert!(m.run_unproven(approve("org", "2.1.0", "alice")).is_err());
    }

    /// Past a release's deadline, a scope whose newest approved release is
    /// older creates, upgrades and installs nothing; one that approved it is
    /// untouched, and approving lifts the stop at once. Judged at the time
    /// each command carries, so replicas and replays agree.
    #[test]
    fn a_release_deadline_stops_new_work_until_the_release_is_approved() {
        use super::{PackageVersion, DEADLINE_PREFIX, OPERATOR};
        const T: u64 = 1_000;
        let mut m = SystemMachine::default();
        let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob"] {
            let _ok = m.run_unproven(account(who)).unwrap();
            if let Some(a) = m.state.accounts.get_mut(who) {
                let _new = a.approved_releases.insert("2.0.0".into());
            }
        }
        let mut a = org(&[("alice", "owner")], &[], &["2.0.0"]);
        a.org_id = "a".into();
        let mut b = org(&[("bob", "owner")], &[], &["2.0.0", "2.1.0"]);
        b.org_id = "b".into();
        let _prev = m.state.orgs.insert("a".into(), a);
        let _prev = m.state.orgs.insert("b".into(), b);
        for hash in ["b1", "b2"] {
            let _prev = m.state.bundles.insert(
                hash.into(),
                serde_json::from_value(json!({ "hash": hash, "size": 1, "uploadedBy": "alice" }))
                    .unwrap(),
            );
        }
        let app = |id: &str, org: &str, owner: &str, at: u64| SystemCmd::CreateApp {
            app: Box::new(
                serde_json::from_value(json!({
                    "appId": id, "name": id, "group": 0, "bundle": "b1", "owner": owner,
                    "org": org, "access": { "kind": "org" }, "replicas": [1], "createdAt": at
                }))
                .unwrap(),
            ),
        };
        let upgrade = |id: &str, actor: &str, at: u64| SystemCmd::UpgradeApp {
            compute_bundle: None,
            app_id: id.into(),
            bundle: "b2".into(),
            by_operator: false,
            actor: actor.into(),
            at,
            run: None,
            consent: serde_json::Value::Null,
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        };
        let install = |by: &str, version: &str, at: u64| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: serde_json::from_value::<PackageVersion>(json!({
                "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
                "services": { "": "b1" }, "installedBy": by, "installedAt": at
            }))
            .unwrap(),
        };
        let _ok = m.run_unproven(app("old", "a", "alice", 1)).unwrap();

        // The operator announces 2.1.0's deadline; closed input first.
        for bad in ["latest", "", &"9".repeat(80)] {
            assert!(m
                .run_unproven(SystemCmd::SetReleaseDeadline {
                    version: bad.into(),
                    after: Some(T),
                    proof: serde_json::Value::Null,
                })
                .is_err());
        }
        let _ok = m
            .run_unproven(SystemCmd::SetReleaseDeadline {
                version: "2.1.0".into(),
                after: Some(T),
                proof: serde_json::Value::Null,
            })
            .unwrap();

        // Before it, everything goes on.
        let _ok = m.run_unproven(app("early", "a", "alice", T - 1)).unwrap();
        let _ok = m.run_unproven(upgrade("old", "alice", T - 1)).unwrap();
        let _ok = m.run_unproven(install("alice", "1.0.0", T - 1)).unwrap();
        assert!(!m.state.retired("2.0.0", T - 1));

        // From it, the organization that has not approved 2.1.0 starts nothing.
        let refused = |r: Result<serde_json::Value, String>| {
            let e = r.unwrap_err();
            assert!(e.starts_with(DEADLINE_PREFIX) && e.contains("2.1.0"), "{e}");
        };
        refused(m.run_unproven(app("late", "a", "alice", T)));
        refused(m.run_unproven(upgrade("old", "alice", T)));
        refused(m.run_unproven(install("alice", "1.1.0", T)));
        assert!(m.state.apps.contains_key("old"), "its apps stay");
        assert_eq!(
            m.state.stalled_for(Some("a"), "alice", T).unwrap().release,
            "2.1.0"
        );
        assert!(m.state.retired("2.0.0", T) && !m.state.retired("2.1.0", T));

        // The organization that approved it is unaffected.
        let _ok = m.run_unproven(app("theirs", "b", "bob", T)).unwrap();
        // So is the operator's install, and the approval itself.
        let _ok = m.run_unproven(install(OPERATOR, "1.2.0", T)).unwrap();

        // Approving 2.1.0 lifts it at once.
        let _ok = m.run_unproven(approve("a", "2.1.0", "alice")).unwrap();
        let _ok = m.run_unproven(app("late", "a", "alice", T)).unwrap();
        let _ok = m.run_unproven(upgrade("old", "alice", T)).unwrap();

        // A later deadline names the newest release not approved.
        let _ok = m
            .run_unproven(SystemCmd::SetReleaseDeadline {
                version: "2.2.0".into(),
                after: Some(T + 10),
                proof: serde_json::Value::Null,
            })
            .unwrap();
        assert!(m.state.stalled_for(Some("a"), "alice", T + 9).is_none());
        assert_eq!(
            m.state
                .stalled_for(Some("b"), "bob", T + 10)
                .unwrap()
                .release,
            "2.2.0"
        );
        // Lifted by the operator: gone.
        let _ok = m
            .run_unproven(SystemCmd::SetReleaseDeadline {
                version: "2.2.0".into(),
                after: None,
                proof: serde_json::Value::Null,
            })
            .unwrap();
        assert!(m.state.stalled_for(Some("b"), "bob", T + 10).is_none());
    }

    /// A package signing key, its `did:key`, and a claim of it for `org`.
    fn publisher_key(seed: u8) -> (ed25519_dalek::SigningKey, String) {
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let did = calimero_bundle::derive_signer_id_did_key(&key.verifying_key().to_bytes());
        (key, did)
    }

    /// The binding of `signer` to `org`, signed by `key` for `as_org` at
    /// `generation`, proposed by `actor`.
    fn bind(
        key: &ed25519_dalek::SigningKey,
        signer: &str,
        (org, as_org): (&str, &str),
        generation: u64,
        actor: &str,
    ) -> SystemCmd {
        use ed25519_dalek::Signer as _;
        let message = super::publisher_binding_message("test", as_org, signer, generation);
        SystemCmd::BindPublisher {
            signer_id: signer.into(),
            org: org.into(),
            cluster: "test".into(),
            proof: super::PublisherProof {
                public_key: calimero_identity::b64url(&key.verifying_key().to_bytes()),
                signature: calimero_identity::b64url(&key.sign(&message).to_bytes()),
            },
            actor: actor.into(),
            consent: serde_json::Value::Null,
        }
    }

    /// A cluster with a publisher org `pub` (pam owner, bob member), a
    /// customer org `cust` (cora owner), the package `p` signed by `signer`
    /// (release 1.0.0 ships `b1`, `upgrades` as given), a raw bundle `raw`,
    /// and the customer's instances `inst` (of `p`) and `rawapp`.
    fn publishing(signer: &str, upgrades: &str) -> SystemMachine {
        let mut m = SystemMachine::default();
        let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
        for who in ["pam", "bob", "cora", "dan"] {
            let _ok = m.run_unproven(account(who)).unwrap();
        }
        let mut o = org(&[("pam", "owner"), ("bob", "member")], &[], &["2.0.0"]);
        o.org_id = "pub".into();
        let _prev = m.state.orgs.insert("pub".into(), o);
        let mut o = org(
            &[("cora", "owner"), ("dan", "member")],
            &[],
            &["2.0.0", "1.9.0"],
        );
        o.org_id = "cust".into();
        let _prev = m.state.orgs.insert("cust".into(), o);
        for (hash, package) in [
            ("b1", json!({ "package": "p", "version": "1.0.0" })),
            ("b2", json!({ "package": "p", "version": "1.1.0" })),
            ("raw", json!(null)),
        ] {
            let _prev = m.state.bundles.insert(
                hash.into(),
                serde_json::from_value(json!({
                    "hash": hash, "size": 1, "uploadedBy": "cora", "package": package
                }))
                .unwrap(),
            );
        }
        let _prev = m.state.packages.insert(
            "p".into(),
            serde_json::from_value(json!({
                "name": "p", "signerId": signer, "owner": "pam",
                "versions": {
                    "1.0.0": {
                        "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                        "services": { "": "b1" }, "installedBy": "pam", "installedAt": 0,
                        "upgrades": upgrades
                    },
                    "1.1.0": {
                        "version": "1.1.0", "mpk": "01", "mpkSize": 1, "source": "upload",
                        "services": { "": "b2" }, "installedBy": "pam", "installedAt": 0,
                        "upgrades": upgrades
                    }
                }
            }))
            .unwrap(),
        );
        for (id, bundle) in [("inst", "b1"), ("rawapp", "raw")] {
            let _prev = m.state.apps.insert(
                id.into(),
                serde_json::from_value(json!({
                    "appId": id, "name": id, "group": 2, "bundle": bundle, "owner": "cora",
                    "org": "cust", "access": { "kind": "org" }, "replicas": [1], "createdAt": 0
                }))
                .unwrap(),
            );
        }
        m
    }

    /// A package signing key bound to an organization makes it the
    /// publisher: its approved releases then govern every instance of the
    /// key's packages, its customers' included (a raw bundle stays its
    /// owner's). The binding is the key's own signature over this cluster,
    /// the organization and the key's generation, checked at apply, proposed
    /// by an `APPROVE_RELEASES` holder there; binding again takes a new
    /// signature, and an old one is never replayed.
    #[test]
    fn a_publisher_approves_the_releases_of_every_instance_of_its_packages() {
        use crate::types::caps::APPROVE_RELEASES;
        let (key, signer) = publisher_key(7);
        let (other, _) = publisher_key(8);
        let mut m = publishing(&signer, "owner");
        let inst = |m: &SystemMachine| m.state.apps["inst"].clone();
        let rawapp = |m: &SystemMachine| m.state.apps["rawapp"].clone();
        assert_eq!(
            m.state.approved_releases(&inst(&m)),
            Some(&m.state.orgs["cust"].approved_releases),
            "unbound: the owner's set"
        );

        // Forged or misplaced bindings are refused at apply.
        for (cmd, why) in [
            (
                bind(&other, &signer, ("pub", "pub"), 0, "pam"),
                "another key",
            ),
            (
                bind(&key, &signer, ("pub", "cust"), 0, "pam"),
                "signed for another org",
            ),
            (
                bind(&key, &signer, ("pub", "pub"), 1, "pam"),
                "a generation ahead",
            ),
            (
                bind(&key, &signer, ("pub", "pub"), 0, "bob"),
                "a member without the capability",
            ),
            (
                bind(&key, &signer, ("pub", "pub"), 0, "cora"),
                "an outsider",
            ),
        ] {
            assert!(m.run_unproven(cmd).is_err(), "{why}");
        }
        let mut cluster = bind(&key, &signer, ("pub", "pub"), 0, "pam");
        if let SystemCmd::BindPublisher { cluster: c, .. } = &mut cluster {
            *c = "another".into();
        }
        assert!(
            m.run_unproven(cluster).is_err(),
            "signed for another cluster"
        );
        assert!(m.state.publishers.is_empty());

        // A member granted APPROVE_RELEASES binds it.
        if let Some(o) = m.state.orgs.get_mut("pub") {
            let _prev = o.capabilities.insert("bob".into(), APPROVE_RELEASES);
        }
        let _ok = m
            .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "bob"))
            .unwrap();
        assert_eq!(m.state.publisher_of("b1"), Some(&"pub".to_owned()));
        assert_eq!(m.state.binding_generation(&signer), 1);
        assert_eq!(
            m.state.approved_releases(&inst(&m)),
            Some(&m.state.orgs["pub"].approved_releases)
        );
        assert_eq!(
            m.state.approved_releases(&rawapp(&m)),
            Some(&m.state.orgs["cust"].approved_releases),
            "a raw bundle stays its owner's"
        );

        // The customer approving a release changes nothing for the instance;
        // the publisher approving it does.
        let _ok = m.run_unproven(approve("cust", "2.1.0", "cora")).unwrap();
        assert!(!m.state.release_may_hold(&inst(&m), Some("2.1.0")));
        assert!(m.state.release_may_hold(&rawapp(&m), Some("2.1.0")));
        let _ok = m.run_unproven(approve("pub", "2.1.0", "pam")).unwrap();
        assert!(m.state.release_may_hold(&inst(&m), Some("2.1.0")));

        // Deadlines judge the instance on its publisher's set.
        let _ok = m
            .run_unproven(SystemCmd::SetReleaseDeadline {
                version: "2.2.0".into(),
                after: Some(10),
                proof: serde_json::Value::Null,
            })
            .unwrap();
        let _ok = m.run_unproven(approve("cust", "2.2.0", "cora")).unwrap();
        let create = |id: &str, bundle: &str| SystemCmd::CreateApp {
            app: Box::new(
                serde_json::from_value(json!({
                    "appId": id, "name": id, "group": 0, "bundle": bundle, "owner": "cora",
                    "org": "cust", "access": { "kind": "org" }, "replicas": [1], "createdAt": 10
                }))
                .unwrap(),
            ),
        };
        let e = m.run_unproven(create("second", "b1")).unwrap_err();
        assert!(e.starts_with(super::DEADLINE_PREFIX), "{e}");
        let _ok = m.run_unproven(create("raw2", "raw")).unwrap();
        let _ok = m.run_unproven(approve("pub", "2.2.0", "pam")).unwrap();
        let _ok = m.run_unproven(create("second", "b1")).unwrap();

        // Rebinding: the old signature is spent; the key signs the next
        // generation for the organization it moves to, and the one it
        // leaves cannot keep it.
        assert!(m
            .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
            .is_err());
        assert!(m
            .run_unproven(bind(&key, &signer, ("cust", "cust"), 0, "cora"))
            .is_err());
        let _ok = m
            .run_unproven(bind(&key, &signer, ("cust", "cust"), 1, "cora"))
            .unwrap();
        assert_eq!(m.state.publisher_of("b1"), Some(&"cust".to_owned()));
        assert_eq!(m.state.binding_generation(&signer), 2);
    }

    /// An upgrade key's upgrade is judged where it applies, on fresh state:
    /// the key's own signature, spent once; refused once its account may no
    /// longer upgrade the app (a member who lost `MANAGE_APPLICATION`, or
    /// left), once the key is revoked or expired, and for a bundle of another
    /// package or of its package reinstalled under another signer.
    #[test]
    fn an_upgrade_keys_upgrade_holds_only_while_its_maker_may_upgrade() {
        use crate::types::caps::MANAGE_APPLICATION;
        let (_key, signer) = publisher_key(9);
        let mut m = publishing(&signer, "owner");
        if let Some(o) = m.state.orgs.get_mut("cust") {
            let _prev = o.capabilities.insert("dan".into(), MANAGE_APPLICATION);
        }
        let secret = "cal_ci-secret";
        let record = |expires_at: u64| super::ApiKeyRecord {
            key_id: "k1".into(),
            account: "dan".into(),
            label: "ci".into(),
            apps: None,
            methods: None,
            read_only: false,
            allow_confirm: false,
            expires_at,
            upgrades: Some(crate::types::UpgradeScope {
                app: "inst".into(),
                package: "p".into(),
                signer: signer.clone(),
                proof_key: crate::proof::api_key_public(secret),
            }),
        };
        let r = record(100);
        assert_eq!(
            m.state.upgrade_key_fits(&r, r.upgrades.as_ref().unwrap()),
            Ok(())
        );
        assert!(
            m.state
                .upgrade_key_fits(
                    &record(100),
                    &crate::types::UpgradeScope {
                        app: "rawapp".into(),
                        ..r.upgrades.clone().unwrap()
                    }
                )
                .is_err(),
            "an app that runs no release of the package"
        );
        let _prev = m.state.api_keys.insert("h1".into(), r);
        let upgrade = |bundle: &str, at: u64, secret: &str| {
            let statement = crate::proof::Statement::new(
                "inst",
                "__upgrade",
                &json!({ "bundle": bundle, "migrate": null, "manifest": null }),
            );
            SystemCmd::UpgradeApp {
                app_id: "inst".into(),
                bundle: bundle.into(),
                compute_bundle: None,
                by_operator: false,
                actor: "dan".into(),
                at,
                run: None,
                consent: json!(crate::proof::sign_key_upgrade(secret, "k1", statement)),
                release: None,
                owner_in_org: true,
                owner_in_team: true,
            }
        };
        let e = m.run(upgrade("b2", 10, "cal_other")).unwrap_err();
        assert!(e.contains("not signed with the API key"), "{e}");
        let e = m.run(upgrade("raw", 10, secret)).unwrap_err();
        assert!(e.contains("only to signed releases"), "{e}");
        let e = m.run(upgrade("b2", 100, secret)).unwrap_err();
        assert!(e.contains("expired"), "{e}");
        let once = upgrade("b2", 10, secret);
        let _ok = m.run(once.clone()).unwrap();
        assert_eq!(m.state.apps["inst"].bundle, "b2");
        assert_eq!(
            m.state.apps["inst"]
                .last_upgrade
                .as_ref()
                .and_then(|u| u.api_key.as_deref()),
            Some("k1")
        );
        let e = m.run(once).unwrap_err();
        assert!(e.contains("used already"), "{e}");

        // Its maker loses the right to upgrade: so does the key.
        if let Some(o) = m.state.orgs.get_mut("cust") {
            let _prev = o.capabilities.insert("dan".into(), 0);
        }
        assert!(!m.state.may_upgrade(&m.state.apps["inst"], "dan"));
        assert!(m.run(upgrade("b1", 10, secret)).is_err(), "descoped");
        if let Some(o) = m.state.orgs.get_mut("cust") {
            let _prev = o.capabilities.insert("dan".into(), MANAGE_APPLICATION);
        }
        let _ok = m.run(upgrade("b1", 11, secret)).unwrap();

        // The package under another signer, then the key revoked.
        let (_other, other_signer) = publisher_key(10);
        if let Some(p) = m.state.packages.get_mut("p") {
            p.signer_id = other_signer;
        }
        let e = m.run(upgrade("b2", 12, secret)).unwrap_err();
        assert!(e.contains("signer it names"), "{e}");
        if let Some(p) = m.state.packages.get_mut("p") {
            p.signer_id = signer.clone();
        }
        let _ok = m
            .run_unproven(SystemCmd::RevokeApiKey {
                key_id: "k1".into(),
                actor: "dan".into(),
                consent: serde_json::Value::Null,
            })
            .unwrap();
        let e = m.run(upgrade("b2", 13, secret)).unwrap_err();
        assert!(e.contains("revoked"), "{e}");
    }

    /// A package's manifest says who upgrades its instances. `publisher`:
    /// the publisher's `MANAGE_APPLICATION` holders, for every instance, and
    /// not the customer; until its key is bound, nobody. The
    /// declaration only loosens toward owners, at install and at upgrade.
    #[test]
    fn a_package_says_whether_its_publisher_or_each_owner_upgrades_it() {
        use super::PackageVersion;
        use crate::types::caps::MANAGE_APPLICATION;
        let (key, signer) = publisher_key(9);
        let mut m = publishing(&signer, "publisher");
        if let Some(o) = m.state.orgs.get_mut("pub") {
            let _prev = o.capabilities.insert("bob".into(), MANAGE_APPLICATION);
        }
        if let Some(o) = m.state.orgs.get_mut("cust") {
            let _prev = o.capabilities.insert("dan".into(), MANAGE_APPLICATION);
        }
        let inst = |m: &SystemMachine| m.state.apps["inst"].clone();
        let may = |m: &SystemMachine, who: &str| m.state.may_upgrade(&inst(m), who);

        // Unbound: nobody upgrades a `publisher` package's instances yet.
        for who in ["pam", "bob", "cora", "dan"] {
            assert!(!may(&m, who), "{who}");
        }

        let _ok = m
            .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
            .unwrap();
        for (who, expected) in [
            ("pam", true),
            ("bob", true),
            ("cora", false),
            ("dan", false),
        ] {
            assert_eq!(may(&m, who), expected, "{who}");
        }
        let upgrade = |actor: &str, bundle: &str| SystemCmd::UpgradeApp {
            compute_bundle: None,
            app_id: "inst".into(),
            bundle: bundle.into(),
            by_operator: false,
            actor: actor.into(),
            at: 0,
            run: None,
            consent: serde_json::Value::Null,
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        };
        assert!(
            m.run_unproven(upgrade("cora", "b2")).is_err(),
            "the customer may not"
        );
        let _ok = m.run_unproven(upgrade("bob", "b2")).unwrap();

        // Releases: publisher first, then owner, never back.
        let install = |version: &str, upgrades: &str| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: signer.clone(),
            release: serde_json::from_value::<PackageVersion>(json!({
                "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
                "services": { "": format!("h{version}") }, "installedBy": "pam",
                "installedAt": 0, "upgrades": upgrades
            }))
            .unwrap(),
        };
        let _ok = m.run_unproven(install("1.2.0", "owner")).unwrap();
        assert!(
            m.run_unproven(install("1.3.0", "publisher")).is_err(),
            "back to the publisher"
        );
        assert!(
            m.run_unproven(install("1.1.5", "owner")).is_ok(),
            "owner between them"
        );
        assert!(
            m.run_unproven(install("0.9.0", "owner")).is_err(),
            "owner below a publisher release"
        );
        let _ok = m.run_unproven(install("0.8.0", "publisher")).unwrap();

        // The instance moves to the owner release, by its publisher; then it
        // is its owner's, and never goes back.
        let _prev = m.state.bundles.insert(
            "h1.2.0".into(),
            serde_json::from_value(json!({
                "hash": "h1.2.0", "size": 1, "uploadedBy": "pam",
                "package": { "package": "p", "version": "1.2.0" }
            }))
            .unwrap(),
        );
        let _ok = m.run_unproven(upgrade("pam", "h1.2.0")).unwrap();
        assert!(may(&m, "cora") && !may(&m, "pam"));
        assert!(
            m.run_unproven(upgrade("cora", "b1")).is_err(),
            "never back to the publisher"
        );
        let _ok = m.run_unproven(upgrade("cora", "h1.2.0")).unwrap();
    }

    // ------------------------------------------------------------ deployers

    /// A release of package `p` (`version`, its WASM `b-<version>`)
    /// installed by `by` at `at`.
    fn install_at(by: &str, version: &str, at: u64) -> SystemCmd {
        SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: serde_json::from_value(json!({
                "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
                "services": { "": format!("b-{version}") }, "installedBy": by, "installedAt": at
            }))
            .unwrap(),
        }
    }

    fn raw_bundle(actor: &str, hash: &str, at: u64) -> SystemCmd {
        SystemCmd::PutBundle {
            hash: hash.into(),
            size: 1,
            actor: actor.into(),
            at,
        }
    }

    fn grant(deployer: super::Deployer, until: Option<u64>, actor: &str) -> SystemCmd {
        SystemCmd::GrantDeployer {
            deployer,
            until,
            at: 1,
            actor: actor.into(),
            proof: None,
            consent: None,
        }
    }

    fn account_deployer(id: &str) -> super::Deployer {
        super::Deployer::Account(id.into())
    }

    /// A cluster deployed with `deployments`, alice, bob and carol, and
    /// organization `org` where bob is a plain member and carol holds
    /// `MANAGE_APPLICATION` organization-wide.
    fn deploying(deployments: super::Deployments) -> SystemMachine {
        let mut m = SystemMachine::default();
        let _ok = m
            .run_unproven(genesis(Governance {
                deployments,
                ..Governance::default()
            }))
            .unwrap();
        for who in ["alice", "bob", "carol"] {
            let _ok = m.run_unproven(account(who)).unwrap();
        }
        let o = org(
            &[("alice", "owner"), ("bob", "member"), ("carol", "member")],
            &[("carol", crate::types::caps::MANAGE_APPLICATION)],
            &[],
        );
        let _prev = m.state.orgs.insert("org".into(), o);
        m
    }

    /// Where deployments are granted, an account installs a package or
    /// uploads a raw bundle only with a live deployer grant, judged at the
    /// install's own time; an organization's grant reaches only its
    /// `MANAGE_APPLICATION` holders; the operator always installs. Revoking
    /// stops new installs and keeps what is installed.
    #[test]
    fn on_a_granted_cluster_only_deployers_install() {
        use super::{Deployer, Deployments, NOT_A_DEPLOYER, OPERATOR};
        const T: u64 = 1_000;
        let mut m = deploying(Deployments::Granted);
        assert_eq!(
            m.run_unproven(install_at("alice", "1.0.0", 10))
                .unwrap_err(),
            NOT_A_DEPLOYER
        );
        assert_eq!(
            m.run_unproven(raw_bundle("alice", "raw", 10)).unwrap_err(),
            NOT_A_DEPLOYER
        );
        let _ok = m.run_unproven(install_at(OPERATOR, "0.9.0", 10)).unwrap();

        // An account's own grant, until T: judged at the install's time.
        let _ok = m
            .run_unproven(grant(account_deployer("alice"), Some(T), OPERATOR))
            .unwrap();
        let _ok = m.run_unproven(install_at("alice", "1.0.0", T - 1)).unwrap();
        let _ok = m.run_unproven(raw_bundle("alice", "raw", T - 1)).unwrap();
        assert_eq!(
            m.run_unproven(install_at("alice", "1.1.0", T)).unwrap_err(),
            NOT_A_DEPLOYER,
            "past `until`"
        );
        assert!(
            m.run_unproven(install_at("bob", "1.0.0", 10)).is_err(),
            "the same release again is still an install"
        );

        // An organization's grant: its MANAGE_APPLICATION holders only.
        let _ok = m
            .run_unproven(grant(Deployer::Org("org".into()), None, OPERATOR))
            .unwrap();
        let _ok = m.run_unproven(install_at("carol", "2.0.0", T)).unwrap();
        let _ok = m.run_unproven(install_at("alice", "2.1.0", T)).unwrap();
        assert_eq!(
            m.run_unproven(install_at("bob", "2.2.0", T)).unwrap_err(),
            NOT_A_DEPLOYER,
            "a plain member"
        );

        // Revoked: nothing new, and what is installed stays.
        let _ok = m
            .run_unproven(SystemCmd::RevokeDeployer {
                deployer: Deployer::Org("org".into()),
                at: T,
                actor: OPERATOR.into(),
                proof: None,
                consent: None,
            })
            .unwrap();
        assert!(m.run_unproven(install_at("carol", "2.3.0", T)).is_err());
        assert!(m.state.packages["p"].versions.contains_key("2.0.0"));

        // Grants name an account or organization that exists, end after
        // they are made, and only the operator makes them here.
        for (why, cmd) in [
            ("unknown", grant(account_deployer("nobody"), None, OPERATOR)),
            (
                "already over",
                grant(account_deployer("bob"), Some(1), OPERATOR),
            ),
            (
                "not the operator",
                grant(account_deployer("bob"), None, "alice"),
            ),
        ] {
            assert!(m.run_unproven(cmd).is_err(), "{why}");
        }
    }

    /// Where deployments are open, installs go on exactly as before.
    #[test]
    fn on_an_open_cluster_anyone_installs() {
        let mut m = deploying(super::Deployments::Open);
        let _ok = m.run_unproven(install_at("bob", "1.0.0", 10)).unwrap();
        let _ok = m.run_unproven(raw_bundle("bob", "raw", 10)).unwrap();
        assert!(m.state.deployers.is_empty());
        let json = serde_json::to_value(&m.state).unwrap();
        assert!(json.get("deployers").is_none() && json.get("clusterOwner").is_none());
    }

    /// The operator moves deployments from open to granted once; never back,
    /// and never twice.
    #[test]
    fn deployments_are_granted_one_way() {
        use super::Deployments;
        let mut m = deploying(Deployments::Open);
        let _ok = m
            .run_unproven(SystemCmd::SetDeploymentsGranted {
                proof: serde_json::Value::Null,
            })
            .unwrap();
        assert_eq!(m.state.governance().deployments, Deployments::Granted);
        assert!(m
            .run_unproven(SystemCmd::SetDeploymentsGranted {
                proof: serde_json::Value::Null,
            })
            .is_err());
        assert_eq!(
            m.run_unproven(install_at("bob", "1.0.0", 10)).unwrap_err(),
            super::NOT_A_DEPLOYER
        );
    }

    /// The operator's key in tests, and its signature over a statement.
    fn operator_signed(subject: &str, method: &str, args: &serde_json::Value) -> serde_json::Value {
        use ed25519_dalek::Signer;
        let statement = crate::proof::Statement::new(subject, method, args);
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let signature = key.sign(&statement.operator_message("c"));
        serde_json::to_value(crate::proof::OperatorProof {
            statement,
            signature: hex::encode(signature.to_bytes()),
        })
        .unwrap()
    }

    /// Where the cluster's owner grants deployers: the operator grants them
    /// until it names the owner, once; from then on the owner's passkey
    /// grants them and the operator's signature does not. Every grant carries
    /// its proof over exactly that grant, used once.
    #[test]
    fn deployer_grants_carry_the_operators_signature_then_the_owners_passkey() {
        use super::{Deployers, Deployments, OPERATOR};
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let mut m = SystemMachine::default();
        let _ok = m
            .run(SystemCmd::Genesis {
                session_proofs: true,
                governance: Governance {
                    deployments: Deployments::Granted,
                    deployers: Deployers::Owner,
                    ..Governance::default()
                },
                release: Some("1.0.0".into()),
                operator_key: Some(crate::proof::OperatorKey {
                    key: hex::encode(key.verifying_key().to_bytes()),
                    cluster: "c".into(),
                }),
                nodes: Vec::new(),
            })
            .unwrap();
        let mut alice = calimero_identity::testing::SoftAuthenticator::new(
            "localhost",
            "http://localhost:3000",
        );
        let mut bob = calimero_identity::testing::SoftAuthenticator::new(
            "localhost",
            "http://localhost:3000",
        );
        let _ok = m.run(account_with("alice", &mut alice)).unwrap();
        let _ok = m.run(account_with("bob", &mut bob)).unwrap();
        let args = |who: &str| json!({ "deployer": { "account": who }, "until": null });
        let by_operator = |who: &str, proof: serde_json::Value| SystemCmd::GrantDeployer {
            deployer: account_deployer(who),
            until: None,
            at: 1,
            actor: OPERATOR.into(),
            proof: Some(proof),
            consent: None,
        };

        // Before the owner is named, the operator grants, with its key.
        let other = {
            use ed25519_dalek::Signer;
            let statement =
                crate::proof::Statement::new("cluster", "__grant_deployer", &args("bob"));
            let signature = ed25519_dalek::SigningKey::from_bytes(&[8; 32])
                .sign(&statement.operator_message("c"));
            serde_json::to_value(crate::proof::OperatorProof {
                statement,
                signature: hex::encode(signature.to_bytes()),
            })
            .unwrap()
        };
        for (why, cmd) in [
            ("another key", by_operator("bob", other)),
            (
                "over another deployer",
                by_operator(
                    "bob",
                    operator_signed("cluster", "__grant_deployer", &args("alice")),
                ),
            ),
            (
                "a revocation's signature",
                by_operator(
                    "bob",
                    operator_signed("cluster", "__revoke_deployer", &args("bob")),
                ),
            ),
            (
                "a passkey instead",
                SystemCmd::GrantDeployer {
                    deployer: account_deployer("bob"),
                    until: None,
                    at: 1,
                    actor: "alice".into(),
                    proof: None,
                    consent: Some(consent(
                        &mut alice,
                        "cluster",
                        "__grant_deployer",
                        &args("bob"),
                    )),
                },
            ),
        ] {
            assert!(m.run(cmd).is_err(), "{why}");
        }
        assert!(m.state.deployers.is_empty());
        let signed = operator_signed("cluster", "__grant_deployer", &args("bob"));
        let _ok = m.run(by_operator("bob", signed.clone())).unwrap();
        assert!(m.state.may_deploy("bob", 5));
        let _ok = m
            .run(SystemCmd::RevokeDeployer {
                deployer: account_deployer("bob"),
                at: 2,
                actor: OPERATOR.into(),
                proof: Some(operator_signed(
                    "cluster",
                    "__revoke_deployer",
                    &json!({ "deployer": { "account": "bob" } }),
                )),
                consent: None,
            })
            .unwrap();
        assert_eq!(
            m.run(by_operator("bob", signed)).unwrap_err(),
            "that consent was used already",
            "a grant replayed after its revocation"
        );
        assert!(!m.state.may_deploy("bob", 5));

        // The operator names alice the owner, once.
        let owner = |who: &str| SystemCmd::SetClusterOwner {
            owner: account_deployer(who),
            proof: operator_signed(
                "cluster",
                "__cluster_owner",
                &json!({ "owner": { "account": who } }),
            ),
        };
        assert!(m
            .run(SystemCmd::SetClusterOwner {
                owner: account_deployer("alice"),
                proof: serde_json::Value::Null,
            })
            .is_err());
        let _ok = m.run(owner("alice")).unwrap();
        assert!(m.run(owner("bob")).is_err(), "a second owner");
        assert_eq!(m.state.cluster_owner, Some(account_deployer("alice")));

        // Now the operator's signature grants nothing; alice's passkey does,
        // and bob's does not.
        let fresh_operator = operator_signed(
            "cluster",
            "__grant_deployer",
            &json!({ "deployer": { "account": "bob" }, "until": 9 }),
        );
        assert!(m
            .run(SystemCmd::GrantDeployer {
                deployer: account_deployer("bob"),
                until: Some(9),
                at: 1,
                actor: OPERATOR.into(),
                proof: Some(fresh_operator),
                consent: None,
            })
            .is_err());
        let by_owner = |actor: &str, pk: &mut calimero_identity::testing::SoftAuthenticator| {
            SystemCmd::GrantDeployer {
                deployer: account_deployer("bob"),
                until: None,
                at: 1,
                actor: actor.into(),
                proof: None,
                consent: Some(consent(pk, "cluster", "__grant_deployer", &args("bob"))),
            }
        };
        assert!(m.run(by_owner("bob", &mut bob)).is_err(), "not the owner");
        assert!(!m.state.may_deploy("bob", 5));
        let _ok = m.run(by_owner("alice", &mut alice)).unwrap();
        assert!(m.state.may_deploy("bob", 5));
        assert_eq!(m.state.deployers["account:bob"].granted_by, "alice");
    }

    /// A deployer's JSON names its kind, and its key names it back.
    #[test]
    fn a_deployer_reads_from_its_key() {
        use super::Deployer;
        let d = Deployer::Org("o1".into());
        assert_eq!(serde_json::to_value(&d).unwrap(), json!({ "org": "o1" }));
        assert_eq!(Deployer::from_key(&d.key()), Some(d));
        assert_eq!(Deployer::from_key("team:x"), None);
    }

    // ------------------------------------------------------------ deployments

    fn create_deployment(id: &str, owner: super::Deployer, actor: &str) -> SystemCmd {
        SystemCmd::CreateDeployment {
            id: id.into(),
            name: id.into(),
            owner,
            actor: actor.into(),
            at: 10,
            consent: serde_json::Value::Null,
        }
    }

    fn offer_app(deployment: &str, app: &str, version: &str, actor: &str) -> SystemCmd {
        SystemCmd::OfferApp {
            deployment: deployment.into(),
            app: app.into(),
            package: "p".into(),
            version: version.into(),
            actor: actor.into(),
            at: 10,
            consent: serde_json::Value::Null,
        }
    }

    fn withdraw_app(deployment: &str, app: &str, actor: &str) -> SystemCmd {
        SystemCmd::WithdrawApp {
            deployment: deployment.into(),
            app: app.into(),
            actor: actor.into(),
            consent: serde_json::Value::Null,
        }
    }

    fn delete_deployment(deployment: &str, actor: &str) -> SystemCmd {
        SystemCmd::DeleteDeployment {
            deployment: deployment.into(),
            actor: actor.into(),
            consent: serde_json::Value::Null,
        }
    }

    /// An instance `id` of `owner`'s running `bundle`, from `deployment`.
    fn instance(id: &str, owner: &str, bundle: &str, deployment: Option<&str>) -> SystemCmd {
        SystemCmd::CreateApp {
            app: Box::new(
                serde_json::from_value(json!({
                    "appId": id, "name": id, "group": 0, "bundle": bundle, "owner": owner,
                    "replicas": [1], "createdAt": 10, "deployment": deployment,
                }))
                .unwrap(),
            ),
        }
    }

    /// An account's deployment is its own; an organization's is its
    /// org-wide `MANAGE_APPLICATION` holders'. Offers name installed
    /// releases the manager sees; an instance from a deployment runs exactly
    /// an offered release; a withdraw waits for the deployment's own
    /// instances; an offered release is not uninstalled; a deployment is
    /// deleted once it offers nothing and no instance came from it.
    #[test]
    fn deployments_are_their_managers_and_keep_what_they_offer() {
        use super::{Deployer, Deployments, MAX_DEPLOYMENTS_PER_OWNER, QUOTA_PREFIX};
        let mut m = deploying(Deployments::Open);
        let alice = || Deployer::Account("alice".into());
        let org = || Deployer::Org("org".into());
        // alice, alone, for herself; carol (MANAGE_APPLICATION) for the org.
        assert!(m
            .run_unproven(create_deployment("a", alice(), "bob"))
            .is_err());
        let _ok = m
            .run_unproven(create_deployment("a", alice(), "alice"))
            .unwrap();
        assert!(m
            .run_unproven(create_deployment("o", org(), "bob"))
            .is_err());
        let _ok = m
            .run_unproven(create_deployment("o", org(), "carol"))
            .unwrap();
        assert!(
            m.run_unproven(create_deployment("o", org(), "alice"))
                .is_err(),
            "exists"
        );
        let bad_name = SystemCmd::CreateDeployment {
            id: "x".into(),
            name: " padded ".into(),
            owner: alice(),
            actor: "alice".into(),
            at: 10,
            consent: serde_json::Value::Null,
        };
        assert!(m.run_unproven(bad_name).is_err());
        assert!(m
            .run_unproven(create_deployment("-x", alice(), "alice"))
            .is_err());

        // Only installed releases, seen by the manager.
        assert!(m
            .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
            .is_err());
        let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
        let _ok = m.run_unproven(install_at("alice", "2.0.0", 5)).unwrap();
        let _ok = m
            .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
            .unwrap();
        assert!(m
            .run_unproven(offer_app("a", "notes", "1.0.0", "bob"))
            .is_err());
        assert!(m
            .run_unproven(offer_app("a", "../x", "1.0.0", "alice"))
            .is_err());
        // bob is a plain member; carol shares the org with alice, so sees it.
        assert!(m
            .run_unproven(offer_app("o", "notes", "1.0.0", "bob"))
            .is_err());
        let _ok = m
            .run_unproven(offer_app("o", "notes", "1.0.0", "carol"))
            .unwrap();
        let _ok = m
            .run_unproven(offer_app("o", "notes", "1.0.0", "alice"))
            .unwrap();
        // An entry keeps its package.
        let mut other = offer_app("a", "notes", "1.0.0", "alice");
        if let SystemCmd::OfferApp { package, .. } = &mut other {
            *package = "q".into();
        }
        assert!(m.run_unproven(other).is_err());

        // Instances: only the offered release, of a deployment that exists.
        let _ok = m
            .run_unproven(instance("i1", "bob", "b-1.0.0", Some("a")))
            .unwrap();
        assert!(m
            .run_unproven(instance("i2", "bob", "b-2.0.0", Some("a")))
            .is_err());
        assert!(m
            .run_unproven(instance("i3", "bob", "b-1.0.0", Some("zz")))
            .is_err());
        let _ok = m
            .run_unproven(instance("i4", "bob", "b-2.0.0", None))
            .unwrap();

        // Withdraw: refused while an instance from this deployment runs the
        // package; the other deployment's entry is its own.
        assert!(m.run_unproven(withdraw_app("a", "notes", "alice")).is_err());
        let _ok = m.run_unproven(withdraw_app("o", "notes", "carol")).unwrap();
        // Offered by `a`: not uninstalled, even by its first installer; a
        // release nobody offers keeps today's rules.
        let uninstall = |v: &str| SystemCmd::UninstallPackage {
            package: "p".into(),
            version: Some(v.into()),
            actor: "alice".into(),
        };
        assert!(m.run_unproven(uninstall("1.0.0")).is_err());
        let _gone = m.state.apps.remove("i1");
        assert!(
            m.run_unproven(delete_deployment("a", "alice")).is_err(),
            "it offers notes"
        );
        let _ok = m.run_unproven(withdraw_app("a", "notes", "alice")).unwrap();
        let _ok = m.run_unproven(uninstall("1.0.0")).unwrap();
        assert!(m.run_unproven(delete_deployment("a", "bob")).is_err());
        let _ok = m.run_unproven(delete_deployment("a", "alice")).unwrap();
        assert!(!m.state.deployments.contains_key("a"));

        // An organization owning a deployment is not deleted under it.
        let delete_org = SystemCmd::DeleteOrg {
            org: "org".into(),
            actor: "alice".into(),
            consent: serde_json::Value::Null,
        };
        assert!(m.run_unproven(delete_org).is_err());

        // A bounded number per owner.
        for i in 1..MAX_DEPLOYMENTS_PER_OWNER {
            let _ok = m
                .run_unproven(create_deployment(&format!("o{i}"), org(), "carol"))
                .unwrap();
        }
        let e = m
            .run_unproven(create_deployment("one-more", org(), "carol"))
            .unwrap_err();
        assert!(e.starts_with(QUOTA_PREFIX), "{e}");
    }

    /// Where deployments are granted, creating one and offering need a live
    /// grant at the command's time; withdrawing does not.
    #[test]
    fn on_a_granted_cluster_deploying_needs_a_grant() {
        use super::{Deployer, Deployments, NOT_A_DEPLOYER, OPERATOR};
        let mut m = deploying(Deployments::Granted);
        let alice = || Deployer::Account("alice".into());
        assert_eq!(
            m.run_unproven(create_deployment("a", alice(), "alice"))
                .unwrap_err(),
            NOT_A_DEPLOYER
        );
        let _ok = m.run_unproven(grant(alice(), None, OPERATOR)).unwrap();
        let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
        let _ok = m
            .run_unproven(create_deployment("a", alice(), "alice"))
            .unwrap();
        let _ok = m
            .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
            .unwrap();
        // A customer without a grant creates from the offer.
        let _ok = m
            .run_unproven(instance("i1", "bob", "b-1.0.0", Some("a")))
            .unwrap();
        let _ok = m
            .run_unproven(SystemCmd::RevokeDeployer {
                deployer: alice(),
                at: 20,
                actor: OPERATOR.into(),
                proof: None,
                consent: None,
            })
            .unwrap();
        assert_eq!(
            m.run_unproven(offer_app("a", "more", "1.0.0", "alice"))
                .unwrap_err(),
            NOT_A_DEPLOYER
        );
        let _gone = m.state.apps.remove("i1");
        let _ok = m.run_unproven(withdraw_app("a", "notes", "alice")).unwrap();
    }

    /// Each change to a deployment carries its actor's passkey consent over
    /// exactly that change, once; a deployment's id is its creating
    /// consent's, so no node picks another.
    #[test]
    fn deployment_changes_carry_their_managers_consent() {
        use super::{deployment_id, Deployer};
        let (mut m, mut alice, mut bob) = two_accounts();
        let owner = json!({ "account": "alice" });
        let args = json!({ "name": "labs", "owner": owner });
        let create =
            |id: String, actor: &str, consent: serde_json::Value| SystemCmd::CreateDeployment {
                id,
                name: "labs".into(),
                owner: Deployer::Account("alice".into()),
                actor: actor.into(),
                at: 10,
                consent,
            };
        assert!(m
            .run(create("d".into(), "alice", serde_json::Value::Null))
            .is_err());
        let c = consent(&mut alice, "cluster", "__create_deployment", &args);
        assert!(
            m.run(create("picked".into(), "alice", c.clone())).is_err(),
            "an id the consent does not name"
        );
        let id = deployment_id(c["nonce"].as_str().unwrap());
        let theirs = consent(&mut bob, "cluster", "__create_deployment", &args);
        let bobs_id = deployment_id(theirs["nonce"].as_str().unwrap());
        assert!(
            m.run(create(bobs_id, "alice", theirs)).is_err(),
            "bob's passkey"
        );
        let _ok = m.run(create(id.clone(), "alice", c.clone())).unwrap();
        assert!(m.run(create(id.clone(), "alice", c)).is_err(), "used once");

        let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
        let subject = format!("deployment:{id}");
        let offer = |version: &str, consent: serde_json::Value| SystemCmd::OfferApp {
            deployment: id.clone(),
            app: "notes".into(),
            package: "p".into(),
            version: version.into(),
            actor: "alice".into(),
            at: 10,
            consent,
        };
        let for_1 = json!({ "app": "notes", "package": "p", "version": "1.0.0" });
        let c = consent(&mut alice, &subject, "__offer_app", &for_1);
        assert!(m.run(offer("2.0.0", c.clone())).is_err(), "another version");
        let _ok = m.run(offer("1.0.0", c)).unwrap();
        let withdraw = |consent: serde_json::Value| SystemCmd::WithdrawApp {
            deployment: id.clone(),
            app: "notes".into(),
            actor: "alice".into(),
            consent,
        };
        let c = consent(
            &mut alice,
            "deployment:other",
            "__withdraw_app",
            &json!({ "app": "notes" }),
        );
        assert!(m.run(withdraw(c)).is_err(), "another deployment");
        let c = consent(
            &mut alice,
            &subject,
            "__withdraw_app",
            &json!({ "app": "notes" }),
        );
        let _ok = m.run(withdraw(c)).unwrap();
        let c = consent(&mut alice, &subject, "__delete_deployment", &json!({}));
        let _ok = m
            .run(SystemCmd::DeleteDeployment {
                deployment: id.clone(),
                actor: "alice".into(),
                consent: c,
            })
            .unwrap();
        assert!(m.state.deployments.is_empty());
    }

    /// A system state and an app record written before deployments (log
    /// entries and snapshots are JSON) read with none, and write back as
    /// they were.
    #[test]
    fn state_from_before_deployments_reads_with_none_and_writes_as_before() {
        // `AppRecord` as the previous release wrote it, every field present.
        let written: crate::types::AppRecord = serde_json::from_value(json!({
            "appId": "a", "name": "n", "group": 2, "bundle": "b", "owner": "alice",
            "replicas": [1], "createdAt": 0,
        }))
        .unwrap();
        let app = serde_json::to_value(&written).unwrap();
        assert!(app.get("deployment").is_none(), "{app}");
        let record: crate::types::AppRecord = serde_json::from_value(app.clone()).unwrap();
        assert_eq!(record.deployment, None);
        assert_eq!(serde_json::to_value(&record).unwrap(), app);

        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        let _ok = m.run_authorized(account("alice")).unwrap();
        let _prev = m.state.apps.insert("a".into(), record);
        let before = serde_json::to_value(&m.state).unwrap();
        assert!(before.get("deployments").is_none(), "{before}");
        assert!(before["apps"]["a"].get("deployment").is_none());
        let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
        assert!(restored.deployments.is_empty());
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
    }

    /// An app of `org` (or a personal one) in a machine whose file quota is `quota`.
    fn machine_with_apps(quota: u64, apps: &[(&str, Option<&str>)]) -> SystemMachine {
        let mut m = SystemMachine::default();
        m.state.quotas.max_file_bytes_per_org = quota;
        for (id, org) in apps {
            let app = serde_json::from_value(json!({
                "appId": id, "name": id, "group": 2, "bundle": "b", "owner": "alice",
                "org": org, "replicas": [1], "createdAt": 0
            }))
            .unwrap();
            let _prev = m.state.apps.insert((*id).into(), app);
            if let Some(org) = org {
                let _prev = m.state.orgs.insert(
                    (*org).into(),
                    serde_json::from_value(json!({
                        "orgId": org, "name": org, "members": { "alice": "owner" }, "teams": {}
                    }))
                    .unwrap(),
                );
            }
        }
        m
    }

    fn charge(app: &str, delta: i64) -> SystemCmd {
        SystemCmd::ChargeFileBytes {
            app_id: app.into(),
            delta,
        }
    }

    #[test]
    fn file_bytes_are_granted_up_to_the_quota_across_the_payers_apps() {
        let mut m = machine_with_apps(100, &[("a", Some("org")), ("b", Some("org")), ("c", None)]);
        let _ok = m.run_unproven(charge("a", 60)).unwrap();
        let _ok = m.run_unproven(charge("b", 40)).unwrap();
        assert_eq!(m.state.file_bytes("org"), 100);

        let refused = m.run_unproven(charge("a", 1)).unwrap_err();
        assert!(refused.starts_with(QUOTA_PREFIX), "{refused}");
        assert_eq!(m.state.file_bytes("org"), 100, "a refusal changes nothing");

        // Another payer has its own quota.
        let _ok = m.run_unproven(charge("c", 100)).unwrap();
        assert_eq!(m.state.file_bytes("account:alice"), 100);
        assert!(m.run_unproven(charge("nobody", 1)).is_err());
    }

    #[test]
    fn a_refund_frees_room_and_stops_at_zero() {
        let mut m = machine_with_apps(100, &[("a", Some("org")), ("b", Some("org"))]);
        let _ok = m.run_unproven(charge("a", 100)).unwrap();
        assert!(m.run_unproven(charge("b", 1)).is_err());
        let _ok = m.run_unproven(charge("a", -30)).unwrap();
        let _ok = m.run_unproven(charge("b", 30)).unwrap();
        assert_eq!(m.state.file_bytes("org"), 100);

        let _ok = m.run_unproven(charge("a", -1_000)).unwrap();
        assert!(!m.state.app_file_bytes.contains_key("a"));
        assert_eq!(m.state.app_file_bytes["b"], 30, "another app's bytes stay");
    }

    #[test]
    fn deleting_an_app_or_an_org_frees_its_files() {
        let mut m = machine_with_apps(100, &[("a", Some("org")), ("b", Some("org")), ("c", None)]);
        for app in ["a", "b", "c"] {
            let _ok = m.run_unproven(charge(app, 10)).unwrap();
        }
        let _ok = m
            .run_unproven(SystemCmd::DeleteApp {
                app_id: "a".into(),
                actor: "alice".into(),
                owner_in_org: true,
                owner_in_team: true,
            })
            .unwrap();
        assert_eq!(m.state.file_bytes("org"), 10);
        assert!(!m.state.app_file_bytes.contains_key("a"));

        let _prev = m.state.orgs.insert(
            "org".into(),
            serde_json::from_value(json!({
                "orgId": "org", "name": "O", "members": { "alice": "owner" }, "teams": {}
            }))
            .unwrap(),
        );
        let _ok = m
            .run_unproven(SystemCmd::DeleteOrg {
                org: "org".into(),
                actor: "alice".into(),
                consent: serde_json::Value::Null,
            })
            .unwrap();
        assert_eq!(m.state.file_bytes("org"), 0);
        assert_eq!(m.state.app_file_bytes.len(), 1, "only the personal app's");
    }

    #[test]
    fn the_file_quota_defaults_to_fifty_gigabytes_and_reads_from_old_state() {
        assert_eq!(Quotas::default().max_file_bytes_per_org, 50_000_000_000);
        let old: Quotas = serde_json::from_value(json!({ "maxAccounts": 7 })).unwrap();
        assert_eq!(old.max_file_bytes_per_org, 50_000_000_000);
        assert_eq!(
            serde_json::to_value(&old).unwrap()["maxFileBytesPerOrg"],
            json!(50_000_000_000u64)
        );
        let old_state: SystemState = serde_json::from_value(json!({
            "nodes": {}, "apps": {}, "nextGroup": 1, "bundles": {}, "accounts": {},
            "credentials": {}, "apiKeys": {}, "refresh": {}, "revokedSessions": [],
            "sessionKeys": [], "orgs": {}, "invites": {}
        }))
        .unwrap();
        assert!(old_state.app_file_bytes.is_empty());
    }

    /// The per-payer totals equal the sum over the apps' bytes.
    fn assert_totals_are_sums(s: &SystemState) {
        let mut sums = std::collections::BTreeMap::new();
        for (id, bytes) in &s.app_file_bytes {
            let payer = crate::usage::payer(&s.apps[id]);
            *sums.entry(payer).or_insert(0u64) += bytes;
        }
        assert_eq!(s.payer_file_bytes, sums);
    }

    #[test]
    fn the_payer_totals_always_equal_the_sum_over_the_apps() {
        let mut m = machine_with_apps(
            1_000,
            &[("a", Some("org")), ("b", Some("org")), ("c", None)],
        );
        let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
        let _ok = m.run_unproven(account("bob")).unwrap();
        let steps = [
            charge("a", 100),
            charge("b", 50),
            charge("c", 70),
            charge("a", -30),
        ];
        for step in steps {
            let _ok = m.run_unproven(step).unwrap();
            assert_totals_are_sums(&m.state);
        }
        let _ok = m.run_unproven(charge("b", -500)).unwrap();
        assert_totals_are_sums(&m.state);
        let _ok = m
            .run_unproven(SystemCmd::TransferApp {
                app_id: "c".into(),
                to: "bob".into(),
                actor: "alice".into(),
                consent: serde_json::Value::Null,
                owner_in_org: true,
                owner_in_team: true,
            })
            .unwrap();
        assert_totals_are_sums(&m.state);
        assert_eq!(m.state.file_bytes("account:bob"), 70);
        assert_eq!(m.state.file_bytes("account:alice"), 0);
        let _ok = m
            .run_unproven(SystemCmd::DeleteApp {
                app_id: "a".into(),
                actor: "alice".into(),
                owner_in_org: true,
                owner_in_team: true,
            })
            .unwrap();
        assert_totals_are_sums(&m.state);
        assert!(!m.state.payer_file_bytes.contains_key("org"));
    }

    #[test]
    fn a_personal_app_moves_only_to_an_owner_whose_quota_holds_its_files() {
        let mut m = machine_with_apps(100, &[("a", None), ("b", None)]);
        let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
        let _ok = m.run_unproven(account("bob")).unwrap();
        let _ok = m.run_unproven(charge("a", 80)).unwrap();
        // Bob holds 50 bytes in an app of his own.
        let mut bobs = m.state.apps["b"].clone();
        bobs.owner = "bob".into();
        let _prev = m.state.apps.insert("b".into(), bobs);
        let _ok = m.run_unproven(charge("b", 50)).unwrap();
        let transfer = SystemCmd::TransferApp {
            app_id: "a".into(),
            to: "bob".into(),
            actor: "alice".into(),
            consent: serde_json::Value::Null,
            owner_in_org: true,
            owner_in_team: true,
        };
        let refused = m.run_unproven(transfer.clone()).unwrap_err();
        assert!(refused.starts_with(QUOTA_PREFIX), "{refused}");
        assert_eq!(m.state.apps["a"].owner, "alice");
        let _ok = m.run_unproven(charge("b", -50)).unwrap();
        let _ok = m.run_unproven(transfer).unwrap();
        assert_eq!(m.state.file_bytes("account:bob"), 80);
    }

    // ------------------------------------------------------------ package origins

    const NOTES: &str = "https://notes.example.com";

    fn add_origin(
        actor: &str,
        package: &str,
        origin: &str,
        rp_id: &str,
        consent: Value,
    ) -> SystemCmd {
        SystemCmd::AddPackageOrigin {
            package: package.into(),
            origin: origin.into(),
            rp_id: rp_id.into(),
            served: false,
            actor: actor.into(),
            at: 1,
            consent,
        }
    }

    fn add_served(actor: &str, origin: &str, rp_id: &str, consent: Value) -> SystemCmd {
        SystemCmd::AddPackageOrigin {
            package: "p".into(),
            origin: origin.into(),
            rp_id: rp_id.into(),
            served: true,
            actor: actor.into(),
            at: 1,
            consent,
        }
    }

    fn remove_origin(actor: &str, origin: &str, consent: Value) -> SystemCmd {
        SystemCmd::RemovePackageOrigin {
            package: "p".into(),
            origin: origin.into(),
            actor: actor.into(),
            consent,
        }
    }

    /// Package `q`, its first release installed by `by`.
    fn install_q(by: &str) -> SystemCmd {
        SystemCmd::InstallPackage {
            package: "q".into(),
            signer_id: "did:key:q".into(),
            release: serde_json::from_value(json!({
                "version": "1.0.0", "mpk": "q1", "mpkSize": 1, "source": "upload",
                "services": { "": "b-q" }, "installedBy": by, "installedAt": 1
            }))
            .unwrap(),
        }
    }

    /// Registering and removing a package's origin applies only with its
    /// manager's passkey consent over exactly that change, once; anyone
    /// else's, however valid, is refused where it applies.
    #[test]
    fn package_origins_carry_their_managers_consent() {
        let (mut m, mut alice, mut bob) = two_accounts();
        let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
        let args = json!({ "origin": NOTES, "rpId": "notes.example.com" });
        let by_bob = consent(&mut bob, "package:p", "__add_origin", &args);
        assert!(
            m.run(add_origin("bob", "p", NOTES, "notes.example.com", by_bob))
                .unwrap_err()
                .contains("manager"),
            "not the package's manager"
        );
        for (why, rp_id, c) in [
            ("no consent", "notes.example.com", Value::Null),
            (
                "over another rpId",
                "example.com",
                consent(&mut alice, "package:p", "__add_origin", &args),
            ),
            (
                "over another package",
                "notes.example.com",
                consent(&mut alice, "package:q", "__add_origin", &args),
            ),
        ] {
            assert!(
                m.run(add_origin("alice", "p", NOTES, rp_id, c)).is_err(),
                "{why}"
            );
        }
        assert!(m.state.package_origins.is_empty());
        let c = consent(&mut alice, "package:p", "__add_origin", &args);
        let _ok = m
            .run(add_origin(
                "alice",
                "p",
                NOTES,
                "notes.example.com",
                c.clone(),
            ))
            .unwrap();
        assert_eq!(m.state.package_origins[NOTES].package, "p");
        assert_eq!(m.state.package_origins[NOTES].rp_id, "notes.example.com");
        assert!(
            m.run(add_origin("alice", "p", NOTES, "notes.example.com", c))
                .unwrap_err()
                .contains("used already"),
            "a consent is spent once"
        );

        let gone = json!({ "origin": NOTES });
        let by_bob = consent(&mut bob, "package:p", "__remove_origin", &gone);
        assert!(m.run(remove_origin("bob", NOTES, by_bob)).is_err());
        let wrong = consent(
            &mut alice,
            "package:p",
            "__remove_origin",
            &json!({ "origin": "https://x.example.com" }),
        );
        assert!(m.run(remove_origin("alice", NOTES, wrong)).is_err());
        assert!(m.state.package_origins.contains_key(NOTES));
        let c = consent(&mut alice, "package:p", "__remove_origin", &gone);
        let _ok = m.run(remove_origin("alice", NOTES, c)).unwrap();
        assert!(m.state.package_origins.is_empty());
    }

    /// A package's publisher manages its origins once its key is bound: the
    /// first installer no longer does; the operator never does.
    #[test]
    fn a_bound_publisher_manages_its_packages_origins() {
        let (mut m, _alice, _bob) = two_accounts();
        let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
        assert!(m.state.manages_package("p", "alice"));
        assert!(!m.state.manages_package("p", "bob"));
        assert!(!m.state.manages_package("p", OPERATOR));
        assert!(!m.state.manages_package("nope", "alice"));
        let _ok = m
            .run_unproven(SystemCmd::CreateOrg {
                org_id: "o".into(),
                name: "o".into(),
                actor: "bob".into(),
                release: None,
                recovery_key: None,
                at: 0,
            })
            .unwrap();
        let _prev = m.state.publishers.insert(
            "did:key:z".into(),
            super::PublisherBinding {
                org: "o".into(),
                generation: 1,
            },
        );
        assert!(m.state.manages_package("p", "bob"));
        assert!(!m.state.manages_package("p", "alice"));
    }

    /// One package per origin; at most `MAX_ORIGINS_PER_PACKAGE` per
    /// package; an origin or rp-id that does not fit is refused at apply too;
    /// a package uninstalled takes its origins with it.
    #[test]
    fn package_origins_are_unique_bounded_and_go_with_their_package() {
        use crate::origins::MAX_ORIGINS_PER_PACKAGE;
        let (mut m, _alice, _bob) = two_accounts();
        let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
        let _ok = m.run_unproven(install_q("bob")).unwrap();
        let add = |m: &mut SystemMachine, who: &str, pkg: &str, origin: &str, rp: &str| {
            m.run_unproven(add_origin(who, pkg, origin, rp, Value::Null))
        };
        let _ok = add(&mut m, "alice", "p", NOTES, "example.com").unwrap();
        // Again, the same: answered as it stands.
        let _ok = add(&mut m, "alice", "p", NOTES, "example.com").unwrap();
        assert!(
            add(&mut m, "alice", "p", NOTES, "notes.example.com").is_err(),
            "another rpId"
        );
        assert!(
            add(&mut m, "bob", "q", NOTES, "notes.example.com")
                .unwrap_err()
                .contains("another package"),
            "first come"
        );
        for (origin, rp) in [
            ("http://notes.example.com", "notes.example.com"),
            ("https://notes.example.com/", "notes.example.com"),
            ("https://x.example.com", "other.com"),
            ("https://x.example.com", "com"),
        ] {
            assert!(
                add(&mut m, "alice", "p", origin, rp).is_err(),
                "{origin} {rp}"
            );
        }
        for i in 1..MAX_ORIGINS_PER_PACKAGE {
            let _ok = add(
                &mut m,
                "alice",
                "p",
                &format!("https://n{i}.example.com"),
                "example.com",
            )
            .unwrap();
        }
        assert!(add(
            &mut m,
            "alice",
            "p",
            "https://more.example.com",
            "example.com"
        )
        .unwrap_err()
        .starts_with(QUOTA_PREFIX));
        let _ok = add(&mut m, "bob", "q", "https://q.example.org", "q.example.org").unwrap();

        let _ok = m
            .run_unproven(SystemCmd::UninstallPackage {
                package: "p".into(),
                version: None,
                actor: "alice".into(),
            })
            .unwrap();
        assert_eq!(
            m.state.package_origins.keys().collect::<Vec<_>>(),
            ["https://q.example.org"],
            "p's origins went with it"
        );
    }

    /// A served origin (a custom domain) applies only with its manager's
    /// consent naming it served: the consent to the same origin unserved,
    /// as a client from before signs it, does not make it served. One
    /// registration holds an origin either way, at most
    /// `MAX_SERVED_ORIGINS_PER_PACKAGE` of a package's are served, and one
    /// with a port is refused where it applies.
    #[test]
    fn served_origins_carry_their_own_consent_and_limit() {
        use crate::origins::{add_args, MAX_SERVED_ORIGINS, MAX_SERVED_ORIGINS_PER_PACKAGE};
        const KV: &str = "https://kv.example.com";
        let (mut m, mut alice, mut bob) = two_accounts();
        let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
        let plain = consent(
            &mut alice,
            "package:p",
            "__add_origin",
            &add_args(KV, "kv.example.com", false),
        );
        assert!(
            m.run(add_served("alice", KV, "kv.example.com", plain))
                .is_err(),
            "a consent to an unserved origin"
        );
        let args = add_args(KV, "kv.example.com", true);
        let by_bob = consent(&mut bob, "package:p", "__add_origin", &args);
        assert!(m
            .run(add_served("bob", KV, "kv.example.com", by_bob))
            .unwrap_err()
            .contains("manager"));
        assert!(m.state.package_origins.is_empty());
        let c = consent(&mut alice, "package:p", "__add_origin", &args);
        let out = m.run(add_served("alice", KV, "kv.example.com", c)).unwrap();
        assert_eq!(out["served"], true, "{out}");
        assert!(m.state.served_origin(KV).is_some());
        // The served consent does not register it unserved either.
        let c = consent(&mut alice, "package:p", "__add_origin", &args);
        assert!(m
            .run(add_origin("alice", "p", KV, "kv.example.com", c))
            .is_err());

        let add = |m: &mut SystemMachine, origin: &str, served: bool| {
            let mut cmd = add_origin("alice", "p", origin, "example.com", Value::Null);
            if let SystemCmd::AddPackageOrigin { served: s, .. } = &mut cmd {
                *s = served;
            }
            m.run_unproven(cmd)
        };
        assert!(
            add(&mut m, KV, false).is_err(),
            "held served, under another rpId too"
        );
        assert!(
            add(&mut m, "https://kv.example.com:8443", true).is_err(),
            "a port"
        );
        for i in 1..MAX_SERVED_ORIGINS_PER_PACKAGE {
            let _ok = add(&mut m, &format!("https://s{i}.example.com"), true).unwrap();
        }
        assert!(add(&mut m, "https://more.example.com", true)
            .unwrap_err()
            .starts_with(QUOTA_PREFIX));
        let _ok = add(&mut m, "https://plain.example.com", false).unwrap();
        assert!(m.state.served_origin("https://plain.example.com").is_none());

        // The cluster's own bound, whoever holds the rest.
        let _ok = m.run_unproven(install_q("bob")).unwrap();
        let held = m
            .state
            .package_origins
            .values()
            .filter(|o| o.served)
            .count();
        for i in held..MAX_SERVED_ORIGINS {
            let mut o = m.state.package_origins[KV].clone();
            o.package = "elsewhere".into();
            let _prev = m
                .state
                .package_origins
                .insert(format!("https://e{i}.example.org"), o);
        }
        let mut cmd = add_origin(
            "bob",
            "q",
            "https://q.example.org",
            "q.example.org",
            Value::Null,
        );
        if let SystemCmd::AddPackageOrigin { served, .. } = &mut cmd {
            *served = true;
        }
        assert_eq!(
            m.run_unproven(cmd).unwrap_err(),
            "quota exceeded: served origins"
        );
    }

    /// A registration and an `AddPackageOrigin` from before served origins
    /// read as unserved and write the bytes they did.
    #[test]
    fn origins_from_before_served_ones_read_unserved_and_write_as_before() {
        let record = json!({
            "package": "p", "rpId": "notes.example.com", "addedBy": "alice", "addedAt": 1
        });
        let o: crate::origins::PackageOrigin = serde_json::from_value(record.clone()).unwrap();
        assert!(!o.served);
        assert_eq!(serde_json::to_value(&o).unwrap(), record);
        let cmd = json!({
            "op": "addPackageOrigin", "package": "p", "origin": NOTES,
            "rp_id": "notes.example.com", "actor": "alice", "at": 1, "consent": null
        });
        let read: SystemCmd = serde_json::from_value(cmd.clone()).unwrap();
        assert!(matches!(
            read,
            SystemCmd::AddPackageOrigin { served: false, .. }
        ));
        assert_eq!(serde_json::to_value(&read).unwrap(), cmd);
    }

    /// State, refresh records and session claims from before package origins
    /// read with none and write the bytes they did.
    #[test]
    fn state_from_before_package_origins_reads_with_none_and_writes_as_before() {
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        let _ok = m.run_authorized(account("alice")).unwrap();
        let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
        let refresh: RefreshRecord = serde_json::from_value(json!({
            "account": "alice", "session": "s", "expiresAt": 9, "passkey": "c", "app": "p"
        }))
        .unwrap();
        assert_eq!(refresh.origin, None);
        let _prev = m.state.refresh.insert("h".into(), refresh);
        let before = serde_json::to_value(&m.state).unwrap();
        assert!(before.get("packageOrigins").is_none(), "{before}");
        assert!(before["refresh"]["h"].get("origin").is_none(), "{before}");
        let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
        assert!(restored.package_origins.is_empty());
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);

        // A token minted before names no origin, and verifies as it did.
        let claims: calimero_identity::SessionClaims = serde_json::from_value(json!({
            "account": "alice", "session": "s", "expires_at": 9, "app": "p", "node": 1, "key": "k"
        }))
        .unwrap();
        assert_eq!(claims.origin, None);
        assert!(serde_json::to_value(&claims)
            .unwrap()
            .get("origin")
            .is_none());
    }

    /// Invites, teams and default-capability entries from before app teams
    /// read with no creator, no team default and the organization's default,
    /// and write the bytes they did.
    #[test]
    fn invites_teams_and_default_caps_from_before_app_teams_read_and_write_as_before() {
        let invite = json!({
            "org": "o", "role": "member", "teams": ["t"], "expiresAt": 9, "usesLeft": 2
        });
        let read: crate::types::Invite = serde_json::from_value(invite.clone()).unwrap();
        assert!(read.created_by.is_empty());
        assert_eq!(read.created_at, 0);
        assert_eq!(serde_json::to_value(&read).unwrap(), invite);

        let team = json!({
            "teamId": "t", "name": "chat", "parent": null, "join": "everyone",
            "members": { "a": true }, "readOnly": [], "capabilities": {},
            "metadata": { "app": { "package": "com.chat" } }, "removed": []
        });
        let read: crate::types::Team = serde_json::from_value(team.clone()).unwrap();
        assert_eq!(read.default_capabilities, None);
        assert_eq!(read.app_package(), Some("com.chat"));
        assert_eq!(serde_json::to_value(&read).unwrap(), team);

        let cmd = json!({
            "op": "setDefaultCapabilities", "org": "o", "capabilities": 4,
            "actor": "a", "consent": null
        });
        let read: SystemCmd = serde_json::from_value(cmd.clone()).unwrap();
        assert!(matches!(
            read,
            SystemCmd::SetDefaultCapabilities { team: None, .. }
        ));
        assert_eq!(serde_json::to_value(&read).unwrap(), cmd);
        // Its consent states what it did before.
        assert_eq!(
            super::default_capabilities_args(None, 4),
            json!({ "capabilities": 4 })
        );
    }

    #[test]
    fn a_scope_apps_from_before_a_per_app_join_reads_writes_and_consents_as_before() {
        let cmd = json!({
            "op": "scopeApps", "org": "o", "apps": [{ "package": "com.chat" }],
            "shared": true, "nonce": "n", "actor": "a", "consent": null
        });
        let read: SystemCmd = serde_json::from_value(cmd.clone()).unwrap();
        let SystemCmd::ScopeApps { apps, shared, .. } = &read else {
            panic!("{read:?}");
        };
        let apps = apps.as_deref().unwrap();
        assert_eq!((apps[0].join, apps[0].keep.as_ref()), (None, None));
        assert_eq!(serde_json::to_value(&read).unwrap(), cmd);
        assert_eq!(
            super::scope_apps_args(Some(apps), *shared),
            json!({ "apps": [{ "package": "com.chat" }], "shared": true })
        );
    }

    #[test]
    fn state_with_no_compute_use_writes_no_compute_keys() {
        let mut m = SystemMachine::default();
        let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
        let before = serde_json::to_value(&m.state).unwrap();
        for key in ["compute", "nextRunSeq", "runGrants", "releasedRuns"] {
            assert!(before.get(key).is_none(), "{key} in {before}");
        }
    }

    // ------------------------------------------------------------ recovery

    use super::{
        NO_RECOVERY_KEY, NO_RECOVERY_PENDING, OWNER_ONLY_RECOVERY, RECOVERY_NOT_READY,
        RECOVERY_PENDING,
    };

    const SEED: [u8; 32] = [9; 32];

    fn set_key(key: Option<String>, actor: &str, consent: serde_json::Value) -> SystemCmd {
        SystemCmd::SetRecoveryKey {
            org: "org".into(),
            public_key: key,
            at: 10,
            actor: actor.into(),
            consent,
        }
    }

    fn start(seed: &[u8; 32], account: &str, nonce: &str, at: u64) -> SystemCmd {
        start_signed_for(seed, account, account, nonce, at)
    }

    /// A start for `account` carrying a signature the seed made for `signed_for`.
    fn start_signed_for(
        seed: &[u8; 32],
        account: &str,
        signed_for: &str,
        nonce: &str,
        at: u64,
    ) -> SystemCmd {
        SystemCmd::StartRecovery {
            org: "org".into(),
            account: account.into(),
            nonce: nonce.into(),
            signature: calimero_identity::recovery::sign_recovery(seed, "org", signed_for, nonce),
            at,
        }
    }

    fn public(seed: &[u8; 32]) -> String {
        calimero_identity::recovery::recovery_public(seed)
    }

    /// The owner (and only the owner) sets the key with a passkey; then the
    /// seed, and nothing else, starts a recovery for the account it signed
    /// for; the owner cancels with a passkey; the recovering account finishes
    /// with its own passkey once the delay passed, and takes the
    /// organization and the previous owner's apps in it.
    #[test]
    fn recovery_needs_the_owners_passkey_the_seed_and_the_delay() {
        let (mut m, mut alice, mut bob, mut carol) = proven_cluster();
        let _ = m
            .state
            .apps
            .get_mut("app")
            .map(|a| a.org = Some("org".into()));
        let key = Some(public(&SEED));
        let args = json!({ "publicKey": key });

        // No key yet: the seed starts nothing.
        assert_eq!(
            m.run(start(&SEED, "carol", "n0", 100)).unwrap_err(),
            NO_RECOVERY_KEY
        );
        // A member's own passkey does not set it.
        let by_bob = consent(&mut bob, "org:org", "__set_recovery_key", &args);
        assert_eq!(
            m.run(set_key(key.clone(), "bob", by_bob)).unwrap_err(),
            OWNER_ONLY_RECOVERY
        );
        needs_consent(
            &mut m,
            &mut alice,
            ("org:org", "__set_recovery_key"),
            &args,
            &json!({ "publicKey": public(&[1; 32]) }),
            |c| set_key(key.clone(), "alice", c),
        );
        assert_eq!(m.state.orgs["org"].recovery.as_ref().unwrap().set_at, 10);

        // A wrong seed, a signature for another account, and another org's
        // signature are refused where the command applies.
        assert!(m.run(start(&[1; 32], "carol", "n1", 100)).is_err());
        assert!(m
            .run(start_signed_for(&SEED, "carol", "bob", "n1", 100))
            .is_err());
        let mut other_org = start(&SEED, "carol", "n1", 100);
        if let SystemCmd::StartRecovery { org, .. } = &mut other_org {
            *org = "elsewhere".into();
        }
        assert!(m.run(other_org).is_err());

        // The seed's signature for carol: pending, and only one at a time.
        let out = m.run(start(&SEED, "carol", "n1", 100)).unwrap();
        assert_eq!(
            out["readyAt"],
            json!(100 + crate::types::RECOVERY_DELAY_SECS)
        );
        assert_eq!(
            m.run(start(&SEED, "bob", "n2", 101)).unwrap_err(),
            RECOVERY_PENDING
        );

        // The owner cancels; the same signature cannot start it again.
        let cancel = |actor: &str, c| SystemCmd::CancelRecovery {
            org: "org".into(),
            account: "carol".into(),
            actor: actor.into(),
            consent: c,
        };
        let by_bob = consent(
            &mut bob,
            "org:org",
            "__cancel_recovery",
            &json!({ "account": "carol" }),
        );
        assert_eq!(
            m.run(cancel("bob", by_bob)).unwrap_err(),
            OWNER_ONLY_RECOVERY
        );
        let by_alice = consent(
            &mut alice,
            "org:org",
            "__cancel_recovery",
            &json!({ "account": "carol" }),
        );
        let _ok = m.run(cancel("alice", by_alice)).unwrap();
        assert_eq!(
            m.run(start(&SEED, "carol", "n1", 100)).unwrap_err(),
            "that recovery signature was used already"
        );

        // Started again; finished only by carol, with her passkey, after the delay.
        let _ok = m.run(start(&SEED, "carol", "n3", 200)).unwrap();
        let ready = 200 + crate::types::RECOVERY_DELAY_SECS;
        let finish = |actor: &str, at: u64, c| SystemCmd::FinishRecovery {
            org: "org".into(),
            at,
            actor: actor.into(),
            consent: c,
        };
        let args = json!({ "org": "org" });
        let early = consent(&mut carol, "org:org", "__finish_recovery", &args);
        assert_eq!(
            m.run(finish("carol", ready - 1, early)).unwrap_err(),
            RECOVERY_NOT_READY
        );
        let by_bob = consent(&mut bob, "org:org", "__finish_recovery", &args);
        assert_eq!(
            m.run(finish("bob", ready, by_bob)).unwrap_err(),
            NO_RECOVERY_PENDING
        );
        let by_alice_for_carol = consent(&mut alice, "org:org", "__finish_recovery", &args);
        assert!(m.run(finish("carol", ready, by_alice_for_carol)).is_err());
        let done = m
            .run(finish(
                "carol",
                ready,
                consent(&mut carol, "org:org", "__finish_recovery", &args),
            ))
            .unwrap();
        assert_eq!(done["previousOwners"], json!(["alice"]));
        let o = &m.state.orgs["org"];
        assert_eq!(o.role("carol"), Some(OrgRole::Owner));
        assert_eq!(o.role("alice"), None, "the previous owner left");
        assert_eq!(o.role("bob"), Some(OrgRole::Member));
        assert!(o.recovery.as_ref().unwrap().pending.is_none());
        assert_eq!(
            m.state.apps["app"].owner, "carol",
            "the org's apps moved with it"
        );
        // Alice's passkeys and account are her own still.
        assert!(m.state.accounts.contains_key("alice"));
    }

    /// Rotating the key cancels a pending recovery and retires the old seed;
    /// removing it leaves nothing to recover with.
    #[test]
    fn rotating_the_recovery_key_retires_the_old_seed() {
        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        let set = |m: &mut SystemMachine, alice: &mut Soft, key: Option<String>| {
            let c = consent(
                alice,
                "org:org",
                "__set_recovery_key",
                &json!({ "publicKey": key }),
            );
            m.run(set_key(key, "alice", c)).unwrap();
        };
        set(&mut m, &mut alice, Some(public(&SEED)));
        let _ok = m.run(start(&SEED, "carol", "n1", 100)).unwrap();
        set(&mut m, &mut alice, Some(public(&[2; 32])));
        assert!(m.state.orgs["org"]
            .recovery
            .as_ref()
            .unwrap()
            .pending
            .is_none());
        assert!(
            m.run(start(&SEED, "carol", "n2", 100)).is_err(),
            "the old seed"
        );
        let _ok = m.run(start(&[2; 32], "carol", "n2", 100)).unwrap();
        set(&mut m, &mut alice, None);
        assert!(m.state.orgs["org"].recovery.is_none());
        assert_eq!(
            m.run(start(&[2; 32], "carol", "n3", 100)).unwrap_err(),
            NO_RECOVERY_KEY
        );
        // A key that is not an Ed25519 public key is refused.
        let bad = Some("not-a-key".to_owned());
        let c = consent(
            &mut alice,
            "org:org",
            "__set_recovery_key",
            &json!({ "publicKey": bad }),
        );
        assert!(m.run(set_key(bad, "alice", c)).is_err());
    }

    /// An organization written before recovery keys reads with none and
    /// writes back the same bytes, so a snapshot from before restores
    /// unchanged; its owner can set a key afterwards.
    #[test]
    fn state_from_before_recovery_reads_with_none_and_writes_as_before() {
        let written = json!({
            "orgId": "org", "name": "Docs", "members": { "alice": "owner" }, "teams": {},
            "defaultCapabilities": crate::types::caps::DEFAULT, "capabilities": {},
            "approvedReleases": ["2.0.0"], "outboundApprovals": {}, "metadata": null,
            "memberMetadata": {}, "upgradeRuns": {},
        });
        let org: crate::types::Org = serde_json::from_value(written.clone()).unwrap();
        assert!(org.recovery.is_none());
        assert_eq!(serde_json::to_value(&org).unwrap(), written);
        // `CreateOrg` as the previous release logged it.
        let cmd: SystemCmd = serde_json::from_value(json!({
            "op": "createOrg", "org_id": "o2", "name": "N", "actor": "alice", "release": null,
        }))
        .unwrap();
        assert!(matches!(
            cmd,
            SystemCmd::CreateOrg {
                recovery_key: None,
                at: 0,
                ..
            }
        ));

        let (mut m, mut alice, _bob, _carol) = proven_cluster();
        let _prev = m.state.orgs.insert("org".into(), org);
        let before = serde_json::to_value(&m.state).unwrap();
        let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
        let key = Some(public(&SEED));
        let c = consent(
            &mut alice,
            "org:org",
            "__set_recovery_key",
            &json!({ "publicKey": key }),
        );
        let _ok = m.run(set_key(key.clone(), "alice", c)).unwrap();
        assert_eq!(
            m.state.orgs["org"].recovery.as_ref().map(|r| &r.public_key),
            key.as_ref()
        );
    }

    mod sign_up {
        use serde_json::json;

        use super::super::{
            Governance, SignUp, SignUpInvite, SignUpRedemption, SystemCmd, SystemMachine,
            SystemState, OPERATOR, SIGN_UP_PREFIX,
        };
        use super::{account_with, operator_signed};

        fn machine(policy: SignUp) -> SystemMachine {
            let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
            let mut m = SystemMachine::default();
            let _ok = m
                .run(SystemCmd::Genesis {
                    governance: Governance {
                        sign_up: policy,
                        ..Governance::default()
                    },
                    release: Some("1.0.0".into()),
                    operator_key: Some(crate::proof::OperatorKey {
                        key: hex::encode(key.verifying_key().to_bytes()),
                        cluster: "c".into(),
                    }),
                    nodes: Vec::new(),
                    session_proofs: true,
                })
                .unwrap();
            m
        }

        /// `id`'s account with its passkey's consent, presenting `token`
        /// (signed for `id` at `now`).
        fn signing_up(id: &str, token: Option<&str>, now: u64) -> SystemCmd {
            let mut pk = calimero_identity::testing::SoftAuthenticator::new(
                "localhost",
                "http://localhost:3000",
            );
            let SystemCmd::CreateAccount {
                account,
                refresh,
                consent,
                ..
            } = account_with(id, &mut pk)
            else {
                unreachable!()
            };
            SystemCmd::CreateAccount {
                at: 0,
                account,
                invite: token.map(|t| SignUpRedemption {
                    invite_key: crate::proof::invite_key(t),
                    now,
                    signature: crate::proof::sign_invite_redeem(t, id, now),
                }),
                refresh,
                consent,
            }
        }

        fn sign_up_invite(token: &str, uses: u32, expires_at: u64) -> SystemCmd {
            let invite = SignUpInvite {
                expires_at,
                uses_left: uses,
                created_by: OPERATOR.into(),
                created_at: 1,
            };
            let key = crate::proof::invite_key(token);
            let args = super::super::sign_up_invite_args(&key, &invite);
            SystemCmd::CreateSignUpInvite {
                invite_key: key,
                invite,
                actor: OPERATOR.into(),
                proof: Some(operator_signed("cluster", "__create_sign_up_invite", &args)),
                consent: None,
            }
        }

        /// Alice (an account from before, whatever the policy) owns `org`,
        /// which has a one-use member invite under `token`.
        fn org_invite(m: &mut SystemMachine, token: &str) {
            let _prev = m.state.accounts.insert(
                "alice".into(),
                serde_json::from_value(
                    json!({ "accountId": "alice", "name": "a", "createdAt": 0, "passkeys": [] }),
                )
                .unwrap(),
            );
            let _prev = m.state.orgs.insert(
                "org".into(),
                serde_json::from_value(json!({
                    "orgId": "org", "name": "Docs", "members": { "alice": "owner" }, "teams": {}
                }))
                .unwrap(),
            );
            let _prev = m.state.invites.insert(
                crate::proof::invite_key(token),
                serde_json::from_value(json!({
                    "org": "org", "role": "member", "teams": [], "expiresAt": 100, "usesLeft": 1,
                }))
                .unwrap(),
            );
        }

        fn refused(r: Result<serde_json::Value, String>) -> String {
            let e = r.unwrap_err();
            assert!(e.starts_with(SIGN_UP_PREFIX), "{e}");
            e
        }

        /// Open takes anyone, as before; an invite presented there is still
        /// checked and, for an organization's, joins it.
        #[test]
        fn an_open_cluster_takes_anyone() {
            let mut m = machine(SignUp::Open);
            let _ok = m.run(signing_up("a", None, 10)).unwrap();
            let _e = refused(m.run(signing_up("b", Some("inv_unknown"), 10)));
            assert!(!m.state.accounts.contains_key("b"));
            org_invite(&mut m, "inv_org");
            let v = m.run(signing_up("c", Some("inv_org"), 10)).unwrap();
            assert_eq!(v["org"], "org");
            assert!(m.state.orgs["org"].members.contains_key("c"));
        }

        /// Invite: refused without one, with an unknown, expired or used
        /// one, or a forged signature; taken with an organization invite (the
        /// account joins) and with a sign-up invite (spent per account).
        #[test]
        fn an_invite_cluster_takes_only_invited_people() {
            let mut m = machine(SignUp::Invite);
            let e = refused(m.run(signing_up("a", None, 10)));
            assert!(e.contains("invite only"), "{e}");
            let _e = refused(m.run(signing_up("a", Some("inv_unknown"), 10)));

            org_invite(&mut m, "inv_org");
            let _e = refused(m.run(signing_up("a", Some("inv_org"), 100)));
            let SystemCmd::CreateAccount {
                account,
                refresh,
                consent,
                ..
            } = signing_up("a", None, 10)
            else {
                unreachable!()
            };
            let forged = SystemCmd::CreateAccount {
                at: 0,
                account,
                invite: Some(SignUpRedemption {
                    invite_key: crate::proof::invite_key("inv_org"),
                    now: 10,
                    signature: crate::proof::sign_invite_redeem("inv_guess", "a", 10),
                }),
                refresh,
                consent,
            };
            assert!(m.run(forged).is_err(), "not the token's key");
            let _ok = m.run(signing_up("a", Some("inv_org"), 10)).unwrap();
            assert_eq!(
                m.state.orgs["org"].members["a"],
                crate::types::OrgRole::Member
            );
            let _e = refused(m.run(signing_up("b", Some("inv_org"), 10)));

            let _ok = m.run(sign_up_invite("sup_two", 2, 50)).unwrap();
            let _ok = m.run(signing_up("b", Some("sup_two"), 10)).unwrap();
            assert_eq!(
                m.state.sign_up_invites[&crate::proof::invite_key("sup_two")].uses_left,
                1
            );
            let _e = refused(m.run(signing_up("c", Some("sup_two"), 50)));
            let _ok = m.run(signing_up("c", Some("sup_two"), 11)).unwrap();
            assert!(
                m.state.sign_up_invites.is_empty(),
                "the last use removes it"
            );
            let _e = refused(m.run(signing_up("d", Some("sup_two"), 12)));
            assert_eq!(m.state.accounts.len(), 4);
        }

        /// Closed: only a sign-up invite (a pre-authorization) lets anyone
        /// in; an organization invite does not.
        #[test]
        fn a_closed_cluster_takes_only_preauthorized_people() {
            let mut m = machine(SignUp::Closed);
            let e = refused(m.run(signing_up("a", None, 10)));
            assert!(e.contains("pre-authorized"), "{e}");
            org_invite(&mut m, "inv_org");
            let _e = refused(m.run(signing_up("a", Some("inv_org"), 10)));
            assert_eq!(m.state.invites.len(), 1, "a refusal spends nothing");
            let _ok = m.run(sign_up_invite("sup_one", 1, 50)).unwrap();
            let _ok = m.run(signing_up("a", Some("sup_one"), 10)).unwrap();
            let _e = refused(m.run(signing_up("b", Some("sup_one"), 10)));
        }

        /// The policy and sign-up invites change only with the operator's
        /// signature over exactly that change, once; with the cluster's owner
        /// named (`deployers: owner`), only with the owner's passkey.
        #[test]
        fn the_policy_changes_only_with_the_operators_signature() {
            let mut m = machine(SignUp::Open);
            let set = |policy: SignUp, proof: Option<serde_json::Value>| SystemCmd::SetSignUp {
                policy,
                actor: OPERATOR.into(),
                proof,
                consent: None,
            };
            let closed = json!({ "policy": "closed" });
            assert!(m.run(set(SignUp::Closed, None)).is_err(), "no signature");
            let wrong = operator_signed("cluster", "__sign_up", &json!({ "policy": "invite" }));
            assert!(
                m.run(set(SignUp::Closed, Some(wrong))).is_err(),
                "another change"
            );
            let mut forged = operator_signed("cluster", "__sign_up", &closed);
            forged["signature"] = json!("00".repeat(64));
            assert!(m.run(set(SignUp::Closed, Some(forged))).is_err(), "forged");
            let as_alice = SystemCmd::SetSignUp {
                policy: SignUp::Closed,
                actor: "alice".into(),
                proof: Some(operator_signed("cluster", "__sign_up", &closed)),
                consent: None,
            };
            assert!(m.run(as_alice).is_err(), "only the operator signs");
            assert_eq!(m.state.governance().sign_up, SignUp::Open);
            let proof = operator_signed("cluster", "__sign_up", &closed);
            let _ok = m.run(set(SignUp::Closed, Some(proof.clone()))).unwrap();
            assert_eq!(m.state.governance().sign_up, SignUp::Closed);
            assert!(m.run(set(SignUp::Closed, Some(proof))).is_err(), "replay");

            // A sign-up invite and its revocation: signed, once.
            let SystemCmd::CreateSignUpInvite {
                invite_key, invite, ..
            } = sign_up_invite("sup_x", 1, 50)
            else {
                unreachable!()
            };
            let unsigned = SystemCmd::CreateSignUpInvite {
                invite_key,
                invite,
                actor: OPERATOR.into(),
                proof: None,
                consent: None,
            };
            assert!(m.run(unsigned).is_err(), "no signature");
            let _ok = m.run(sign_up_invite("sup_x", 1, 50)).unwrap();
            let key = crate::proof::invite_key("sup_x");
            let revoke = |proof| SystemCmd::RevokeSignUpInvite {
                invite_key: key.clone(),
                actor: OPERATOR.into(),
                proof,
                consent: None,
            };
            assert!(m.run(revoke(None)).is_err());
            let args = json!({ "inviteKey": key });
            let _ok = m
                .run(revoke(Some(operator_signed(
                    "cluster",
                    "__revoke_sign_up_invite",
                    &args,
                ))))
                .unwrap();
            let _e = refused(m.run(signing_up("a", Some("sup_x"), 10)));

            // Once the cluster's owner is named, the operator's signature no
            // longer changes the policy.
            m.state.governance.as_mut().unwrap().deployers = super::super::Deployers::Owner;
            let _e = refused(m.run(signing_up("owner", None, 10)));
            let _prev = m.state.accounts.insert(
                "owner".into(),
                serde_json::from_value(
                    json!({ "accountId": "owner", "name": "o", "createdAt": 0, "passkeys": [] }),
                )
                .unwrap(),
            );
            m.state.cluster_owner = Some(super::super::Deployer::Account("owner".into()));
            let open = json!({ "policy": "open" });
            let e = m
                .run(set(
                    SignUp::Open,
                    Some(operator_signed("cluster", "__sign_up", &open)),
                ))
                .unwrap_err();
            assert!(e.contains("owner"), "{e}");
            assert!(m.state.runs_sign_up("owner").is_ok());
            assert!(m.state.runs_sign_up(OPERATOR).is_err());
        }

        /// State from before sign-up policies reads as `open` with no
        /// sign-up invites, and writes the bytes it did; a `CreateAccount`
        /// from before (no `invite`) applies as before.
        #[test]
        fn state_from_before_sign_up_reads_as_open() {
            let mut m = SystemMachine::default();
            let _ok = m
                .run_authorized(super::genesis(Governance::default()))
                .unwrap();
            let old = serde_json::to_value(&m.state).unwrap();
            assert!(old.get("signUpInvites").is_none(), "{old}");
            assert!(old["governance"].get("signUp").is_none(), "{old}");
            let mut restored: SystemState = serde_json::from_value(old.clone()).unwrap();
            assert_eq!(restored.governance().sign_up, SignUp::Open);
            assert!(restored.sign_up_invites.is_empty());
            assert_eq!(serde_json::to_value(&restored).unwrap(), old);
            // A command the previous release logged.
            let cmd: SystemCmd = serde_json::from_value(json!({
                "op": "createAccount",
                "account": { "accountId": "a", "name": "a", "createdAt": 0, "passkeys": [] },
            }))
            .unwrap();
            let mut m = SystemMachine {
                state: restored.clone(),
                ..SystemMachine::default()
            };
            let _ok = m.run_authorized(cmd).unwrap();
            restored = m.state;
            assert!(restored.accounts.contains_key("a"));
        }
    }
}
