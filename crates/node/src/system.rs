//! The system group: fleet, app directory, identity and organizations.
//!
//! Commands are applied in log order on every node. Each command that acts for
//! someone carries that `actor`, and `apply` checks their authority against
//! the state the command applies to, so the log alone decides every outcome.

use std::collections::{BTreeMap, BTreeSet};

use calimero_bundle::UpgradeAuthority;
use calimero_identity::Credential;
use calimero_log::Machine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::types::{
    Access, Account, AccountId, ApiKeyRecord, AppId, AppRecord, Audience, Invite, NodeRecord, Org,
    OrgId, OrgRole, RefreshRecord, Team, TeamId, TeamJoin, TeamRole,
};

/// Per-account limits, replicated so every node enforces the same ones at apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Quotas {
    pub max_apps_per_account: u32,
    pub max_bundle_bytes_per_account: u64,
    pub max_api_keys_per_account: u32,
    pub max_orgs_per_account: u32,
    /// Accounts the cluster takes in all: sign-up is open, so this is what
    /// keeps it from growing without bound (Xil's review, issue 12).
    pub max_accounts: u32,
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            max_apps_per_account: 100,
            max_bundle_bytes_per_account: 1024 * 1024 * 1024,
            max_api_keys_per_account: 50,
            max_orgs_per_account: 20,
            max_accounts: 100_000,
        }
    }
}

/// The prefix of every apply error that is a quota refusal.
pub const QUOTA_PREFIX: &str = "quota exceeded";

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
    pub nodes: BTreeMap<u64, NodeRecord>,
    /// Nodes the operator removed (`RemoveNode`): registered again only by
    /// the operator (`/ops/nodes`).
    #[serde(default)]
    pub removed_nodes: std::collections::BTreeSet<u64>,
    pub apps: BTreeMap<AppId, AppRecord>,
    pub next_group: u64,
    pub bundles: BTreeMap<String, BundleRecord>,
    pub accounts: BTreeMap<AccountId, Account>,
    /// credential id -> account
    pub credentials: BTreeMap<String, AccountId>,
    /// API key hash -> record
    pub api_keys: BTreeMap<String, ApiKeyRecord>,
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
}

pub use calimero_client::attest::{Authority, Deployers, Deployments, Governance};

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

impl SystemState {
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
                v.services.values().any(|h| *h == a.bundle) && self.may_reach(a, Some(who))
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

    /// Whether `who` may name the bundle `hash` for an app: it must be
    /// recorded, and if a package release ships it, one such release must be
    /// visible to `who`, or `who` uploaded the same WASM as a raw bundle.
    #[must_use]
    pub fn sees_bundle(&self, hash: &str, who: &str) -> bool {
        let Some(b) = self.bundles.get(hash) else {
            return false;
        };
        let mut shipped = self
            .packages
            .values()
            .flat_map(|p| p.versions.values())
            .filter(|v| v.services.values().any(|h| h == hash))
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
    /// its owner, or, for an org app, whoever
    /// [manages the organization's apps](Self::manages_apps) on the app's
    /// team.
    #[must_use]
    pub fn may_upgrade(&self, app: &AppRecord, account: &str) -> bool {
        if !self.upgrades_of(&app.bundle).is_owner() {
            return self
                .upgraded_by_publisher(&app.bundle)
                .is_some_and(|p| self.manages_apps(p, account, None));
        }
        let team = match &app.access {
            Access::Team { team } => Some(team.as_str()),
            _ => None,
        };
        app.owner == account
            || app
                .org
                .as_ref()
                .is_some_and(|o| self.manages_apps(o, account, team))
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

    /// Revoke `session`: its refresh tokens go and its tokens stop verifying.
    fn revoke_session(&mut self, session: &str, now: u64) {
        self.refresh.retain(|_, r| r.session != session);
        let _new = self.revoked_sessions.insert(session.to_owned());
        let _prev = self.revoked_at.insert(session.to_owned(), now);
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

    /// Whether `account` may reach `app` at all.
    #[must_use]
    pub fn may_reach(&self, app: &AppRecord, account: Option<&str>) -> bool {
        match (&app.access, account) {
            (Access::Public, _) => true,
            (_, None) => false,
            (_, Some(a)) if a == app.owner => true,
            (Access::Owner, Some(_)) => false,
            (Access::Accounts { accounts }, Some(a)) => accounts.iter().any(|x| x == a),
            (Access::Org, Some(a)) => app
                .org
                .as_ref()
                .and_then(|o| self.orgs.get(o))
                .is_some_and(|o| o.role(a).is_some()),
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

    match cmd {
        SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator,
            actor,
            run: None,
            consent,
            ..
        } => {
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
                || !release.is_some_and(|v| v.services.values().any(|h| h == bundle))
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
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            states(&st, app_id, "__transfer", &|a| a["to"] == json!(to))?;
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
        SystemCmd::CreateApiKey {
            hash,
            record,
            consent,
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
        SystemCmd::RemoveTeamMember {
            org,
            team,
            account,
            actor,
            consent,
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
            capabilities,
            actor,
            consent,
        } => {
            let (st, used) = fresh(consent_of(actor, consent)?)?;
            let args = json!({ "capabilities": capabilities });
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
        // The token's key signs the redemption: only a node the redeemer
        // gave the token to can make it, never one that only saw the log.
        SystemCmd::RedeemInvite {
            invite_key,
            account,
            now,
            signature,
        } => {
            if !s.invites.contains_key(invite_key) {
                return Err("unknown invite");
            }
            let used = crate::proof::verify_invite_redeem(invite_key, account, *now, signature)?;
            if s.used_proofs.contains(&used) {
                return Err("that redemption was used already");
            }
            Ok(Some(used))
        }
        // The fleet: the operator's signature.
        SystemCmd::RegisterNode { node, proof } => {
            let (st, used) = fresh(operator(proof)?)?;
            let args = json!({ "id": node.id, "addr": node.addr, "zone": node.zone });
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
    json!({
        "hash": hash,
        "keyId": record.key_id,
        "label": record.label,
        "apps": record.apps,
        "methods": record.methods,
        "readOnly": record.read_only,
        "allowConfirm": record.allow_confirm,
        "expiresAt": record.expires_at,
    })
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
    },
    TransferApp {
        app_id: AppId,
        to: AccountId,
        actor: AccountId,
        /// The owner's passkey assertion over this transfer.
        #[serde(default)]
        consent: Value,
    },
    DeleteApp {
        app_id: AppId,
        actor: AccountId,
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
    },
    CreateAccount {
        account: Account,
    },
    /// A new passkey, approved by one the account already has.
    AddPasskey {
        account: AccountId,
        credential: Credential,
        /// The actor's passkey consent over it ([`crate::proof`]).
        #[serde(default)]
        consent: Value,
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
    SetSignCount {
        credential_id: String,
        count: u32,
    },
    CreateApiKey {
        hash: String,
        record: ApiKeyRecord,
        /// The account's passkey consent over [`api_key_args`].
        #[serde(default)]
        consent: Value,
    },
    RevokeApiKey {
        key_id: String,
        actor: AccountId,
    },
    CreateRefresh {
        hash: String,
        record: RefreshRecord,
    },
    UseRefresh {
        old_hash: String,
        new_hash: String,
        record: RefreshRecord,
        now: u64,
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
    CreateOrg {
        org_id: OrgId,
        name: String,
        actor: AccountId,
        /// The release of the node the org was created through: approved from
        /// the start, so its apps run where the org began.
        #[serde(default)]
        release: Option<String>,
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
    /// The operator names the cluster's owner where governance says
    /// `deployers: owner` (`cluster / __cluster_owner / { owner }`): once; it
    /// grants deployers from then on.
    SetClusterOwner {
        owner: Deployer,
        #[serde(default)]
        proof: Value,
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
    /// The capabilities members hold unless set for them.
    SetDefaultCapabilities {
        org: OrgId,
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

/// The system group's state machine.
#[derive(Default)]
pub struct SystemMachine {
    pub state: SystemState,
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

impl SystemMachine {
    fn org_mut(&mut self, org: &str) -> Result<&mut Org, &'static str> {
        self.state.orgs.get_mut(org).ok_or("no such organization")
    }

    /// Apply `cmd`, only once its proof holds ([`authorize`]): refused if it
    /// is new work its scope has lost ([`Self::new_work_refusal`]), else by its
    /// own rules. The proof is spent only when the command applies.
    fn run(&mut self, cmd: SystemCmd) -> Result<Value, String> {
        let used = authorize(&self.state, &cmd).map_err(str::to_owned)?;
        if let Some(stall) = self.new_work_refusal(&cmd) {
            return Err(stall.message());
        }
        let out = self.run_authorized(cmd).map_err(str::to_owned)?;
        if let Some(used) = used {
            let _new = self.state.used_proofs.insert(used);
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
        let s = &mut self.state;
        match cmd {
            SystemCmd::RegisterNode { node, .. } => {
                let _was = s.removed_nodes.remove(&node.id);
                let _prev = s.nodes.insert(node.id, node);
                Ok(Value::Null)
            }
            SystemCmd::RemoveNode { id, .. } => {
                if s.apps.values().any(|a| a.replicas.contains(&id)) {
                    return Err("an app still keeps a replica on that node; move it first");
                }
                let _gone = s.nodes.remove(&id).ok_or("no such node")?;
                let _new = s.removed_nodes.insert(id);
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
                // The operator's installs count against no account's quota.
                if !by_operator && used + release.mpk_size > s.quotas.max_bundle_bytes_per_account {
                    return Err("quota exceeded: bundle storage");
                }
                for (service, hash) in &release.services {
                    let at = PackageRef {
                        package: package.clone(),
                        version: release.version.clone(),
                        service: (!service.is_empty()).then(|| service.clone()),
                    };
                    let b = s.bundles.entry(hash.clone()).or_insert(BundleRecord {
                        hash: hash.clone(),
                        size: release.mpk_size,
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
                let hashes: BTreeSet<String> = gone
                    .iter()
                    .filter_map(|v| p.versions.get(v))
                    .flat_map(|v| v.services.values().cloned())
                    .collect();
                // Two releases may ship the same WASM: a hash another release
                // still provides stays, recorded under that release.
                let provider = |s: &SystemState, h: &str| -> Option<PackageRef> {
                    s.packages.iter().find_map(|(name, p)| {
                        p.versions.iter().find_map(|(v, r)| {
                            let gone_here = *name == package && gone.contains(v);
                            (!gone_here)
                                .then(|| r.services.iter().find(|(_, x)| x.as_str() == h))
                                .flatten()
                                .map(|(svc, _)| PackageRef {
                                    package: name.clone(),
                                    version: v.clone(),
                                    service: (!svc.is_empty()).then(|| svc.clone()),
                                })
                        })
                    })
                };
                let mut keep = BTreeMap::new();
                for h in &hashes {
                    if let Some(r) = provider(s, h) {
                        let _prev = keep.insert(h.clone(), r);
                    }
                }
                if s.apps
                    .values()
                    .any(|a| hashes.contains(&a.bundle) && !keep.contains_key(&a.bundle))
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
                }
                Ok(json!({ "removed": gone }))
            }
            SystemCmd::CreateApp { app } => {
                let mut app = *app;
                if !s.bundles.contains_key(&app.bundle) {
                    return Err("unknown bundle");
                }
                let owned = s.apps.values().filter(|a| a.owner == app.owner).count();
                if owned >= s.quotas.max_apps_per_account as usize {
                    return Err("quota exceeded: apps");
                }
                if s.apps.contains_key(&app.app_id) {
                    return Err("app exists");
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
                by_operator,
                actor,
                at,
                run,
                consent: _,
            } => {
                if !s.bundles.contains_key(&bundle) {
                    return Err("unknown bundle");
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
                    s.apps
                        .get(&app_id)
                        .is_some_and(|a| s.may_upgrade(a, &actor))
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
                    });
                    a.bundle = bundle;
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
            } => {
                let a = s.apps.get_mut(&app_id).ok_or("no such app")?;
                a.last_upgrade = Some(crate::types::UpgradeRecord {
                    from: a.bundle.clone(),
                    to: bundle,
                    at,
                    by,
                    ok: false,
                    error: error.clone(),
                    run: run.clone(),
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
                consent: _,
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
                let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
                if app.owner != actor {
                    return Err("only the owner may transfer");
                }
                app.owner = to;
                Ok(Value::Null)
            }
            SystemCmd::DeleteApp { app_id, actor } => {
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                let admin = app.org.as_ref().is_some_and(|o| {
                    s.orgs
                        .get(o)
                        .and_then(|o| o.role(&actor))
                        .is_some_and(|r| r >= OrgRole::Admin)
                });
                if app.owner != actor && !admin {
                    return Err("only the owner or an org admin may delete");
                }
                let _prev = s.apps.remove(&app_id);
                Ok(Value::Null)
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
            } => {
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                if app.owner != actor {
                    return Err("only the owner may change access");
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
            SystemCmd::CreateAccount { account } => {
                // The first account waits for genesis: the rules it joins
                // under are fixed before anyone relies on them.
                if s.governance.is_none() && s.accounts.is_empty() {
                    return Err("the cluster is starting");
                }
                if s.accounts.contains_key(&account.account_id) {
                    return Err("account exists");
                }
                if s.accounts.len() >= s.quotas.max_accounts as usize {
                    return Err("quota exceeded: accounts");
                }
                for c in &account.passkeys {
                    if s.credentials.contains_key(&c.id) {
                        return Err("credential already registered");
                    }
                }
                for c in &account.passkeys {
                    let _prev = s
                        .credentials
                        .insert(c.id.clone(), account.account_id.clone());
                }
                let id = account.account_id.clone();
                let _prev = s.accounts.insert(id.clone(), account);
                Ok(json!({ "accountId": id }))
            }
            SystemCmd::AddPasskey {
                account,
                credential,
                ..
            } => {
                if s.credentials.contains_key(&credential.id) {
                    return Err("credential already registered");
                }
                let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
                if acc.passkeys.len() >= 10 {
                    return Err("an account holds at most 10 passkeys");
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
                let sessions: BTreeSet<String> = s
                    .refresh
                    .values()
                    .filter(|r| r.account == actor)
                    .map(|r| r.session.clone())
                    .collect();
                let n = sessions.len();
                for session in sessions {
                    s.revoke_session(&session, now);
                }
                s.prune_sessions(now);
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
                // Counters only move forward, whatever order logins committed in.
                if count != 0 && count <= cred.sign_count {
                    return Err("stale signature counter");
                }
                cred.sign_count = count;
                Ok(Value::Null)
            }
            SystemCmd::CreateApiKey { hash, record, .. } => {
                if !s.accounts.contains_key(&record.account) {
                    return Err("unknown account");
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
            SystemCmd::RevokeApiKey { key_id, actor } => {
                let before = s.api_keys.len();
                s.api_keys
                    .retain(|_, r| !(r.key_id == key_id && r.account == actor));
                if s.api_keys.len() == before {
                    return Err("no such key");
                }
                Ok(Value::Null)
            }
            SystemCmd::CreateRefresh { hash, record } => {
                let _prev = s.refresh.insert(hash, record);
                Ok(Value::Null)
            }
            SystemCmd::UseRefresh {
                old_hash,
                new_hash,
                record,
                now,
            } => {
                let old = s.refresh.remove(&old_hash).ok_or("unknown refresh token")?;
                if old.expires_at <= now || old.account != record.account {
                    return Err("refresh token expired");
                }
                if s.revoked_sessions.contains(&old.session) {
                    return Err("session revoked");
                }
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
                Ok(Value::Null)
            }
            SystemCmd::CreateOrg {
                org_id,
                name,
                actor,
                release,
            } => {
                if !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
                }
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
                    },
                );
                Ok(json!({ "orgId": org_id }))
            }
            SystemCmd::Genesis {
                governance,
                release,
                operator_key,
                nodes,
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
                Ok(json!({ "governance": governance }))
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
            SystemCmd::DeleteOrg {
                org,
                actor,
                consent: _,
            } => {
                let o = s.orgs.get(&org).ok_or("no such organization")?;
                if o.role(&actor) != Some(OrgRole::Owner) {
                    return Err("only the organization's owner deletes it");
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
                    let _removed = s.apps.remove(a);
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
            } => {
                metadata_fits(&metadata)?;
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                let admin = app.org.as_ref().is_some_and(|o| {
                    s.orgs
                        .get(o)
                        .and_then(|o| o.role(&actor))
                        .is_some_and(|r| r >= OrgRole::Admin)
                });
                if app.owner != actor && !admin {
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
            } => {
                let o = self.org_mut(&org)?;
                if o.teams.contains_key(&team_id) {
                    return Err("team exists");
                }
                // An admin anywhere, a team's lead below it; a member with
                // CREATE_TEAM at the top.
                let allowed = match &parent {
                    Some(p) => o.teams.contains_key(p) && o.manages_team(&actor, p),
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
                let default = o
                    .capabilities
                    .get(&account)
                    .copied()
                    .unwrap_or(o.default_capabilities);
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
            SystemCmd::DeleteTeam { org, team, actor } => {
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
                let apps: Vec<String> = s
                    .apps
                    .values()
                    .filter(|a| {
                        a.org.as_deref() == Some(org.as_str())
                            && matches!(&a.access, Access::Team { team } if gone.contains(team))
                    })
                    .map(|a| a.app_id.clone())
                    .collect();
                for a in &apps {
                    let _removed = s.apps.remove(a);
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
                capabilities,
                actor,
                ..
            } => {
                let o = self.org_mut(&org)?;
                if !o.role(&actor).is_some_and(|r| r >= OrgRole::Admin) {
                    return Err("only admins manage members");
                }
                o.default_capabilities = capabilities;
                Ok(Value::Null)
            }
            SystemCmd::RemoveTeamMember {
                org,
                team,
                account,
                actor,
                ..
            } => {
                let o = self.org_mut(&org)?;
                if actor != account && !o.manages_team(&actor, &team) {
                    return Err("not allowed to manage this team");
                }
                let t = o.teams.get_mut(&team).ok_or("no such team")?;
                let _prev = t.members.remove(&account);
                let _was = t.read_only.remove(&account);
                let _caps = t.capabilities.remove(&account);
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
                ..
            } => {
                let o = s.orgs.get(&invite.org).ok_or("no such organization")?;
                let role = o.role(&actor).ok_or("not a member")?;
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
                let _prev = s.invites.insert(invite_key, invite);
                Ok(Value::Null)
            }
            SystemCmd::RedeemInvite {
                invite_key,
                account,
                now,
                ..
            } => {
                if !s.accounts.contains_key(&account) {
                    return Err("unknown account");
                }
                let inv = s.invites.get_mut(&invite_key).ok_or("unknown invite")?;
                if inv.expires_at <= now || inv.uses_left == 0 {
                    return Err("invite expired");
                }
                inv.uses_left -= 1;
                let inv = inv.clone();
                if inv.uses_left == 0 {
                    let _prev = s.invites.remove(&invite_key);
                }
                let o = s.orgs.get_mut(&inv.org).ok_or("no such organization")?;
                let role = o
                    .members
                    .get(&account)
                    .copied()
                    .map_or(inv.role, |r| r.max(inv.role));
                let _prev = o.members.insert(account.clone(), role);
                for t in &inv.teams {
                    if let Some(team) = o.teams.get_mut(t) {
                        let _prev = team.members.entry(account.clone()).or_insert(false);
                    }
                }
                Ok(json!({ "org": inv.org }))
            }
        }
    }
}

impl Machine for SystemMachine {
    const IN_MEMORY: bool = true;

    fn apply(
        &mut self,
        _index: u64,
        command: &[u8],
        _state: &mut calimero_log::StateTx<'_>,
    ) -> Vec<u8> {
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
mod tests {

    use serde_json::json;

    use super::{
        Authority, Governance, OrgRole, ReleaseScope, SystemCmd, SystemMachine, SystemState,
    };

    // The tests above `account_with` judge each command's own authority rules
    // (`run_authorized`); the proofs `run` also requires are tested below it.

    fn account(id: &str) -> SystemCmd {
        SystemCmd::CreateAccount {
            account: serde_json::from_value(
                json!({ "accountId": id, "name": id, "createdAt": 0, "passkeys": [] }),
            )
            .unwrap(),
        }
    }

    fn genesis(governance: Governance) -> SystemCmd {
        SystemCmd::Genesis {
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
            app_id: app.into(),
            bundle: "b2".into(),
            by_operator: false,
            actor: actor.into(),
            at: 0,
            run: None,
            consent: serde_json::Value::Null,
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
            app_id: "app".into(),
            bundle: "b2".into(),
            by_operator,
            actor: actor.into(),
            at: 0,
            run: None,
            consent: serde_json::Value::Null,
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
        SystemCmd::CreateAccount {
            account: serde_json::from_value(json!({
                "accountId": id, "name": id, "createdAt": 0,
                "passkeys": [credential], "approvedReleases": ["1.0.0"],
            }))
            .unwrap(),
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
            app_id: "app".into(),
            bundle: "b2".into(),
            by_operator: false,
            actor: "alice".into(),
            at: 0,
            run: run.map(str::to_owned),
            consent,
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
                governance: all(Authority::Owner),
                release: Some("2.0.0".into()),
                operator_key: Some(operator_signer().1),
                nodes: [1, 2, 3, 4]
                    .map(|id| crate::types::NodeRecord {
                        id,
                        addr: format!("http://n{id}/"),
                        zone: String::new(),
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
        };
        // Another key's hash under the same consent: refused.
        let c = consent(&mut alice, "account:alice", "__create_api_key", &args);
        assert!(m
            .run(SystemCmd::CreateApiKey {
                hash: "h2".into(),
                record: record.clone(),
                consent: c,
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
            },
        );
        let redeem = |signature: String| SystemCmd::RedeemInvite {
            invite_key: key.clone(),
            account: "carol".into(),
            now: 10,
            signature,
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
            app_id: id.into(),
            bundle: "b2".into(),
            by_operator: false,
            actor: actor.into(),
            at,
            run: None,
            consent: serde_json::Value::Null,
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
            app_id: "inst".into(),
            bundle: bundle.into(),
            by_operator: false,
            actor: actor.into(),
            at: 0,
            run: None,
            consent: serde_json::Value::Null,
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
}
