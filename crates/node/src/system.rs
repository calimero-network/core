//! The system group: fleet, app directory, identity and organizations.
//!
//! Commands are applied in log order on every node. Each command that acts for
//! someone carries that `actor`, and `apply` checks their authority against
//! the state the command applies to, so the log alone decides every outcome.

use std::collections::{BTreeMap, BTreeSet};

use calimero_identity::Credential;
use calimero_log::Machine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::types::{
    Access, Account, AccountId, ApiKeyRecord, AppId, AppRecord, Audience, Invite, NodeRecord, Org,
    OrgId, OrgRole, RefreshRecord, Team, TeamId, TeamJoin, TeamRole,
};

/// A session key's encoding (base64url of its 32 bytes). Serialized as the
/// string itself; `Debug` never prints it, so no `{:?}` of the system state or
/// of a command logs a key that signs every session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretKey(pub String);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionKey {
    pub version: u32,
    pub key: SecretKey,
    /// Unix seconds; the leader rotates the key once it is old enough.
    #[serde(default)]
    pub created_at: u64,
}

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
    /// Nodes the operator removed (`RemoveNode`): never registered again from
    /// `--peers`, only by `/ops/nodes`.
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
    pub session_keys: Vec<SessionKey>,
    pub orgs: BTreeMap<OrgId, Org>,
    /// invite token hash -> invite
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
    /// When each session in `revoked_sessions` was revoked (Unix seconds), so
    /// it can be forgotten once no token of it can still be valid.
    #[serde(default)]
    pub revoked_at: BTreeMap<String, u64>,
    /// Refresh tokens already exchanged (hash -> whose session, until when): a
    /// second use is a stolen token, and revokes the session.
    #[serde(default)]
    pub rotated_refresh: BTreeMap<String, RefreshRecord>,
}

pub use calimero_client::attest::{Authority, Governance};

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

/// How long a revoked session is remembered: past every token it could have
/// minted (sessions live 15 minutes), with room for clock differences.
pub const REVOKED_SESSION_KEEP_SECS: u64 = 60 * 60;

impl SystemState {
    /// The releases approved to hold `app`'s data: its organization's, or its
    /// owner's for an app outside one. Empty means none was ever recorded
    /// (state from before approvals), which leaves the app unrestricted.
    #[must_use]
    pub fn approved_releases(&self, app: &AppRecord) -> Option<&BTreeSet<String>> {
        self.approved_for(app.org.as_deref(), &app.owner)
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

    /// Whether `account` may upgrade `app`: its owner, or, for an org app,
    /// whoever [manages the organization's apps](Self::manages_apps) on the
    /// app's team.
    #[must_use]
    pub fn may_upgrade(&self, app: &AppRecord, account: &str) -> bool {
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

    /// Whether a node on `release` may hold `app`'s data. `None` is a node
    /// whose release is not known yet, which may not.
    #[must_use]
    pub fn release_may_hold(&self, app: &AppRecord, release: Option<&str>) -> bool {
        match self.approved_releases(app) {
            None => true,
            Some(set) => release.is_some_and(|r| set.contains(r)),
        }
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
    #[must_use]
    pub fn current_session_key(&self) -> Option<(u32, [u8; 32])> {
        let k = self.session_keys.last()?;
        Some((k.version, key32(&k.key.0)?))
    }

    #[must_use]
    pub fn session_key(&self, version: u32) -> Option<[u8; 32]> {
        self.session_keys
            .iter()
            .find(|k| k.version == version)
            .and_then(|k| key32(&k.key.0))
    }

    #[must_use]
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

fn key32(s: &str) -> Option<[u8; 32]> {
    let v = calimero_identity::from_b64url(s).ok()?;
    v.try_into().ok()
}

/// A change to the system group.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum SystemCmd {
    RegisterNode {
        node: NodeRecord,
    },
    /// Out of the directory; refused while an app keeps a replica on it.
    RemoveNode {
        id: u64,
    },
    PutBundle {
        hash: String,
        size: u64,
        actor: AccountId,
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
        /// When it was proposed (Unix seconds), for the app's upgrade record.
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
    SetReplicas {
        app_id: AppId,
        replicas: Vec<u64>,
    },
    SetAccess {
        app_id: AppId,
        access: Access,
        actor: AccountId,
    },
    CreateAccount {
        account: Account,
    },
    AddPasskey {
        account: AccountId,
        credential: Credential,
    },
    /// Remove a passkey and revoke every session opened with it.
    RemovePasskey {
        account: AccountId,
        credential_id: String,
        #[serde(default)]
        now: u64,
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
    AddSessionKey {
        version: u32,
        key: SecretKey,
        #[serde(default)]
        created_at: u64,
    },
    SetQuotas {
        quotas: Quotas,
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
    },
    /// Where the operator holds release approvals, it approves (or
    /// withdraws) a mero-tee release for every app's data.
    SetOperatorRelease {
        version: String,
        approved: bool,
    },
    /// The owner approves (or withdraws) a mero-tee release for an org's apps,
    /// or for their own apps outside one. Logged with the passkey consent.
    SetReleaseApproval {
        scope: ReleaseScope,
        version: String,
        approved: bool,
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
    },
    RemoveOrgMember {
        org: OrgId,
        account: AccountId,
        actor: AccountId,
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
    },
    /// The capabilities members hold unless set for them.
    SetDefaultCapabilities {
        org: OrgId,
        capabilities: u32,
        actor: AccountId,
    },
    RemoveTeamMember {
        org: OrgId,
        team: TeamId,
        account: AccountId,
        actor: AccountId,
    },
    CreateInvite {
        hash: String,
        invite: Invite,
        actor: AccountId,
    },
    RedeemInvite {
        hash: String,
        account: AccountId,
        now: u64,
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

    #[allow(clippy::too_many_lines, reason = "one arm per command")]
    fn run(&mut self, cmd: SystemCmd) -> Result<Value, &'static str> {
        let s = &mut self.state;
        match cmd {
            SystemCmd::RegisterNode { node } => {
                let _was = s.removed_nodes.remove(&node.id);
                let _prev = s.nodes.insert(node.id, node);
                Ok(Value::Null)
            }
            SystemCmd::RemoveNode { id } => {
                if s.apps.values().any(|a| a.replicas.contains(&id)) {
                    return Err("an app still keeps a replica on that node; move it first");
                }
                let _gone = s.nodes.remove(&id).ok_or("no such node")?;
                let _new = s.removed_nodes.insert(id);
                Ok(Value::Null)
            }
            SystemCmd::PutBundle { hash, size, actor } => {
                if !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
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
            SystemCmd::StartUpgradeRun { org, run } => {
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
            SystemCmd::SetReplicas { app_id, replicas } => {
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
            SystemCmd::CreateApiKey { hash, record } => {
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
            SystemCmd::SetQuotas { quotas } => {
                s.quotas = quotas;
                Ok(Value::Null)
            }
            SystemCmd::AddSessionKey {
                version,
                key,
                created_at,
            } => {
                if s.session_keys.iter().any(|k| k.version >= version) {
                    return Err("stale session key version");
                }
                s.session_keys.push(SessionKey {
                    version,
                    key,
                    created_at,
                });
                // Keep the current and previous key only.
                let excess = s.session_keys.len().saturating_sub(2);
                drop(s.session_keys.drain(..excess));
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
            } => {
                if s.governance.is_some() || !s.accounts.is_empty() {
                    return Err("the cluster's governance is fixed at genesis");
                }
                if governance.releases == Authority::Operator {
                    s.operator_releases = release.into_iter().collect();
                }
                s.governance = Some(governance.clone());
                Ok(json!({ "governance": governance }))
            }
            SystemCmd::SetOperatorRelease { version, approved } => {
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
                        let o = s.orgs.get_mut(org).ok_or("no such organization")?;
                        if o.role(&actor) != Some(OrgRole::Owner) {
                            return Err("only the organization's owner approves releases");
                        }
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
                hash,
                invite,
                actor,
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
                let _prev = s.invites.insert(hash, invite);
                Ok(Value::Null)
            }
            SystemCmd::RedeemInvite { hash, account, now } => {
                if !s.accounts.contains_key(&account) {
                    return Err("unknown account");
                }
                let inv = s.invites.get_mut(&hash).ok_or("unknown invite")?;
                if inv.expires_at <= now || inv.uses_left == 0 {
                    return Err("invite expired");
                }
                inv.uses_left -= 1;
                let inv = inv.clone();
                if inv.uses_left == 0 {
                    let _prev = s.invites.remove(&hash);
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
                Err(e) => err(e),
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

    #[test]
    fn debug_output_never_carries_a_session_key() {
        use super::{SecretKey, SessionKey, SystemCmd};
        let key = SecretKey("c2VjcmV0LXNlc3Npb24ta2V5".to_owned());
        let stored = SessionKey {
            version: 1,
            key: key.clone(),
            created_at: 0,
        };
        let cmd = SystemCmd::AddSessionKey {
            version: 1,
            key: key.clone(),
            created_at: 0,
        };
        for printed in [format!("{stored:?}"), format!("{cmd:?}")] {
            assert!(!printed.contains(&key.0), "{printed}");
            assert!(printed.contains("<redacted>"), "{printed}");
        }
        // The wire and stored form is the string itself, as before.
        assert_eq!(
            serde_json::to_value(&stored).unwrap()["key"],
            serde_json::json!(key.0)
        );
    }
    use serde_json::json;

    use super::{Authority, Governance, ReleaseScope, SystemCmd, SystemMachine, SystemState};

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
        }
    }

    fn all(who: Authority) -> Governance {
        Governance {
            releases: who,
            app_upgrades: who,
            outbound: who,
        }
    }

    /// The rules are written once, before anyone has an account, and an
    /// operator who restarts the cluster with other ones changes nothing.
    #[test]
    fn governance_is_fixed_at_genesis() {
        let mut m = SystemMachine::default();
        assert!(
            m.run(account("alice")).is_err(),
            "an account before genesis"
        );
        let _ok = m.run(genesis(all(Authority::Operator))).unwrap();
        let _ok = m.run(account("alice")).unwrap();
        assert!(m.run(genesis(all(Authority::Owner))).is_err());
        assert_eq!(m.state.governance(), all(Authority::Operator));

        // Even before any account, a second genesis is refused.
        let mut m = SystemMachine::default();
        let _ok = m.run(genesis(all(Authority::Owner))).unwrap();
        assert!(m.run(genesis(all(Authority::Operator))).is_err());
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
        let _ok = m.run(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob"] {
            let _ok = m.run(account(who)).unwrap();
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
        };
        assert!(reaches(&m), "a direct row");

        let _ok = m
            .run(SystemCmd::RemoveTeamMember {
                org: "org".into(),
                team: "open".into(),
                account: "bob".into(),
                actor: "bob".into(),
            })
            .unwrap();
        assert!(!reaches(&m), "inheritance does not put a leaver back");
        assert!(
            m.run(join("bob")).is_err(),
            "nor may they rejoin themselves"
        );

        let _ok = m.run(join("alice")).unwrap();
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
        let _ok = m.run(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob", "carol", "dave", "erin", "frank"] {
            let _ok = m.run(account(who)).unwrap();
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
                m.run(upgrade(app, actor)).is_ok(),
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
            assert_eq!(m.run(outbound(actor)).is_ok(), may, "{actor} approving");
        }
    }

    fn check(g: &Governance) {
        let op = |a: Authority| a == Authority::Operator;
        let mut m = SystemMachine::default();
        let _ok = m.run(genesis(g.clone())).unwrap();
        let _ok = m.run(account("alice")).unwrap();
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
        let by_operator = m.run(upgrade(true, "")).is_ok();
        let by_owner = m.run(upgrade(false, "alice")).is_ok();
        assert_eq!(by_operator, op(g.app_upgrades), "{g:?}: operator upgrade");
        assert_eq!(by_owner, !op(g.app_upgrades), "{g:?}: owner upgrade");
        assert!(m.run(upgrade(false, "mallory")).is_err(), "{g:?}");

        // Release approvals.
        let ops_release = m.run(SystemCmd::SetOperatorRelease {
            version: "2.1.0".into(),
            approved: true,
        });
        let owner_release = m.run(SystemCmd::SetReleaseApproval {
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
        let ops_outbound = m.run(outbound(ReleaseScope::Cluster, "")).is_ok();
        let owner_outbound = m.run(outbound(ReleaseScope::Account, "alice")).is_ok();
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
        let _ok = m.run(genesis(all(Authority::Owner))).unwrap();
        for who in ["alice", "bob", "carol"] {
            let _ok = m.run(account(who)).unwrap();
        }
        let install = |by: &str| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: PackageVersion {
                installed_by: by.into(),
                ..release.clone()
            },
        };
        let _ok = m.run(install("alice")).unwrap();
        let snapshot = serde_json::to_value(&m.state).unwrap();
        assert_eq!(snapshot["packages"]["p"]["versions"]["1.0.0"], old);
        let seen =
            |s: &SystemState, who: &str| s.sees_release(&s.packages["p"].versions["1.0.0"], who);
        assert!(seen(&m.state, "alice"));
        assert!(!seen(&m.state, "bob"), "another account, no shared org");
        assert!(!seen(&m.state, OPERATOR));

        // Installed again: by Bob (twice), then by the operator, which puts
        // it in the cluster's catalog; Alice stays first.
        let _ok = m.run(install("bob")).unwrap();
        let _ok = m.run(install("bob")).unwrap();
        assert!(seen(&m.state, "bob"));
        assert!(!seen(&m.state, "carol") && !seen(&m.state, OPERATOR));
        let _ok = m.run(install(OPERATOR)).unwrap();
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
        assert!(m.run(other).is_err());

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
}
