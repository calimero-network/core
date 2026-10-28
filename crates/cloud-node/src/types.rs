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
    /// The app itself: the node running an event handler, a timer, or an
    /// `xcall` from app `app` (then the caller is the calling app), on the
    /// wasmer executor. Never from a client: the API builds callers from
    /// credentials.
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

    /// The 32 bytes the wasmer runtime exposes as `env::account_id()`.
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
    /// for an open team, everyone who reaches its parent (v1's subgroup).
    Team { team: TeamId },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRecord {
    pub id: u64,
    pub addr: String,
    #[serde(default)]
    pub zone: String,
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
    /// Methods that need a fresh passkey assertion (`#[app::confirm]`), for an
    /// app on the wasmer executor; a component's come from its ABI.
    #[serde(default)]
    pub confirm_methods: Vec<String>,
    /// The executor its bundle runs on, decided once from the bundle's WASM when
    /// the app is created. Absent (a record written before components existed):
    /// wasmer.
    #[serde(default, skip_serializing_if = "Executor::is_wasmer")]
    pub executor: Executor,
    pub replicas: Vec<u64>,
    pub created_at: u64,
    /// The arguments `init` ran with (JSON; `null` for none).
    #[serde(default)]
    pub init_args: serde_json::Value,
    /// The app instance's metadata (v1's context metadata): a title, an icon.
    /// Set by its owner or an admin of its organization.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// The last upgrade tried on this app, done or failed (v1's upgrade status).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upgrade: Option<UpgradeRecord>,
}

/// Which executor runs an app's code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Executor {
    /// A v1 core module (SDK 0.12), on the wasmer runtime (`app::Executor`).
    #[default]
    Wasmer,
    /// A `calimero:app` component (the v2 SDK), on wasmtime through the host
    /// core (`calimero-app-host`).
    Component,
}

impl Executor {
    /// The executor for `wasm`: a component (its header's layer field says so,
    /// `wasmparser::Parser::is_component`) runs on wasmtime; anything else is a
    /// core module for wasmer, which judges it as it does today.
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
}

/// An upgrade of many apps at once (v1's group upgrade, with cascade): every
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
    /// id: v1's device label.
    #[serde(default)]
    pub passkey_labels: std::collections::BTreeMap<String, String>,
    /// The account's own names for apps (v1's aliases): alias -> app id.
    #[serde(default)]
    pub aliases: std::collections::BTreeMap<String, AppId>,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Open (v1's `Open` subgroup): everyone who reaches the parent team (or,
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
    /// What a member holds unless an admin says otherwise.
    pub const DEFAULT: u32 = JOIN_OPEN_TEAMS;
}

/// A member's standing in one team (a direct row; v1's group member role).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamRole {
    /// Manages the team and the teams below it (v1's group admin).
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
    /// Members whose row is read-only (v1's `ReadOnly` role): they write none
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
    /// when someone adds them again (v1's removal from an `Open` subgroup).
    #[serde(default)]
    pub removed: std::collections::BTreeSet<AccountId>,
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
    /// The organization's own metadata (v1's group metadata), for its apps
    /// and frontends to read: a name's display form, a logo, settings.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// Per-member metadata (v1's member metadata): a display name, a role
    /// title. Set by the member, or by an admin.
    #[serde(default)]
    pub member_metadata: std::collections::BTreeMap<AccountId, serde_json::Value>,
    /// Organization-wide upgrade runs, the newest [`UPGRADE_RUNS_KEPT`].
    #[serde(default)]
    pub upgrade_runs: std::collections::BTreeMap<String, UpgradeRun>,
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
                let parent_caps = self
                    .teams
                    .get(p)
                    .and_then(|pt| pt.capabilities.get(account).copied())
                    .unwrap_or_else(|| self.org_caps(account));
                parent_caps & caps::JOIN_OPEN_TEAMS != 0 && self.in_team_at(account, p, depth + 1)
            }
            None => self.org_caps(account) & caps::JOIN_OPEN_TEAMS != 0,
        }
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
    /// row's capabilities there), or organization-wide when `team` is `None`.
    #[must_use]
    pub fn may(&self, account: &str, team: Option<&str>, cap: u32) -> bool {
        match team {
            Some(t) if self.manages_team(account, t) => true,
            Some(t) => {
                self.teams
                    .get(t)
                    .and_then(|tt| tt.capabilities.get(account).copied())
                    .unwrap_or_else(|| self.org_caps(account))
                    & cap
                    != 0
            }
            None => self.org_caps(account) & cap != 0,
        }
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
    /// `None` for an event of an app on wasmer, which goes to everyone who
    /// reaches the app, as before.
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
