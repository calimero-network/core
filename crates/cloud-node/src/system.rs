//! The system group: fleet, app directory, identity and organizations.
//!
//! Commands are applied in log order on every node. Each command that acts for
//! someone carries that `actor`, and `apply` checks their authority against
//! the state the command applies to, so the log alone decides every outcome.

use std::collections::{BTreeMap, BTreeSet};

use calimero_identity::Credential;
use calimero_log::{Bytes, Machine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::types::{
    Access, Account, AccountId, ApiKeyRecord, AppId, AppRecord, Invite, NodeRecord, Org, OrgId,
    OrgRole, RefreshRecord, Team, TeamId, TeamJoin,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionKey {
    pub version: u32,
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleRecord {
    pub hash: String,
    pub wasm: Bytes,
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
        wasm: Bytes,
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
            SystemCmd::PutBundle { hash, wasm, actor } => {
                if !s.accounts.contains_key(&actor) {
                    return Err("unknown account");
                }
                let _prev = s.bundles.entry(hash.clone()).or_insert(BundleRecord {
                    hash: hash.clone(),
                    wasm,
                    uploaded_by: actor,
                });
                Ok(json!({ "hash": hash }))
            }
            SystemCmd::CreateApp { mut app } => {
                if !s.bundles.contains_key(&app.bundle) {
                    return Err("unknown bundle");
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
                let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
                if app.owner != actor {
                    return Err("only the owner may change access");
                }
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
            SystemCmd::AddSessionKey { version, key } => {
                if s.session_keys.iter().any(|k| k.version >= version) {
                    return Err("stale session key version");
                }
                s.session_keys.push(SessionKey { version, key });
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
                let mut members = BTreeMap::new();
                let _prev = members.insert(actor, OrgRole::Owner);
                let _prev = s.orgs.insert(
                    org_id.clone(),
                    Org {
                        org_id: org_id.clone(),
                        name,
                        members,
                        teams: BTreeMap::new(),
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
                for t in o.teams.values_mut() {
                    let _prev = t.members.remove(&account);
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
                let allowed = match &parent {
                    Some(p) => o.teams.contains_key(p) && o.manages_team(&actor, p),
                    None => o.role(&actor).is_some_and(|r| r >= OrgRole::Admin),
                };
                if !allowed {
                    return Err("not allowed to create this team");
                }
                let _prev = o.teams.insert(
                    team_id.clone(),
                    Team {
                        team_id: team_id.clone(),
                        name,
                        parent,
                        join,
                        members: BTreeMap::new(),
                    },
                );
                Ok(json!({ "teamId": team_id }))
            }
            SystemCmd::SetTeamMember {
                org,
                team,
                account,
                lead,
                actor,
            } => {
                let o = self.org_mut(&org)?;
                if !o.teams.contains_key(&team) {
                    return Err("no such team");
                }
                let joins_self = actor == account
                    && !lead
                    && o.teams
                        .get(&team)
                        .is_some_and(|t| t.join == TeamJoin::Everyone)
                    && o.role(&actor).is_some_and(|r| r >= OrgRole::Member);
                if !joins_self && !o.manages_team(&actor, &team) {
                    return Err("not allowed to manage this team");
                }
                if o.role(&account).is_none() {
                    return Err("only organization members join teams");
                }
                if let Some(t) = o.teams.get_mut(&team) {
                    let _prev = t.members.insert(account, lead);
                }
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
