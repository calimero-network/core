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
}

impl Caller {
    #[must_use]
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::Account { account } | Self::ApiKey { account, .. } => Some(account),
            Self::Public => None,
        }
    }

    /// The 32 bytes the runtime exposes as `env::account_id()`.
    #[must_use]
    pub fn account_bytes(&self) -> [u8; 32] {
        let mut out = [0; 32];
        if let Some(a) = self.account() {
            if let Ok(b) = calimero_identity::from_b64url(a) {
                let n = b.len().min(32);
                out[..n].copy_from_slice(&b[..n]);
            }
        }
        out
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
    /// Methods that need a fresh passkey assertion (`#[app::confirm]`).
    #[serde(default)]
    pub confirm_methods: Vec<String>,
    pub replicas: Vec<u64>,
    pub created_at: u64,
    /// The arguments `init` ran with (JSON; `null` for none).
    #[serde(default)]
    pub init_args: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub account_id: AccountId,
    pub name: String,
    pub created_at: u64,
    pub passkeys: Vec<Credential>,
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
    /// Every org member (not guests) counts as a member.
    Everyone,
    /// Explicit members only.
    Invite,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Org {
    pub org_id: OrgId,
    pub name: String,
    pub members: std::collections::BTreeMap<AccountId, OrgRole>,
    pub teams: std::collections::BTreeMap<TeamId, Team>,
}

impl Org {
    #[must_use]
    pub fn role(&self, account: &str) -> Option<OrgRole> {
        self.members.get(account).copied()
    }

    #[must_use]
    pub fn in_team(&self, account: &str, team: &str) -> bool {
        let Some(role) = self.role(account) else {
            return false;
        };
        let Some(t) = self.teams.get(team) else {
            return false;
        };
        match t.join {
            TeamJoin::Everyone => role >= OrgRole::Member || t.members.contains_key(account),
            TeamJoin::Invite => t.members.contains_key(account),
        }
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

/// One event committed with a call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub index: u64,
    pub seq: u32,
    pub kind: String,
    pub data: serde_json::Value,
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
