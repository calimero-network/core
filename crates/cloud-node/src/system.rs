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
    Access, Account, AccountId, ApiKeyRecord, AppId, AppRecord, Invite, NodeRecord, Org, OrgId,
    OrgRole, RefreshRecord, Team, TeamId, TeamJoin, TeamRole,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionKey {
    pub version: u32,
    pub key: String,
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
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            max_apps_per_account: 100,
            max_bundle_bytes_per_account: 1024 * 1024 * 1024,
            max_api_keys_per_account: 50,
            max_orgs_per_account: 20,
        }
    }
}

/// The prefix of every apply error that is a quota refusal.
pub const QUOTA_PREFIX: &str = "quota exceeded";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleRecord {
    pub hash: String,
    /// The WASM itself lives in each node's blob store, under `hash`.
    pub size: u64,
    pub uploaded_by: AccountId,
}

/// Everything the system group replicates.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemState {
    pub nodes: BTreeMap<u64, NodeRecord>,
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
}

impl SystemState {
    #[must_use]
    pub fn current_session_key(&self) -> Option<(u32, [u8; 32])> {
        let k = self.session_keys.last()?;
        Some((k.version, key32(&k.key)?))
    }

    #[must_use]
    pub fn session_key(&self, version: u32) -> Option<[u8; 32]> {
        self.session_keys
            .iter()
            .find(|k| k.version == version)
            .and_then(|k| key32(&k.key))
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
    PutBundle {
        hash: String,
        size: u64,
        actor: AccountId,
    },
    CreateApp {
        app: AppRecord,
    },
    UpgradeApp {
        app_id: AppId,
        bundle: String,
        actor: AccountId,
    },
    TransferApp {
        app_id: AppId,
        to: AccountId,
        actor: AccountId,
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
    RemovePasskey {
        account: AccountId,
        credential_id: String,
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
    },
    AddSessionKey {
        version: u32,
        key: String,
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
        #[serde(default)]
        parent: Option<Option<TeamId>>,
        #[serde(default)]
        metadata: Option<Value>,
        actor: AccountId,
    },
    /// Delete a team, the teams below it and their apps (v1's cascading
    /// `delete_group`).
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
                let _prev = s.nodes.insert(node.id, node);
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
                });
                Ok(json!({ "hash": hash }))
            }
            SystemCmd::CreateApp { mut app } => {
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
                    let member = s.orgs.get(org).and_then(|o| o.role(&app.owner));
                    if !member.is_some_and(|r| r >= OrgRole::Member) {
                        return Err("owner is not a member of the organization");
                    }
                }
                if let Access::Team { team } = &app.access {
                    let o = app.org.as_ref().and_then(|o| s.orgs.get(o));
                    // A team's app is created by someone who writes the team
                    // (v1: a context in a group its creator belongs to).
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
                actor,
            } => {
                if !s.bundles.contains_key(&bundle) {
                    return Err("unknown bundle");
                }
                let can = s.apps.get(&app_id).is_some_and(|a| {
                    a.owner == actor
                        || a.org.as_ref().is_some_and(|o| {
                            s.orgs
                                .get(o)
                                .and_then(|o| o.role(&actor))
                                .is_some_and(|r| r >= OrgRole::Admin)
                        })
                });
                if !can {
                    return Err("only the owner or an org admin may upgrade");
                }
                if let Some(a) = s.apps.get_mut(&app_id) {
                    a.bundle = bundle;
                }
                Ok(Value::Null)
            }
            SystemCmd::TransferApp { app_id, to, actor } => {
                let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
                if app.owner != actor {
                    return Err("only the owner may transfer");
                }
                if !s.accounts.contains_key(&to) {
                    return Err("unknown account");
                }
                app.owner = to;
                Ok(Value::Null)
            }
            SystemCmd::DeleteApp { app_id, actor } => {
                let app = s.apps.get(&app_id).ok_or("no such app")?;
                if app.owner != actor {
                    return Err("only the owner may delete");
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
                if s.accounts.contains_key(&account.account_id) {
                    return Err("account exists");
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
            } => {
                let acc = s.accounts.get_mut(&account).ok_or("unknown account")?;
                if acc.passkeys.len() <= 1 {
                    return Err("cannot remove the last passkey");
                }
                acc.passkeys.retain(|c| c.id != credential_id);
                let _prev = s.credentials.remove(&credential_id);
                Ok(Value::Null)
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
                let _prev = s.refresh.insert(new_hash, record);
                Ok(Value::Null)
            }
            SystemCmd::RevokeSession { session, actor } => {
                s.refresh
                    .retain(|_, r| !(r.session == session && r.account == actor));
                let _new = s.revoked_sessions.insert(session);
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
                while s.session_keys.len() > 2 {
                    let _old = s.session_keys.remove(0);
                }
                Ok(Value::Null)
            }
            SystemCmd::CreateOrg {
                org_id,
                name,
                actor,
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
                    },
                );
                Ok(json!({ "orgId": org_id }))
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
                // CREATE_TEAM at the top (v1: CAN_CREATE_SUBGROUP, root only).
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
                // Its creator leads it (v1: the creator is the subgroup's admin).
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
                // Leaving is one's own; a removal also keeps them from coming
                // back by inheritance.
                if actor != account {
                    let _new = t.removed.insert(account);
                }
                Ok(Value::Null)
            }
            SystemCmd::CreateInvite {
                hash,
                invite,
                actor,
            } => {
                let o = s.orgs.get(&invite.org).ok_or("no such organization")?;
                let role = o.role(&actor).ok_or("not a member")?;
                if role < OrgRole::Admin && !invite.teams.iter().all(|t| o.manages_team(&actor, t))
                {
                    return Err("not allowed to invite");
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
    fn apply(&mut self, _index: u64, command: &[u8]) -> Vec<u8> {
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
