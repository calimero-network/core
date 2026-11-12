//! The consent page's organization kinds beyond creating, inviting and
//! joining (`createOrg`, `createInvite`, `acceptInvite`, in the parent): what
//! an app's frontend, refused every organization route since #4934, asks the
//! person to approve for a "server" (an organization) it manages.
//!
//! - Membership: `leaveOrg`, `removeMember`, `setMemberRole`,
//!   `setMemberTeams`, `removeMemberTeams`, `revokeInvite`.
//! - Settings: `renameOrg`, `deleteOrg`, `setCapabilities` (a member's or a
//!   default), `setAllApps`.
//!
//! Each is judged at request time with the predicates the log applies (the
//! role, `MANAGE_MEMBERS`, a team's lead, admin), so nobody approves what the
//! log would refuse; [`act`] runs the API's own route function
//! (`api::remove_member`, ...), which judges again on fresh state and logs
//! exactly the API's entry with the API's consent. No log command or field is
//! new, so no feature gate is: the kinds whose routes need one
//! (`RenameOrg`, `AllApps`, `TeamMembersBatch`) ask for it at request time
//! too.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{attention, phrase, plain_date, role_words, stale, Acted};
use crate::api::{
    delete_org, remove_member, remove_member_teams, rename_org, revoke_invite, set_all_apps,
    set_capabilities, set_member_role, set_member_teams, CapabilitiesBody, ConfirmBody, Principal,
};
use crate::error::ApiError;
use crate::node::Node;
use crate::system::cluster_version::Feature;
use crate::system::{SystemState, TeamRow};
use crate::types::{caps, now_secs, Access, Org, OrgRole, TeamRole};

/// The longest organization, account, team or invite id an organization
/// request may name (ids are base64url of 16 bytes, invite keys 64 hex).
pub const MAX_SUBJECT_ID_BYTES: usize = 128;

// ---------------------------------------------------------------- subjects

/// `leaveOrg` and `deleteOrg`: an organization.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrgSubject {
    org: String,
}

/// `removeMember`: a member of an organization.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemberSubject {
    org: String,
    account: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetMemberRoleSubject {
    org: String,
    account: String,
    role: OrgRole,
}

/// `setMemberTeams`: one member's rows in up to `MAX_TEAM_BATCH` teams, as
/// `POST /orgs/{org}/members/{account}/teams` takes them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetMemberTeamsSubject {
    org: String,
    account: String,
    teams: Vec<TeamRow>,
}

/// `removeMemberTeams`: one member out of up to `MAX_TEAM_BATCH` teams (and
/// the teams below them).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveMemberTeamsSubject {
    org: String,
    account: String,
    teams: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeInviteSubject {
    org: String,
    /// The invite's key, as `GET /orgs/{org}/invites` lists it.
    invite_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameOrgSubject {
    org: String,
    name: String,
}

/// `setCapabilities`, both of `POST /orgs/{org}/capabilities`'s shapes: a
/// member's (`account`; organization-wide, or in `team`), or without an
/// account a default (the organization's, or `team`'s).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetCapabilitiesSubject {
    org: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    team: Option<String>,
    capabilities: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAllAppsSubject {
    org: String,
    account: String,
    all_apps: bool,
}

/// What an organization request asks, by kind.
pub(super) enum OrgAsk {
    Leave(OrgSubject),
    RemoveMember(MemberSubject),
    SetMemberRole(SetMemberRoleSubject),
    SetMemberTeams(SetMemberTeamsSubject),
    RemoveMemberTeams(RemoveMemberTeamsSubject),
    RevokeInvite(RevokeInviteSubject),
    Rename(RenameOrgSubject),
    Delete(OrgSubject),
    SetCapabilities(SetCapabilitiesSubject),
    SetAllApps(SetAllAppsSubject),
}

/// What approving an organization request does, with the facts the page
/// showed that could change before it is approved.
#[derive(Clone, Debug)]
pub(super) enum OrgAction {
    Leave {
        org: String,
    },
    RemoveMember {
        org: String,
        account: String,
    },
    SetMemberRole {
        org: String,
        account: String,
        role: OrgRole,
        /// The role the page showed they hold now.
        from: OrgRole,
    },
    SetMemberTeams {
        org: String,
        account: String,
        teams: Vec<TeamRow>,
    },
    RemoveMemberTeams {
        org: String,
        account: String,
        teams: Vec<String>,
    },
    RevokeInvite {
        org: String,
        invite_key: String,
    },
    Rename {
        org: String,
        name: String,
        /// The name the page showed.
        from: String,
    },
    Delete {
        org: String,
        /// What the page said goes with it.
        holds: Holds,
    },
    SetCapabilities {
        org: String,
        account: Option<String>,
        team: Option<String>,
        capabilities: u32,
        /// What the page showed is held now (`None`: a team without a default).
        from: Option<u32>,
    },
    SetAllApps {
        org: String,
        account: String,
        all_apps: bool,
        /// Whether the page showed they have it now.
        from: bool,
    },
}

/// What deleting an organization takes with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Holds {
    members: usize,
    teams: usize,
    invites: usize,
    apps: usize,
}

// ---------------------------------------------------------------- shared

fn id_fits(field: &str, id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > MAX_SUBJECT_ID_BYTES {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            &format!("subject.{field} is 1 to {MAX_SUBJECT_ID_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn no_org() -> ApiError {
    ApiError::not_found("no such organization")
}

fn not_a_member() -> ApiError {
    ApiError::not_found("no such member")
}

/// `account`'s organization `org` and role there; `404` outside it.
fn org_for(s: &SystemState, org: &str, account: &str) -> Result<(Org, OrgRole), ApiError> {
    let o = s.orgs.get(org).ok_or_else(no_org)?;
    let role = o.role(account).ok_or_else(no_org)?;
    Ok((o.clone(), role))
}

/// `who` as `viewer` reads them in `o`: their self-set name there, else their
/// account's name, else their id, for a member `viewer` sees; "you" for the
/// viewer.
fn person(s: &SystemState, o: &Org, viewer: &str, who: &str) -> String {
    if viewer == who {
        return "you".to_owned();
    }
    let seen = o.sees_member(viewer, who) || o.role(who).is_none();
    let named = seen
        .then(|| {
            o.member_metadata
                .get(who)
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    s.accounts
                        .get(who)
                        .map(|a| a.name.clone())
                        .filter(|n| !n.trim().is_empty())
                })
        })
        .flatten();
    named.unwrap_or_else(|| format!("account {who}"))
}

/// The teams of `o` `account` is in, as ids.
fn teams_of(o: &Org, account: &str) -> Vec<String> {
    o.teams
        .keys()
        .filter(|t| o.in_team(account, t))
        .cloned()
        .collect()
}

/// The names of the apps of `org` that `account` reaches only as a member
/// (any but a public one): what leaving or being removed takes.
fn member_apps(s: &SystemState, org: &str, account: &str) -> Vec<String> {
    s.apps
        .values()
        .filter(|a| a.org.as_deref() == Some(org) && a.access != Access::Public)
        .filter(|a| s.may_reach(a, Some(account)))
        .map(|a| a.name.clone())
        .collect()
}

/// How many apps `count` is, in words.
fn apps_words(count: usize) -> String {
    if count == 1 {
        "1 app".to_owned()
    } else {
        format!("{count} apps")
    }
}

/// The capabilities `bits` hold, as a person reads them.
fn cap_words(bits: u32) -> Vec<String> {
    const NAMES: &[(u32, &str)] = &[
        (caps::CREATE_APP, "create apps"),
        (caps::INVITE_MEMBERS, "invite people"),
        (caps::JOIN_OPEN_TEAMS, "join open teams"),
        (caps::MANAGE_MEMBERS, "manage members"),
        (caps::MANAGE_APPLICATION, "upgrade and manage apps"),
        (caps::CREATE_TEAM, "create teams"),
        (caps::DELETE_TEAM, "delete teams"),
        (caps::MANAGE_VISIBILITY, "change who sees a team"),
        (caps::MANAGE_METADATA, "change settings"),
        (caps::APPROVE_RELEASES, "approve node releases"),
        (caps::CREATE_CHILD_TEAM, "create private channels"),
    ];
    let mut out: Vec<String> = NAMES
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let known = NAMES.iter().fold(0, |acc, (bit, _)| acc | bit);
    if bits & !known != 0 {
        out.push(format!("other capabilities ({:#x})", bits & !known));
    }
    out
}

/// What a set of capabilities can do, in a sentence's tail.
fn cap_phrase(bits: u32) -> String {
    let words = cap_words(bits);
    if words.is_empty() {
        "nothing beyond what a member does anyway".to_owned()
    } else {
        phrase(&words)
    }
}

/// What a team row's role means, as a person reads it.
const fn team_role_words(role: TeamRole) -> &'static str {
    match role {
        TeamRole::Lead => "lead",
        TeamRole::Member => "member",
        TeamRole::ReadOnly => "read-only member",
    }
}

/// The capabilities held now where `setCapabilities` would set them: the
/// member's row in `team`, the member's organization-wide ones, `team`'s
/// default (`None` when it has none), or the organization's default.
fn current_caps(
    o: &Org,
    account: Option<&str>,
    team: Option<&str>,
) -> Result<Option<u32>, ApiError> {
    match (account, team) {
        (Some(a), Some(t)) => {
            let rec = o
                .teams
                .get(t)
                .ok_or_else(|| ApiError::not_found("no such team"))?;
            if !rec.members.contains_key(a) {
                return Err(ApiError::not_found("not a member of this team"));
            }
            Ok(Some(o.team_caps(a, t)))
        }
        (Some(a), None) => Ok(Some(
            o.capabilities
                .get(a)
                .copied()
                .unwrap_or(o.default_capabilities),
        )),
        (None, Some(t)) => Ok(o
            .teams
            .get(t)
            .ok_or_else(|| ApiError::not_found("no such team"))?
            .default_capabilities),
        (None, None) => Ok(Some(o.default_capabilities)),
    }
}

fn holds(s: &SystemState, org: &str, o: &Org) -> Holds {
    Holds {
        members: o.members.len(),
        teams: o.teams.len(),
        invites: s.invites.values().filter(|i| i.org == org).count(),
        apps: s
            .apps
            .values()
            .filter(|a| a.org.as_deref() == Some(org))
            .count(),
    }
}

/// `n thing(s)`.
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn scope(org: &str, name: &str) -> Value {
    json!({ "kind": "org", "org": org, "name": name })
}

// ---------------------------------------------------------------- requests

/// Judge an organization request for `account` and freeze what the page shows.
pub(super) async fn request(
    node: &Node,
    account: &str,
    ask: OrgAsk,
) -> Result<(OrgAction, Value), ApiError> {
    // The routes' own gates, before a page exists: a node that could not
    // apply the entry refuses it anyway.
    let features: &[Feature] = match &ask {
        OrgAsk::Rename(_) => &[Feature::RenameOrg],
        OrgAsk::SetAllApps(_) => &[Feature::AllApps],
        OrgAsk::SetMemberTeams(_) | OrgAsk::RemoveMemberTeams(_) => &[Feature::TeamMembersBatch],
        _ => &[],
    };
    node.require_features(features).await?;
    node.sys(|s| match ask {
        OrgAsk::Leave(sub) => leave(s, account, sub),
        OrgAsk::RemoveMember(sub) => remove(s, account, sub),
        OrgAsk::SetMemberRole(sub) => set_role(s, account, sub),
        OrgAsk::SetMemberTeams(sub) => set_teams(s, account, sub),
        OrgAsk::RemoveMemberTeams(sub) => remove_teams(s, account, sub),
        OrgAsk::RevokeInvite(sub) => revoke(s, account, sub),
        OrgAsk::Rename(sub) => rename(s, account, sub),
        OrgAsk::Delete(sub) => delete(s, account, sub),
        OrgAsk::SetCapabilities(sub) => capabilities(s, account, sub),
        OrgAsk::SetAllApps(sub) => all_apps(s, account, sub),
    })
}

fn leave(s: &SystemState, account: &str, sub: OrgSubject) -> Result<(OrgAction, Value), ApiError> {
    let OrgSubject { org } = sub;
    id_fits("org", &org)?;
    let (o, role) = org_for(s, &org, account)?;
    // `RemoveOrgMember`: the owner is never removed, by anyone; there is one.
    if role == OrgRole::Owner {
        return Err(ApiError::conflict(
            "you own this organization: make someone else its owner before you leave",
        ));
    }
    let name = o.name.clone();
    let teams = teams_of(&o, account);
    let team_names = team_names_in(&o, &teams);
    let apps = member_apps(s, &org, account);
    let owned = s
        .apps
        .values()
        .filter(|a| a.org.as_deref() == Some(org.as_str()) && a.owner == account)
        .count();
    let mut points = vec![format!(
        "You leave {name}: you are no longer {} of it.",
        role_words(role)
    )];
    if !team_names.is_empty() {
        points.push(format!("You leave its teams: {}.", phrase(&team_names)));
    }
    points.push(if apps.is_empty() {
        format!("You reach no app of {name} that only its members reach.")
    } else {
        format!(
            "You lose {} only its members reach: {}.",
            apps_words(apps.len()),
            phrase(&apps)
        )
    });
    if owned > 0 {
        points.push(format!(
            "The {} you created there stay in {name}; you stop managing them, and its admins \
             can hand them to someone else.",
            apps_words(owned)
        ));
    }
    points.push(format!(
        "To come back, someone in {name} has to invite you again."
    ));
    let attn = if apps.is_empty() {
        vec![]
    } else {
        vec![attention(
            "losesApps",
            format!("You lose access to {}.", apps_words(apps.len())),
            apps.clone(),
        )]
    };
    let model = json!({
        "kind": "leaveOrg",
        "scope": scope(&org, &name),
        "headline": format!("Leave {name}"),
        "title": format!("Leave {name}"),
        "points": points,
        "attention": attn,
        "before": format!("Until you approve, you stay {} of {name}.", role_words(role)),
        "after": format!("Once you approve, you are no longer in {name}."),
        "notes": [],
        "member": {
            "org": org,
            "account": account,
            "role": role,
            "teams": teams,
            "teamNames": team_names,
            "apps": apps,
        },
    });
    Ok((OrgAction::Leave { org }, model))
}

/// The names of `teams` of `o`, for someone in them.
fn team_names_in(o: &Org, teams: &[String]) -> Vec<String> {
    teams
        .iter()
        .map(|t| o.teams.get(t).map_or_else(|| t.clone(), |t| t.name.clone()))
        .collect()
}

fn remove(
    s: &SystemState,
    actor: &str,
    sub: MemberSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let MemberSubject { org, account } = sub;
    id_fits("org", &org)?;
    id_fits("account", &account)?;
    let (o, role) = org_for(s, &org, actor)?;
    if account == actor {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "to leave the organization, ask for leaveOrg",
        ));
    }
    if role < OrgRole::Admin {
        return Err(ApiError::forbidden("only admins remove members"));
    }
    let target = o.role(&account).ok_or_else(not_a_member)?;
    if target == OrgRole::Owner {
        return Err(ApiError::forbidden(
            "the owner is never removed: ownership moves first",
        ));
    }
    let name = o.name.clone();
    let who = person(s, &o, actor, &account);
    let teams = teams_of(&o, &account);
    let team_names = seen_team_names(&o, actor, &teams);
    let apps = member_apps(s, &org, &account);
    let mut points = vec![format!(
        "{who} is removed from {name}, where they are {}.",
        role_words(target)
    )];
    if !team_names.is_empty() {
        points.push(format!("They leave its teams: {}.", phrase(&team_names)));
    }
    if !apps.is_empty() {
        points.push(format!(
            "They lose {} only its members reach.",
            apps_words(apps.len())
        ));
    }
    points.push("They can come back only with a new invite.".to_owned());
    let model = json!({
        "kind": "removeMember",
        "scope": scope(&org, &name),
        "headline": format!("Remove {who} from {name}"),
        "title": format!("Remove {who} from {name}"),
        "points": points,
        "attention": [],
        "before": format!("Until you approve, {who} stays {} of {name}.", role_words(target)),
        "after": format!("Once you approve, {who} is no longer in {name}."),
        "notes": [],
        "member": {
            "org": org,
            "account": account,
            "name": who,
            "role": target,
            "teams": teams,
            "teamNames": team_names,
        },
    });
    Ok((OrgAction::RemoveMember { org, account }, model))
}

/// The names of `teams` of `o` as `viewer` sees them (an id where they do not).
fn seen_team_names(o: &Org, viewer: &str, teams: &[String]) -> Vec<String> {
    teams
        .iter()
        .map(|t| {
            o.teams
                .get(t)
                .filter(|_| o.sees_team(viewer, t))
                .map_or_else(|| t.clone(), |team| team.name.clone())
        })
        .collect()
}

fn set_role(
    s: &SystemState,
    actor: &str,
    sub: SetMemberRoleSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let SetMemberRoleSubject { org, account, role } = sub;
    id_fits("org", &org)?;
    id_fits("account", &account)?;
    let (o, actor_role) = org_for(s, &org, actor)?;
    // `SetOrgRole`'s rules.
    if actor_role < OrgRole::Admin {
        return Err(ApiError::forbidden("only admins manage members"));
    }
    // People join with an invite (`createInvite`); this changes a member's.
    let from = o.role(&account).ok_or_else(not_a_member)?;
    if (role == OrgRole::Owner || from == OrgRole::Owner) && actor_role != OrgRole::Owner {
        return Err(ApiError::forbidden("only the owner changes ownership"));
    }
    if from == role {
        return Err(ApiError::conflict(&format!(
            "they are {} already",
            role_words(role)
        )));
    }
    let name = o.name.clone();
    let who = person(s, &o, actor, &account);
    let mut points = vec![format!(
        "{who} becomes {} of {name}; they are {} now.",
        role_words(role),
        role_words(from)
    )];
    let mut attn = vec![];
    match role {
        OrgRole::Owner => {
            points.push(format!(
                "You hand {name} to {who}: you become an admin, and only they can delete it or \
                 change who owns it."
            ));
            attn.push(attention(
                "transfersOwnership",
                format!("You stop being the owner of {name}."),
                vec![],
            ));
        }
        OrgRole::Admin => points.push(format!(
            "As an admin they manage every member, team, invite and app of {name}, and can \
             rename it."
        )),
        OrgRole::Member => points.push(format!(
            "As a member they reach {name}'s apps their teams reach and manage nothing they were \
             not given."
        )),
        OrgRole::Guest => {
            points.push("As a guest they reach only the apps of the teams they are in.".to_owned());
        }
    }
    let model = json!({
        "kind": "setMemberRole",
        "scope": scope(&org, &name),
        "headline": format!("Make {who} {} of {name}", role_words(role)),
        "title": format!("Make {who} {} of {name}", role_words(role)),
        "points": points,
        "attention": attn,
        "before": format!("Until you approve, {who} stays {}.", role_words(from)),
        "after": format!("Once you approve, {who} is {} of {name}.", role_words(role)),
        "notes": [],
        "member": { "org": org, "account": account, "name": who, "from": from, "role": role },
    });
    Ok((
        OrgAction::SetMemberRole {
            org,
            account,
            role,
            from,
        },
        model,
    ))
}

fn set_teams(
    s: &SystemState,
    actor: &str,
    sub: SetMemberTeamsSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let SetMemberTeamsSubject {
        org,
        account,
        teams,
    } = sub;
    id_fits("org", &org)?;
    id_fits("account", &account)?;
    for r in &teams {
        id_fits("teams[].team", &r.team)?;
    }
    let (o, _role) = org_for(s, &org, actor)?;
    o.role(&account).ok_or_else(not_a_member)?;
    crate::system::judge_team_rows(&o, &account, &teams, actor)
        .map_err(|e| refusal(e, "BAD_TEAMS"))?;
    let name = o.name.clone();
    let who = person(s, &o, actor, &account);
    let ids: Vec<String> = teams.iter().map(|r| r.team.clone()).collect();
    let names = seen_team_names(&o, actor, &ids);
    let mut points: Vec<String> = teams
        .iter()
        .zip(&names)
        .map(|(r, team)| {
            let role = team_role_words(r.role.unwrap_or(TeamRole::Member));
            match r.capabilities {
                Some(c) => format!("{team}: {role}, who can {}.", cap_phrase(c)),
                None => format!("{team}: {role}."),
            }
        })
        .collect();
    points.insert(
        0,
        format!("{who} is put in these teams of {name}, or their place there changes:"),
    );
    let model = json!({
        "kind": "setMemberTeams",
        "scope": scope(&org, &name),
        "headline": format!("Set {who}'s teams in {name}"),
        "title": format!("Put {who} in {}", phrase(&names)),
        "points": points,
        "attention": [],
        "before": format!("Until you approve, {who}'s teams stay as they are."),
        "after": format!("Once you approve, {who} is in {} as listed.", phrase(&names)),
        "notes": [],
        "member": { "org": org, "account": account, "name": who, "teams": teams, "teamNames": names },
    });
    Ok((
        OrgAction::SetMemberTeams {
            org,
            account,
            teams,
        },
        model,
    ))
}

/// A refusal the log would answer, before it is asked: a batch's shape is
/// `400 code`, anything else `403`.
fn refusal(e: &'static str, code: &str) -> ApiError {
    match e {
        "name at least one team" | "too many teams in one step" | "a team is named twice" => {
            ApiError::bad_request(code, e)
        }
        "no such team" => ApiError::not_found(e),
        _ => ApiError::forbidden(e),
    }
}

fn remove_teams(
    s: &SystemState,
    actor: &str,
    sub: RemoveMemberTeamsSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let RemoveMemberTeamsSubject {
        org,
        account,
        teams,
    } = sub;
    id_fits("org", &org)?;
    id_fits("account", &account)?;
    for t in &teams {
        id_fits("teams[]", t)?;
    }
    let (o, _role) = org_for(s, &org, actor)?;
    o.role(&account).ok_or_else(not_a_member)?;
    crate::system::judge_remove_team_rows(&o, &account, &teams, actor)
        .map_err(|e| refusal(e, "BAD_TEAMS"))?;
    let name = o.name.clone();
    let who = person(s, &o, actor, &account);
    let names = seen_team_names(&o, actor, &teams);
    let below: Vec<String> = {
        let mut out = Vec::new();
        for t in &teams {
            for b in o.subtree(t).into_iter().skip(1) {
                let has_row = o
                    .teams
                    .get(&b)
                    .is_some_and(|x| x.members.contains_key(&account));
                if has_row && !teams.contains(&b) && !out.contains(&b) {
                    out.push(b);
                }
            }
        }
        out
    };
    let below_names = seen_team_names(&o, actor, &below);
    let leaving = actor == account;
    let (they, them) = if leaving {
        ("you", "you")
    } else {
        ("they", "them")
    };
    let mut points = vec![if leaving {
        format!("You leave {} in {name}.", phrase(&names))
    } else {
        format!("{who} is taken out of {} in {name}.", phrase(&names))
    }];
    if !below_names.is_empty() {
        points.push(format!(
            "Also of the teams below them {they} are in: {}.",
            phrase(&below_names)
        ));
    }
    points.push(format!(
        "To an open one, {they} come back only when a lead adds {them} or an invite names it."
    ));
    let title = if leaving {
        format!("Leave {}", phrase(&names))
    } else {
        format!("Take {who} out of {}", phrase(&names))
    };
    let model = json!({
        "kind": "removeMemberTeams",
        "scope": scope(&org, &name),
        "headline": title,
        "title": title,
        "points": points,
        "attention": [],
        "before": format!("Until you approve, {who}'s teams stay as they are."),
        "after": format!("Once you approve, {who} is no longer in {}.", phrase(&names)),
        "notes": [],
        "member": {
            "org": org,
            "account": account,
            "name": who,
            "teams": teams,
            "teamNames": names,
            "below": below,
            "belowNames": below_names,
        },
    });
    Ok((
        OrgAction::RemoveMemberTeams {
            org,
            account,
            teams,
        },
        model,
    ))
}

fn revoke(
    s: &SystemState,
    actor: &str,
    sub: RevokeInviteSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let RevokeInviteSubject { org, invite_key } = sub;
    id_fits("org", &org)?;
    id_fits("inviteKey", &invite_key)?;
    let (o, _role) = org_for(s, &org, actor)?;
    // `RevokeInvite`: whoever sees it (`SystemState::sees_invite`).
    let inv = s
        .invites
        .get(&invite_key)
        .filter(|i| i.org == org && s.sees_invite(i, actor))
        .ok_or_else(|| ApiError::not_found(crate::system::INVITE_GONE))?
        .clone();
    let name = o.name.clone();
    let teams = seen_team_names(&o, actor, &inv.teams);
    let role_text = role_words(inv.role);
    let joins = if teams.is_empty() {
        format!("{name} as {role_text}")
    } else {
        format!("{name} as {role_text}, in {}", phrase(&teams))
    };
    let by = (!inv.created_by.is_empty()).then(|| person(s, &o, actor, &inv.created_by));
    let mut points = vec![format!(
        "The invite to join {joins} stops working, for everyone who has its link."
    )];
    points.push(format!(
        "{} could still join with it, until {}.",
        count(
            usize::try_from(inv.uses_left).unwrap_or(usize::MAX),
            "more person",
            "more people"
        ),
        plain_date(inv.expires_at)
    ));
    if let Some(by) = &by {
        points.push(format!("It was made by {by}."));
    }
    points.push("Whoever joined with it already stays.".to_owned());
    let model = json!({
        "kind": "revokeInvite",
        "scope": scope(&org, &name),
        "headline": format!("Revoke an invite to {joins}"),
        "title": format!("Revoke an invite to {name}"),
        "points": points,
        "attention": [],
        "before": format!("Until you approve, anyone with the link can join {joins}."),
        "after": "Once you approve, nobody can join with it.",
        "notes": [],
        "invite": {
            "org": org,
            "inviteKey": invite_key,
            "role": inv.role,
            "teams": inv.teams,
            "teamNames": teams,
            "usesLeft": inv.uses_left,
            "expiresAt": inv.expires_at,
            "expired": inv.expires_at <= now_secs(),
            "createdBy": by,
        },
    });
    Ok((OrgAction::RevokeInvite { org, invite_key }, model))
}

fn rename(
    s: &SystemState,
    actor: &str,
    sub: RenameOrgSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let RenameOrgSubject { org, name } = sub;
    id_fits("org", &org)?;
    let (o, role) = org_for(s, &org, actor)?;
    if role < OrgRole::Admin {
        return Err(ApiError::forbidden(crate::system::ONLY_ADMINS_RENAME));
    }
    crate::system::org_name_fits(&name).map_err(|why| ApiError::bad_request("BAD_REQUEST", why))?;
    let from = o.name.clone();
    let model = json!({
        "kind": "renameOrg",
        "scope": scope(&org, &from),
        "headline": format!("Rename {from} to {name}"),
        "title": format!("Rename {from} to {name}"),
        "points": [
            format!("{from} is called {name} from now on, for everyone in it."),
            "Its members, teams, invites and apps stay as they are.",
        ],
        "attention": [],
        "before": format!("Until you approve, it is called {from}."),
        "after": format!("Once you approve, it is called {name}."),
        "notes": [],
        "org": { "org": org, "from": from, "name": name },
    });
    Ok((OrgAction::Rename { org, name, from }, model))
}

fn delete(s: &SystemState, actor: &str, sub: OrgSubject) -> Result<(OrgAction, Value), ApiError> {
    let OrgSubject { org } = sub;
    id_fits("org", &org)?;
    let (o, role) = org_for(s, &org, actor)?;
    // `DeleteOrg`'s rules.
    if role != OrgRole::Owner {
        return Err(ApiError::forbidden(
            "only the organization's owner deletes it",
        ));
    }
    let owner = crate::system::Deployer::Org(org.clone());
    if s.deployments.values().any(|d| d.owner == owner) {
        return Err(ApiError::conflict(
            "the organization owns a deployment; delete it first",
        ));
    }
    let name = o.name.clone();
    let h = holds(s, &org, &o);
    let apps: Vec<String> = s
        .apps
        .values()
        .filter(|a| a.org.as_deref() == Some(org.as_str()))
        .map(|a| a.name.clone())
        .collect();
    let gone = format!(
        "its {}, {} and {}",
        count(h.teams, "team", "teams"),
        count(h.invites, "invite", "invites"),
        count(h.apps, "app", "apps")
    );
    let others = h.members.saturating_sub(1);
    let points = vec![
        format!("{name} is deleted, with {gone}."),
        format!(
            "The apps' data is deleted with them and cannot be brought back{}.",
            if apps.is_empty() {
                String::new()
            } else {
                format!(": {}", phrase(&apps))
            }
        ),
        format!(
            "{} lose it, and nobody can join with its invites.",
            if others == 0 {
                "You".to_owned()
            } else {
                format!("You and {}", count(others, "other member", "other members"))
            }
        ),
        "This cannot be undone.".to_owned(),
    ];
    let model = json!({
        "kind": "deleteOrg",
        "scope": scope(&org, &name),
        "headline": format!("Delete {name}"),
        "title": format!("Delete {name} and everything in it"),
        "points": points,
        "attention": [attention(
            "deletes",
            format!("Deleting {name} deletes {gone}, for good."),
            apps.clone(),
        )],
        "before": format!("Until you approve, {name} and everything in it stay."),
        "after": format!("Once you approve, {name}, {gone} are gone."),
        "notes": [],
        "delete": {
            "org": org,
            "members": h.members,
            "teams": h.teams,
            "invites": h.invites,
            "apps": h.apps,
            "appNames": apps,
        },
    });
    Ok((OrgAction::Delete { org, holds: h }, model))
}

fn capabilities(
    s: &SystemState,
    actor: &str,
    sub: SetCapabilitiesSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let SetCapabilitiesSubject {
        org,
        account,
        team,
        capabilities,
    } = sub;
    id_fits("org", &org)?;
    if let Some(a) = &account {
        id_fits("account", a)?;
    }
    if let Some(t) = &team {
        id_fits("team", t)?;
    }
    let (o, role) = org_for(s, &org, actor)?;
    if let Some(a) = &account {
        o.role(a).ok_or_else(not_a_member)?;
    }
    if let Some(t) = &team {
        if !o.teams.contains_key(t) || !o.sees_team(actor, t) {
            return Err(ApiError::not_found("no such team"));
        }
    }
    // `SetCapabilities` and `SetDefaultCapabilities`: a team's are its
    // managers' or `MANAGE_MEMBERS` there; the organization's an admin's.
    let allowed = match &team {
        Some(t) => o.manages_team(actor, t) || o.may(actor, Some(t), caps::MANAGE_MEMBERS),
        None => role >= OrgRole::Admin,
    };
    if !allowed {
        return Err(ApiError::forbidden(match team {
            Some(_) => "not allowed to manage this team",
            None => "only admins manage members",
        }));
    }
    let from = current_caps(&o, account.as_deref(), team.as_deref())?;
    let name = o.name.clone();
    let team_name = team
        .as_ref()
        .map(|t| seen_team_names(&o, actor, std::slice::from_ref(t)).remove(0));
    let whose = match (&account, &team_name) {
        (Some(a), Some(t)) => format!("{} in {t}", person(s, &o, actor, a)),
        (Some(a), None) => format!("{} across {name}", person(s, &o, actor, a)),
        (None, Some(t)) => format!("everyone in {t}"),
        (None, None) => format!("every member of {name} without their own"),
    };
    let held = from.unwrap_or(0);
    let gains = capabilities & !held;
    let loses = held & !capabilities;
    let mut points = vec![format!("{whose} can {}.", cap_phrase(capabilities))];
    if gains != 0 {
        points.push(format!("New: {}.", phrase(&cap_words(gains))));
    }
    if loses != 0 {
        points.push(format!("No longer: {}.", phrase(&cap_words(loses))));
    }
    if gains == 0 && loses == 0 {
        points.push("Nothing changes from what is held now.".to_owned());
    }
    if account.is_none() {
        points.push(
            "A default counts for whoever has no capabilities of their own set there.".to_owned(),
        );
    }
    let strong = gains & (caps::MANAGE_MEMBERS | caps::MANAGE_APPLICATION | caps::APPROVE_RELEASES);
    let attn = if strong == 0 {
        vec![]
    } else {
        vec![attention(
            "grantsManagement",
            format!("{whose} can then manage what others use."),
            cap_words(strong),
        )]
    };
    let title = match &account {
        Some(_) => format!("Change what {whose} can do"),
        None => format!("Change what {whose} can do by default"),
    };
    let model = json!({
        "kind": "setCapabilities",
        "scope": scope(&org, &name),
        "headline": title,
        "title": title,
        "points": points,
        "attention": attn,
        "before": format!("Until you approve, {whose} can {}.", cap_phrase(held)),
        "after": format!("Once you approve, {whose} can {}.", cap_phrase(capabilities)),
        "notes": [],
        "capabilities": {
            "org": org,
            "account": account,
            "team": team,
            "teamName": team_name,
            "from": from,
            "capabilities": capabilities,
            "names": cap_words(capabilities),
        },
    });
    Ok((
        OrgAction::SetCapabilities {
            org,
            account,
            team,
            capabilities,
            from,
        },
        model,
    ))
}

fn all_apps(
    s: &SystemState,
    actor: &str,
    sub: SetAllAppsSubject,
) -> Result<(OrgAction, Value), ApiError> {
    let SetAllAppsSubject {
        org,
        account,
        all_apps,
    } = sub;
    id_fits("org", &org)?;
    id_fits("account", &account)?;
    let (o, role) = org_for(s, &org, actor)?;
    if role < OrgRole::Admin {
        return Err(ApiError::forbidden(crate::system::ONLY_ADMINS_ALL_APPS));
    }
    o.role(&account).ok_or_else(not_a_member)?;
    let from = o.all_apps.contains(&account);
    if from == all_apps {
        return Err(ApiError::conflict(if all_apps {
            "they get every app added later already"
        } else {
            "they do not get every app added later"
        }));
    }
    let name = o.name.clone();
    let who = person(s, &o, actor, &account);
    let (title, points, after) = if all_apps {
        (
            format!("Give {who} every app added to {name}"),
            vec![
                format!("{who} gets every app added to {name} from now on, as it is added."),
                "Apps already there stay as they are for them.".to_owned(),
            ],
            format!("Once you approve, {who} gets each new app of {name}."),
        )
    } else {
        (
            format!("Stop giving {who} every app added to {name}"),
            vec![
                format!("{who} no longer gets each app added to {name} from now on."),
                "Apps they have already stay theirs.".to_owned(),
            ],
            format!("Once you approve, new apps of {name} reach {who} only as anyone else."),
        )
    };
    let model = json!({
        "kind": "setAllApps",
        "scope": scope(&org, &name),
        "headline": title,
        "title": title,
        "points": points,
        "attention": [],
        "before": format!(
            "Until you approve, {who} {} every app added later.",
            if from { "gets" } else { "does not get" }
        ),
        "after": after,
        "notes": [],
        "member": { "org": org, "account": account, "name": who, "allApps": all_apps },
    });
    Ok((
        OrgAction::SetAllApps {
            org,
            account,
            all_apps,
            from,
        },
        model,
    ))
}

// ---------------------------------------------------------------- approving

/// The facts the page showed that could change before the approval: a
/// role, a name, what a deletion takes, capabilities, the all-apps flag. The
/// routes judge authority again when they run.
pub(super) fn still_current(node: &Node, action: &OrgAction) -> Result<(), ApiError> {
    node.sys(|s| match action {
        OrgAction::Leave { .. }
        | OrgAction::RemoveMember { .. }
        | OrgAction::SetMemberTeams { .. }
        | OrgAction::RemoveMemberTeams { .. }
        | OrgAction::RevokeInvite { .. } => Ok(()),
        OrgAction::SetMemberRole {
            org, account, from, ..
        } => {
            let now = s.orgs.get(org).and_then(|o| o.role(account));
            if now == Some(*from) {
                Ok(())
            } else {
                Err(stale("their role"))
            }
        }
        OrgAction::Rename { org, from, .. } => {
            if s.orgs.get(org).is_some_and(|o| &o.name == from) {
                Ok(())
            } else {
                Err(stale("the organization's name"))
            }
        }
        OrgAction::Delete { org, holds: h } => {
            if s.orgs.get(org).is_some_and(|o| holds(s, org, o) == *h) {
                Ok(())
            } else {
                Err(stale("what the organization holds"))
            }
        }
        OrgAction::SetCapabilities {
            org,
            account,
            team,
            from,
            ..
        } => {
            let now = s
                .orgs
                .get(org)
                .and_then(|o| current_caps(o, account.as_deref(), team.as_deref()).ok());
            if now == Some(*from) {
                Ok(())
            } else {
                Err(stale("the capabilities held now"))
            }
        }
        OrgAction::SetAllApps {
            org, account, from, ..
        } => {
            let now = s.orgs.get(org).map(|o| o.all_apps.contains(account));
            if now == Some(*from) {
                Ok(())
            } else {
                Err(stale("whether they get every app added later"))
            }
        }
    })
}

/// Run the request's route as `p` (the page's principal for the request's
/// account). What the route answers goes to the asking app as `outcome`,
/// unless it answers nothing (`{}`).
pub(super) async fn act(
    node: &Node,
    p: &Principal,
    action: &OrgAction,
    confirm: Option<ConfirmBody>,
) -> Result<Acted, ApiError> {
    let axum::Json(v) = match action.clone() {
        OrgAction::Leave { org } => remove_member(node, p, org, p.account()?, confirm).await?,
        OrgAction::RemoveMember { org, account } => {
            remove_member(node, p, org, account, confirm).await?
        }
        OrgAction::SetMemberRole {
            org, account, role, ..
        } => set_member_role(node, p, org, account, role, confirm).await?,
        OrgAction::SetMemberTeams {
            org,
            account,
            teams,
        } => set_member_teams(node, p, org, account, teams, confirm).await?,
        OrgAction::RemoveMemberTeams {
            org,
            account,
            teams,
        } => remove_member_teams(node, p, org, account, teams, confirm).await?,
        OrgAction::RevokeInvite { org, invite_key } => {
            revoke_invite(node, p, org, invite_key, confirm).await?
        }
        OrgAction::Rename { org, name, .. } => rename_org(node, p, org, name, confirm).await?,
        OrgAction::Delete { org, .. } => delete_org(node, p, org, confirm).await?,
        OrgAction::SetCapabilities {
            org,
            account,
            team,
            capabilities,
            ..
        } => {
            set_capabilities(
                node,
                p,
                org,
                CapabilitiesBody {
                    account,
                    team,
                    capabilities,
                    confirm,
                },
            )
            .await?
        }
        OrgAction::SetAllApps {
            org,
            account,
            all_apps,
            ..
        } => set_all_apps(node, p, org, account, all_apps, confirm).await?,
    };
    let outcome = v
        .as_object()
        .is_some_and(|o| !o.is_empty())
        .then(|| v.clone());
    Ok(Acted { shown: v, outcome })
}
