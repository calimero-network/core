//! Types shared by the node's state machines and its API.

use calimero_identity::Credential;
use serde::{Deserialize, Serialize};

/// An account id: 32 random bytes, base64url.
pub type AccountId = String;
/// An app instance id: 16 random bytes, base64url.
pub type AppId = String;
/// An organization id: 16 random bytes, base64url.
pub type OrgId = String;
/// A team id: 16 random bytes, base64url.
pub type TeamId = String;

/// Who made a call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Caller {
    /// A person, signed in with a passkey.
    Account { account: AccountId },
    /// A machine, with an API key of `account`.
    ApiKey { key: String, account: AccountId },
    /// Nobody in particular (public apps).
    Public,
    /// The app itself: an `xcall` from app `app` (then the caller is the calling
    /// app). Never from a client: the API builds callers from credentials.
    App { app: AppId },
    /// The platform itself: a component's timer ticks (D34), which reach only
    /// methods whose ABI says `allowSystem`. Never from a client.
    System,
}

impl Caller {
    #[must_use]
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::Account { account } | Self::ApiKey { account, .. } => Some(account),
            Self::Public | Self::App { .. } | Self::System => None,
        }
    }

    /// The caller as 32 bytes: an account's id, an app's context id.
    #[must_use]
    pub fn account_bytes(&self) -> [u8; 32] {
        // An app calls as its context id, what it sees as `env::context_id()`.
        if let Self::App { app } = self {
            return calimero_identity::sha256(app.as_bytes());
        }
        let mut out = [0; 32];
        if let Some(a) = self.account() {
            if let Ok(b) = calimero_identity::from_b64url(a) {
                let n = b.len().min(32);
                out[..n].copy_from_slice(&b[..n]);
            }
        }
        out
    }

    /// The caller as the component host core sees it (the WIT `caller`, D13).
    #[must_use]
    pub fn to_host(&self) -> calimero_app_host::Caller {
        use calimero_app_host::Caller as H;
        match self {
            Self::Account { account } => H::Account(account.clone()),
            Self::ApiKey { key, account } => H::ApiKey {
                key_id: key.clone(),
                account: account.clone(),
            },
            Self::Public => H::Public,
            Self::App { app } => H::App(app.clone()),
            Self::System => H::System,
        }
    }
}

/// Who may reach an app at all. Finer rules are the app's own code.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Access {
    /// Only the owner.
    #[default]
    Owner,
    /// Members of the app's organization.
    Org,
    /// The owner and these accounts.
    Accounts { accounts: Vec<AccountId> },
    /// Anyone, signed in or not.
    Public,
    /// Members of a team of the app's organization: its direct members, and,
    /// for an open team, everyone who reaches its parent.
    Team { team: TeamId },
}

/// What a registered node is for: a replica holds apps and the cluster key, a
/// job node only runs compute for them and never holds either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeRole {
    #[default]
    Replica,
    Job,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRecord {
    pub id: u64,
    pub addr: String,
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub role: NodeRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRecord {
    pub app_id: AppId,
    pub name: String,
    pub group: u64,
    pub bundle: String,
    pub owner: AccountId,
    #[serde(default)]
    pub org: Option<OrgId>,
    #[serde(default)]
    pub access: Access,
    /// Methods that needed a fresh passkey assertion (`#[app::confirm]`), as an
    /// SDK 0.12 app's record names them; read by nothing now (a component's come
    /// from its ABI), kept so a record writes as it read.
    #[serde(default)]
    pub confirm_methods: Vec<String>,
    /// The executor its bundle runs on, decided once from the bundle's WASM when
    /// the app is created: a component. Absent (a record written before components
    /// existed): wasmer, an SDK 0.12 app this node no longer runs.
    #[serde(default, skip_serializing_if = "Executor::is_wasmer")]
    pub executor: Executor,
    pub replicas: Vec<u64>,
    pub created_at: u64,
    /// The arguments `init` ran with (JSON; `null` for none).
    #[serde(default)]
    pub init_args: serde_json::Value,
    /// The compute component of the release `bundle` came from, set with `bundle` at create
    /// and upgrade; `None` when that release ships none (or `bundle` is a raw upload).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute_bundle: Option<String>,
    /// The app instance's metadata: a title, an icon.
    /// Set by its owner or an admin of its organization.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// The last upgrade tried on this app, done or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upgrade: Option<UpgradeRecord>,
    /// The deployment it was created from (`system::Deployment`), at a
    /// release that deployment offered then. Absent (a record from before,
    /// or an instance made from a release directly): none, and it is left
    /// out, so such a record writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
    /// The package release this instance runs, set at create (the release
    /// named, or the one its deployment offered) and at every upgrade, one
    /// to a release with the same WASM included: releases may share a WASM
    /// and differ in their frontend, which works with its own release only,
    /// so `bundle` alone does not say which one it runs. Absent (a record
    /// from before, or a raw upload): the release is inferred from `bundle`
    /// (`report::release_of_bundle`), and it is left out, so such a record
    /// writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<crate::system::PackageRef>,
}

/// Which executor runs an app's code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Executor {
    /// An SDK 0.12 core module, which ran on wasmer. This node has no wasmer: such
    /// an app is listed and can be deleted, and every call, view and upgrade of it
    /// is `410 UNSUPPORTED_ENGINE` ([`crate::error::ApiError::unsupported_engine`]).
    /// The default, as a record from before components names it by omission.
    #[default]
    Wasmer,
    /// A `calimero:app` component (the v2 SDK), on wasmtime through the host
    /// core (`calimero-app-host`).
    Component,
}

impl Executor {
    /// The executor for `wasm`: a component (its header's layer field says so,
    /// `wasmparser::Parser::is_component`) runs on wasmtime; anything else is a
    /// core module, which nothing here runs (`400 UNSUPPORTED_BUNDLE`).
    #[must_use]
    pub fn of(wasm: &[u8]) -> Self {
        if wasmparser::Parser::is_component(wasm) {
            Self::Component
        } else {
            Self::Wasmer
        }
    }

    #[must_use]
    pub fn is_wasmer(&self) -> bool {
        *self == Self::Wasmer
    }
}

/// One upgrade of one app, as the directory remembers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeRecord {
    pub from: String,
    pub to: String,
    /// Unix seconds.
    pub at: u64,
    /// The account that upgraded it, or `operator`.
    pub by: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub error: serde_json::Value,
    /// The organization-wide run it was part of, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The upgrade key (its id) that made it, in place of a passkey.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// The release the app ran before (`AppRecord::release`), and the one it
    /// was moved to: `from` and `to` alone do not tell apart releases that
    /// share a WASM. Absent on a record from before, or for a raw upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_release: Option<crate::system::PackageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_release: Option<crate::system::PackageRef>,
}

/// An upgrade of many apps at once, with cascade: every
/// app of the organization (or of a team, and with `cascade` the teams below
/// it) that runs `package`, to `version`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeRun {
    pub run_id: String,
    pub package: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamId>,
    #[serde(default)]
    pub cascade: bool,
    /// The account that started it, or `operator`.
    pub by: String,
    pub started_at: u64,
    pub apps: Vec<AppId>,
    pub done: Vec<AppId>,
    /// App -> why its upgrade failed.
    pub failed: std::collections::BTreeMap<AppId, serde_json::Value>,
    pub finished: bool,
}

/// How many upgrade runs an organization remembers.
pub const UPGRADE_RUNS_KEPT: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub account_id: AccountId,
    pub name: String,
    pub created_at: u64,
    pub passkeys: Vec<Credential>,
    /// mero-tee releases approved to hold the apps this account owns outside
    /// an organization (as [`Org::approved_releases`]).
    #[serde(default)]
    pub approved_releases: std::collections::BTreeSet<String>,
    /// Outbound URL prefixes approved for the apps this account owns outside
    /// an organization (as [`Org::outbound_approvals`]).
    #[serde(default)]
    pub outbound_approvals: std::collections::BTreeMap<String, OutboundApproval>,
    /// A name for each of the account's passkeys (its devices), by credential
    /// id: a device label.
    #[serde(default)]
    pub passkey_labels: std::collections::BTreeMap<String, String>,
    /// The account's own names for apps (aliases): alias -> app id.
    #[serde(default)]
    pub aliases: std::collections::BTreeMap<String, AppId>,
    /// When each passkey was added and last signed in, by credential id,
    /// shown to the account holder alone (`/auth/me`). Written at apply from
    /// the times the log entries carry; a passkey with no entry, or a time
    /// absent, was added or signed in before this was kept and reads as
    /// unknown. Left out when empty, so accounts from before, and the
    /// account a sign-up's passkey signs (`system::account_args`), read and
    /// write as they did.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub passkey_times: std::collections::BTreeMap<String, PasskeyTimes>,
}

/// When one passkey was added and last signed in (Unix seconds, as the node
/// that took the ceremony read its clock). `None`: not known, because it
/// happened before the node kept it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyTimes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<u64>,
    /// The last sign-in with it (a sign-up, a sign-in or a recovery's
    /// finish): the log entry each already writes. Refreshes do not move it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sign_in_at: Option<u64>,
}

impl PasskeyTimes {
    /// Records a sign-in at `at` (0: unknown, nothing changes); the latest
    /// wins, whatever order sign-ins committed in.
    pub fn signed_in(&mut self, at: u64) {
        if at > 0 {
            self.last_sign_in_at = Some(self.last_sign_in_at.map_or(at, |t| t.max(at)));
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyRecord {
    pub key_id: String,
    pub account: AccountId,
    pub label: String,
    /// `None` = every app the account can reach.
    #[serde(default)]
    pub apps: Option<Vec<AppId>>,
    /// `None` = every method.
    #[serde(default)]
    pub methods: Option<Vec<String>>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub allow_confirm: bool,
    pub expires_at: u64,
    /// A key that upgrades one app and does nothing else: no passkey on each
    /// upgrade, only to signed releases of one package by its pinned signer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrades: Option<UpgradeScope>,
}

/// What an upgrade key ([`ApiKeyRecord::upgrades`]) may do: upgrade `app`
/// to a release of `package` signed by `signer`, proving each upgrade with
/// the Ed25519 key its secret stands for (`proof::api_key_public`), so a
/// node that saw only the log cannot forge one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradeScope {
    pub app: AppId,
    pub package: String,
    /// The package's pinned signer (`did:key`) when the key was made.
    pub signer: String,
    /// The key's public half, hex.
    pub proof_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRecord {
    pub account: AccountId,
    pub session: String,
    pub expires_at: u64,
    /// The passkey the session was opened with.
    #[serde(default)]
    pub passkey: Option<String>,
    /// The package whose frontend opened the session, which binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The registered origin that opened it (`SessionClaims::origin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The token's public key (`proof::refresh_key`, hex), which signs its
    /// exchange (`UseRefresh`). Empty in records from before: those are not
    /// exchanged, and their holders sign in again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// When the session was opened (Unix seconds), carried by every exchange
    /// of its token (`UseRefresh`). 0: opened before this was kept, unknown.
    /// Left out when 0, so the records from before, and the record a
    /// sign-up's passkey signs (`system::account_args`), read and write as
    /// they did.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub opened_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrgRole {
    Guest,
    Member,
    Admin,
    Owner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamJoin {
    /// Open: everyone who reaches the parent team (or,
    /// at the top, the organization) is a member too, unless an admin denied
    /// them `caps::JOIN_OPEN_TEAMS`.
    Everyone,
    /// Restricted: explicit members only.
    Invite,
}

/// What a member may do beyond their role: v1's `MemberCapabilities` bits, at
/// the same values. A team's lead and an organization admin hold them all.
pub mod caps {
    pub const CREATE_APP: u32 = 1 << 0;
    pub const INVITE_MEMBERS: u32 = 1 << 1;
    /// Inherited into open teams; denying it keeps a member out of them.
    pub const JOIN_OPEN_TEAMS: u32 = 1 << 2;
    pub const MANAGE_MEMBERS: u32 = 1 << 3;
    pub const MANAGE_APPLICATION: u32 = 1 << 4;
    /// Create a top-level team without being an admin.
    pub const CREATE_TEAM: u32 = 1 << 5;
    /// Delete a team (and what it holds) without being an admin.
    pub const DELETE_TEAM: u32 = 1 << 6;
    pub const MANAGE_VISIBILITY: u32 = 1 << 7;
    pub const MANAGE_METADATA: u32 = 1 << 8;
    /// Approve (or withdraw) the mero-tee releases that may hold the
    /// organization's apps, held organization-wide only. Bit 10, not 9: v1's
    /// `MemberCapabilities` spends 9 on `CAN_AUTHOR_ON_BEHALF`, so a client
    /// that sends v1's `ALL` does not hand out release approvals with it.
    pub const APPROVE_RELEASES: u32 = 1 << 10;
    /// What a member holds unless an admin says otherwise.
    pub const DEFAULT: u32 = JOIN_OPEN_TEAMS;
}

/// A member's standing in one team (a direct row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamRole {
    /// Manages the team and the teams below it.
    Lead,
    Member,
    /// Reads, never writes, the team's apps.
    ReadOnly,
}

fn default_caps() -> u32 {
    caps::DEFAULT
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub team_id: TeamId,
    pub name: String,
    #[serde(default)]
    pub parent: Option<TeamId>,
    pub join: TeamJoin,
    /// account -> is lead
    #[serde(default)]
    pub members: std::collections::BTreeMap<AccountId, bool>,
    /// Members whose row is read-only: they write none
    /// of the team's apps. Only a direct row counts.
    #[serde(default)]
    pub read_only: std::collections::BTreeSet<AccountId>,
    /// Capabilities per direct row ([`caps`]).
    #[serde(default)]
    pub capabilities: std::collections::BTreeMap<AccountId, u32>,
    /// What the app stores about the team (a folder's name and colour).
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// Members a manager removed: they do not come back by inheritance, only
    /// when someone adds them again.
    #[serde(default)]
    pub removed: std::collections::BTreeSet<AccountId>,
    /// Capabilities ([`caps`]) everyone in the team holds there on top of
    /// their organization-wide ones, unless their row sets its own: "members
    /// may invite others to this app". Absent in teams from before, and left
    /// out when unset, so such a team reads and writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_capabilities: Option<u32>,
}

impl Team {
    /// The package this team is the app team of: `metadata.app.package` on a
    /// top-level team ([`Org::app_team`]). One per package per organization.
    #[must_use]
    pub fn app_package(&self) -> Option<&str> {
        if self.parent.is_some() {
            return None;
        }
        self.metadata
            .get("app")
            .and_then(|a| a.get("package"))
            .and_then(serde_json::Value::as_str)
    }
}

/// How an organization holds its apps, derived from its app teams
/// ([`Team::app_package`]), never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrgMode {
    /// No app team: its apps use organization access, as every organization
    /// made before app teams does. Behaves as standalone.
    Legacy,
    /// Every app team is open: everyone in the organization has its apps.
    Standalone,
    /// Some app team is closed: each app lists who is in it, and members see
    /// only the people of the apps they are in.
    Shared,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Org {
    pub org_id: OrgId,
    pub name: String,
    pub members: std::collections::BTreeMap<AccountId, OrgRole>,
    pub teams: std::collections::BTreeMap<TeamId, Team>,
    /// Capabilities a member holds unless set for them.
    #[serde(default = "default_caps")]
    pub default_capabilities: u32,
    #[serde(default)]
    pub capabilities: std::collections::BTreeMap<AccountId, u32>,
    /// mero-tee releases the owner approved to hold this organization's apps
    /// (the release the org was created on, then each one the owner approves).
    /// A node on any other release is sent none of their data.
    #[serde(default)]
    pub approved_releases: std::collections::BTreeSet<String>,
    /// Outbound URL prefixes the owner approved, by package signer (`did:key`):
    /// an app's run reaches only a prefix its manifest declares and this holds.
    #[serde(default)]
    pub outbound_approvals: std::collections::BTreeMap<String, OutboundApproval>,
    /// The organization's own metadata, for its apps
    /// and frontends to read: a name's display form, a logo, settings.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// Per-member metadata: a display name, a role
    /// title. Set by the member, or by an admin.
    #[serde(default)]
    pub member_metadata: std::collections::BTreeMap<AccountId, serde_json::Value>,
    /// Organization-wide upgrade runs, the newest [`UPGRADE_RUNS_KEPT`].
    #[serde(default)]
    pub upgrade_runs: std::collections::BTreeMap<String, UpgradeRun>,
    /// The organization's recovery key and any recovery under way
    /// (`docs/specs/org-recovery.md`); absent for an organization that never
    /// set one, as every organization written before this field was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<OrgRecovery>,
}

/// An organization's recovery key: whoever holds its seed can hand the
/// organization to an account, [`RECOVERY_DELAY_SECS`] after starting, unless
/// the owner cancels first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgRecovery {
    /// The key's Ed25519 public half, base64url
    /// ([`calimero_identity::recovery`]).
    pub public_key: String,
    /// When the owner set it (the setting command's time, Unix seconds).
    pub set_at: u64,
    /// A recovery under way.
    #[serde(default)]
    pub pending: Option<PendingRecovery>,
}

/// A recovery started with the seed, not finished or cancelled yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRecovery {
    /// The account the organization goes to.
    pub account: AccountId,
    /// When it started (Unix seconds).
    pub started_at: u64,
    /// The earliest time it may finish.
    pub ready_at: u64,
}

/// How long a started recovery, an organization's or an account's, waits
/// before it may finish: the owner's time to notice and cancel (72 hours).
pub const RECOVERY_DELAY_SECS: u64 = 72 * 60 * 60;

/// An account recovery started with the account's recovery code, not
/// finished or cancelled yet (`docs/specs/account-recovery.md`): the passkey
/// it adds, [`RECOVERY_DELAY_SECS`] after the start (the same delay as an
/// organization's), unless one of the account's passkeys cancels first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAccountRecovery {
    /// The new passkey, registered at the start.
    pub credential: Credential,
    /// When it started (the start command's time, Unix seconds).
    pub started_at: u64,
    /// The earliest time it may finish.
    pub ready_at: u64,
}

impl PendingAccountRecovery {
    /// What the start answers and `/auth/me` shows: never the credential's
    /// public key.
    #[must_use]
    pub fn json(&self, account: &str) -> serde_json::Value {
        serde_json::json!({
            "accountId": account,
            "credentialId": self.credential.id,
            "startedAt": self.started_at,
            "readyAt": self.ready_at,
        })
    }
}

/// The most bytes of JSON one metadata value holds (org, member, team, app).
pub const MAX_METADATA_BYTES: usize = 16 * 1024;

/// What an owner approved for one publisher's apps to call out to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundApproval {
    /// The approved URL prefixes.
    pub prefixes: Vec<String>,
    /// Every prefix the owner was shown, approved or not: one declared later
    /// reads as awaiting approval, one left out as not approved.
    pub reviewed: Vec<String>,
}

impl Org {
    #[must_use]
    pub fn role(&self, account: &str) -> Option<OrgRole> {
        self.members.get(account).copied()
    }

    /// Whether `account` is a member of `team`: a direct row, or, for an
    /// open team, a member of its parent (the organization at the top) who
    /// holds [`caps::JOIN_OPEN_TEAMS`] there.
    #[must_use]
    pub fn in_team(&self, account: &str, team: &str) -> bool {
        self.in_team_at(account, team, 0)
    }

    fn in_team_at(&self, account: &str, team: &str, depth: usize) -> bool {
        if self.role(account).is_none() || depth > 32 {
            return false;
        }
        let Some(t) = self.teams.get(team) else {
            return false;
        };
        if t.members.contains_key(account) {
            return true;
        }
        if t.join != TeamJoin::Everyone || t.removed.contains(account) {
            return false;
        }
        match &t.parent {
            Some(p) => {
                // The team's default counts only for those in it, which the
                // recursion below requires anyway.
                let parent_caps = self.teams.get(p).map_or_else(
                    || self.org_caps(account),
                    |pt| self.row_caps(pt, account, true),
                );
                parent_caps & caps::JOIN_OPEN_TEAMS != 0 && self.in_team_at(account, p, depth + 1)
            }
            // Where apps have app teams a guest has only the apps of the
            // teams they are in: open teams are not theirs by inheritance.
            None => !self.app_only(account) && self.org_caps(account) & caps::JOIN_OPEN_TEAMS != 0,
        }
    }

    /// What `account` holds in `t`: their row's own capabilities, else their
    /// organization-wide ones with the team's default on top when they are in
    /// it (`in_team`).
    fn row_caps(&self, t: &Team, account: &str, in_team: bool) -> u32 {
        if let Some(c) = t.capabilities.get(account) {
            return *c;
        }
        let default = if in_team {
            t.default_capabilities.unwrap_or(0)
        } else {
            0
        };
        self.org_caps(account) | default
    }

    /// The capabilities `account`'s row in `team` holds, or would hold
    /// without one ([`Self::may`] below a lead).
    #[must_use]
    pub fn team_caps(&self, account: &str, team: &str) -> u32 {
        self.teams.get(team).map_or_else(
            || self.org_caps(account),
            |t| self.row_caps(t, account, self.in_team(account, team)),
        )
    }

    /// `account`'s direct row in `team`, if any.
    #[must_use]
    pub fn team_role(&self, account: &str, team: &str) -> Option<TeamRole> {
        let t = self.teams.get(team)?;
        let lead = *t.members.get(account)?;
        Some(if lead {
            TeamRole::Lead
        } else if t.read_only.contains(account) {
            TeamRole::ReadOnly
        } else {
            TeamRole::Member
        })
    }

    /// Whether `account` may write `team`'s apps: a member whose direct row
    /// there is not read-only.
    #[must_use]
    pub fn writes_team(&self, account: &str, team: &str) -> bool {
        self.in_team(account, team) && self.team_role(account, team) != Some(TeamRole::ReadOnly)
    }

    /// Whether `account` sees `team` at all (its name, members, metadata): a
    /// member, or someone who manages it. A restricted team is invisible to
    /// everyone else, and so is everything under it they do not reach.
    #[must_use]
    pub fn sees_team(&self, account: &str, team: &str) -> bool {
        self.in_team(account, team) || self.manages_team(account, team)
    }

    /// The organization-wide capabilities of `account` ([`caps`]).
    #[must_use]
    pub fn org_caps(&self, account: &str) -> u32 {
        if self.role(account).is_some_and(|r| r >= OrgRole::Admin) {
            return u32::MAX;
        }
        self.capabilities
            .get(account)
            .copied()
            .unwrap_or(self.default_capabilities)
    }

    /// Whether `account` holds `cap` for `team` (a team they manage, or their
    /// row's capabilities there, else the team's default while they are in it
    /// on top of their organization-wide ones), or organization-wide when
    /// `team` is `None`.
    #[must_use]
    pub fn may(&self, account: &str, team: Option<&str>, cap: u32) -> bool {
        match team {
            Some(t) if self.manages_team(account, t) => true,
            Some(t) => self.team_caps(account, t) & cap != 0,
            None => self.org_caps(account) & cap != 0,
        }
    }

    /// Whether `account` is a guest of an organization whose apps have app
    /// teams: they reach only the apps of the teams they have a row in (no
    /// organization-access app, no open top-level team). In a legacy
    /// organization, the organization is its one app, and a guest reaches its
    /// organization-access apps as before.
    #[must_use]
    pub fn app_only(&self, account: &str) -> bool {
        self.role(account) == Some(OrgRole::Guest) && self.mode() != OrgMode::Legacy
    }

    /// The app team of `package` here, if it has one.
    #[must_use]
    pub fn app_team(&self, package: &str) -> Option<&Team> {
        self.teams
            .values()
            .find(|t| t.app_package() == Some(package))
    }

    /// The organization's mode, from its app teams.
    #[must_use]
    pub fn mode(&self) -> OrgMode {
        let mut apps = self.teams.values().filter(|t| t.app_package().is_some());
        let Some(first) = apps.next() else {
            return OrgMode::Legacy;
        };
        if std::iter::once(first)
            .chain(apps)
            .any(|t| t.join == TeamJoin::Invite)
        {
            OrgMode::Shared
        } else {
            OrgMode::Standalone
        }
    }

    /// Whether `viewer` sees `other` in the organization's member directory
    /// (`GET /orgs/{org}`): admins and the owner see everyone; a member sees
    /// everyone unless the organization is shared, and then the people of the
    /// teams they are in and its admins; a guest, who has only the apps of
    /// their teams, sees only the people of those teams. Everyone sees
    /// themselves.
    #[must_use]
    pub fn sees_member(&self, viewer: &str, other: &str) -> bool {
        let Some(role) = self.role(viewer) else {
            return false;
        };
        if viewer == other || role >= OrgRole::Admin {
            return true;
        }
        if role >= OrgRole::Member {
            if self.mode() != OrgMode::Shared {
                return true;
            }
            if self.role(other).is_some_and(|r| r >= OrgRole::Admin) {
                return true;
            }
        }
        self.teams
            .keys()
            .any(|t| self.in_team(viewer, t) && self.in_team(other, t))
    }

    /// `team` and every team below it.
    #[must_use]
    pub fn subtree(&self, team: &str) -> Vec<TeamId> {
        let mut out = vec![team.to_owned()];
        let mut i = 0;
        while i < out.len() && out.len() <= self.teams.len() {
            let cur = out[i].clone();
            out.extend(
                self.teams
                    .values()
                    .filter(|t| t.parent.as_deref() == Some(cur.as_str()))
                    .map(|t| t.team_id.clone()),
            );
            i += 1;
        }
        out
    }

    /// Whether `account` may manage `team`: an org admin, the team's lead, or
    /// the lead of an ancestor team.
    #[must_use]
    pub fn manages_team(&self, account: &str, team: &str) -> bool {
        if self.role(account).is_some_and(|r| r >= OrgRole::Admin) {
            return true;
        }
        let mut cur = Some(team.to_owned());
        let mut hops = 0;
        while let Some(t) = cur {
            let Some(rec) = self.teams.get(&t) else { break };
            if rec.members.get(account) == Some(&true) {
                return true;
            }
            cur = rec.parent.clone();
            hops += 1;
            if hops > 32 {
                break;
            }
        }
        false
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub org: OrgId,
    pub role: OrgRole,
    #[serde(default)]
    pub teams: Vec<TeamId>,
    pub expires_at: u64,
    pub uses_left: u32,
    /// Who made it: `CreateInvite`'s actor, set where it applies. Empty in
    /// invites from before, and left out then, so they write as they did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: AccountId,
    /// When it was made (Unix seconds, the creating node's clock); 0 and left
    /// out in invites from before.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub created_at: u64,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Who receives an event (R9). Resolved when the event is sent to a
/// subscriber, against the system state of that moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Audience {
    /// Everyone who reaches the app.
    All,
    /// The account that made the call.
    Caller,
    Accounts {
        accounts: Vec<AccountId>,
    },
    /// The members of a team of the app's organization.
    Team {
        team: TeamId,
    },
    /// Members, admins and the owner of the organization.
    OrgMembers {
        org: OrgId,
    },
    /// Admins and the owner of the organization.
    OrgAdmins {
        org: OrgId,
    },
}

impl From<calimero_app_host::Audience> for Audience {
    fn from(a: calimero_app_host::Audience) -> Self {
        use calimero_app_host::Audience as H;
        match a {
            H::All => Self::All,
            H::Caller => Self::Caller,
            H::Accounts(accounts) => Self::Accounts { accounts },
            H::Team(team) => Self::Team { team },
            H::OrgMembers(org) => Self::OrgMembers { org },
            H::OrgAdmins(org) => Self::OrgAdmins { org },
        }
    }
}

/// One event committed with a call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub index: u64,
    pub seq: u32,
    pub kind: String,
    pub data: serde_json::Value,
    /// A component's event: who receives it (R9), checked when it is sent.
    /// `None` for an event that goes to everyone who reaches the app (one an SDK
    /// 0.12 app recorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Audience>,
    /// The calling account, for [`Audience::Caller`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<AccountId>,
}

/// Seconds since the epoch.
#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Milliseconds since the epoch.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// For an `Option<Option<T>>` field: absent is `None`, `null` is `Some(None)`
/// ("clear it"), a value is `Some(Some(v))`. Plain serde reads `null` as
/// absent, which silently dropped "move this team to the top".
pub fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}
