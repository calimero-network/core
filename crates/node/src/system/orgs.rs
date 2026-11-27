//! System commands for organizations, members, teams and invites.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`]. Each arm of either
//! is one call to a function of its own, in the order of the arms, and each
//! refusal of this area's own rules is a named [`Rejection`] below.

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// What `CreateOrg` answers: the new organization, and its app team when it is made with one
/// (the consent page's `createOrg` for a package).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgCreated {
    pub org_id: OrgId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<TeamId>,
}

/// What redeeming an invite answers: the organization joined; for an instance invite, the
/// instance and whether the account is a member of its organization (else a guest).
#[derive(Debug, Serialize, Deserialize)]
pub struct InviteRedeemed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppId>,
    pub org: OrgId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<bool>,
}

/// What `DeleteOrg` answers: the instances deleted with it.
#[derive(Debug, Serialize, Deserialize)]
pub struct OrgDeleted {
    pub apps: Vec<AppId>,
}

/// What `RenameOrg` answers.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgRenamed {
    pub org_id: OrgId,
    pub name: String,
}

/// What `CreateTeam` answers.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamCreated {
    pub team_id: TeamId,
}

/// What `DeleteTeam` answers: the teams deleted (the team and those below it), the instances
/// deleted with them, and with `keepApps` those kept instead (`apps` then empty).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDeleted {
    pub teams: Vec<TeamId>,
    pub apps: Vec<AppId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_apps: Option<Vec<AppId>>,
}

/// What `SetOrgSettings` answers: the settings after the change.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgSettingsSet {
    pub org: OrgId,
    pub allow_guests: bool,
    pub allow_public_links: bool,
}

// This area's refusals. Each is `400 REJECTED`, the kind the bare message it
// names was answered with; the message is what a client reads.

/// The actor or the account named is not a member of the organization.
const NOT_A_MEMBER: Rejection = Rejection::new(RejectionKind::Rejected, "not a member");
/// An organization's delete by someone other than its owner.
const ONLY_OWNER_DELETES_ORG: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the organization's owner deletes it",
);
/// An organization's delete while it owns a deployment.
const OWNS_A_DEPLOYMENT: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the organization owns a deployment; delete it first",
);
/// An organization's metadata set by someone neither an admin nor holding
/// `MANAGE_METADATA`.
const MAY_NOT_SET_ORG_METADATA: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "not allowed to change this organization's metadata",
);
/// A member's metadata set by someone neither that member nor an admin.
const MEMBER_METADATA_THEIRS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a member's metadata is theirs or an admin's to set",
);
/// A member's role, or the organization's capabilities, set by someone below admin.
const ONLY_ADMINS_MANAGE_MEMBERS: Rejection =
    Rejection::new(RejectionKind::Rejected, "only admins manage members");
/// An ownership change by someone other than the owner.
const ONLY_OWNER_CHANGES_OWNERSHIP: Rejection =
    Rejection::new(RejectionKind::Rejected, "only the owner changes ownership");
/// The owner removed while still the owner.
const TRANSFER_OWNERSHIP_FIRST: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "transfer ownership before removing the owner",
);
/// Another member removed by someone below admin.
const ONLY_ADMINS_REMOVE_MEMBERS: Rejection =
    Rejection::new(RejectionKind::Rejected, "only admins remove members");
/// A team of that id exists already.
const TEAM_EXISTS: Rejection = Rejection::new(RejectionKind::Rejected, "team exists");
/// A team created by someone who may not create it there.
const MAY_NOT_CREATE_TEAM: Rejection =
    Rejection::new(RejectionKind::Rejected, "not allowed to create this team");
/// A team renamed by someone who does not manage it.
const MAY_NOT_RENAME_TEAM: Rejection =
    Rejection::new(RejectionKind::Rejected, "not allowed to rename this team");
/// A team's visibility changed by someone without `MANAGE_VISIBILITY` there.
const MAY_NOT_SET_TEAM_VISIBILITY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "not allowed to change this team's visibility",
);
/// A team's metadata changed by someone without `MANAGE_METADATA` there.
const MAY_NOT_SET_TEAM_METADATA: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "not allowed to change this team's metadata",
);
/// A team moved by someone who does not manage it and where it goes.
const MAY_NOT_MOVE_TEAM: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "not allowed to move this team there",
);
/// A team moved under itself or a team below it.
const TEAM_UNDER_ITSELF: Rejection =
    Rejection::new(RejectionKind::Rejected, "a team cannot move under itself");
/// A team deleted by someone who neither manages it nor holds `DELETE_TEAM`.
const MAY_NOT_DELETE_TEAM: Rejection =
    Rejection::new(RejectionKind::Rejected, "not allowed to delete this team");
/// A team row's capabilities set for an account without a row in the team.
const NOT_IN_THIS_TEAM: Rejection =
    Rejection::new(RejectionKind::Rejected, "not a member of this team");
/// An invite below admin naming a team its inviter does not see.
const INVITE_TEAMS_UNSEEN: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an invite names only teams the inviter sees",
);
/// An invite below admin without `INVITE_MEMBERS` where it invites.
const NEEDS_INVITE_MEMBERS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "inviting here needs the INVITE_MEMBERS capability",
);
/// An invite to a role above the inviter's, or to ownership.
const INVITE_ABOVE_ROLE: Rejection =
    Rejection::new(RejectionKind::Rejected, "cannot invite above your own role");
/// An organization's settings changed by someone neither its owner nor an admin.
const ONLY_ADMINS_CHANGE_SETTINGS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the organization's owner or an admin changes its settings",
);
/// A settings change that changes no setting.
const NO_SETTING_TO_CHANGE: Rejection =
    Rejection::new(RejectionKind::Rejected, "no setting to change");
/// An organization invite revoked by someone who may not see it.
const ONLY_CREATOR_OR_MANAGERS_REVOKE_ORG_INVITE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the invite's creator, an admin, or whoever manages every team it names may revoke it",
);

/// Judge the proof a command of this area carries ([`super::authorize`]).
///
/// Memberships carry the actor's passkey consent over exactly this change
/// (a member leaving consents to that too); who may make it is the command's
/// own rule, judged at apply ([`SystemMachine::run_authorized`]).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    match cmd {
        SystemCmd::DeleteOrg {
            org,
            actor,
            consent,
        } => org_consent_where(proofs, actor, consent, org, "__delete_org", &|args| {
            args["org"] == json!(org)
        }),
        SystemCmd::RenameOrg {
            org,
            name,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__rename_org",
            &json!({ "name": name }),
        ),
        SystemCmd::SetOrgRole {
            org,
            account,
            role,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__set_org_role",
            &json!({ "account": account, "role": role }),
        ),
        SystemCmd::RemoveOrgMember {
            org,
            account,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__remove_member",
            &json!({ "account": account }),
        ),
        SystemCmd::SetTeamMember {
            org,
            team,
            account,
            lead,
            role,
            capabilities,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__set_team_member",
            &json!({
                "team": team, "account": account, "lead": lead,
                "role": role, "capabilities": capabilities,
            }),
        ),
        SystemCmd::SetTeamMembers {
            org,
            account,
            teams,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__set_team_members",
            &team_members_args(account, teams),
        ),
        SystemCmd::RemoveTeamMembers {
            org,
            account,
            teams,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__remove_team_members",
            &remove_team_members_args(account, teams),
        ),
        SystemCmd::DeleteTeam {
            org,
            team,
            actor,
            consent,
            apps_need_consent: true,
            keep_apps,
        } => authorize_team_delete(proofs, org, team, actor, consent, *keep_apps),
        SystemCmd::RemoveTeamMember {
            org,
            team,
            account,
            actor,
            consent,
            ..
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__remove_team_member",
            &json!({ "team": team, "account": account }),
        ),
        SystemCmd::SetCapabilities {
            org,
            team,
            account,
            capabilities,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__set_capabilities",
            &json!({ "team": team, "account": account, "capabilities": capabilities }),
        ),
        SystemCmd::SetAllApps {
            org,
            account,
            all_apps,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__set_all_apps",
            &all_apps_args(account, *all_apps),
        ),
        SystemCmd::SetDefaultCapabilities {
            org,
            team,
            capabilities,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__set_default_capabilities",
            &default_capabilities_args(team.as_deref(), *capabilities),
        ),
        SystemCmd::CreateInvite {
            invite_key,
            invite,
            actor,
            consent,
            ..
        } => org_consent(
            proofs,
            actor,
            consent,
            &invite.org,
            "__create_invite",
            &invite_args(invite_key, invite),
        ),
        SystemCmd::SetOrgSettings {
            org,
            settings,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__set_org_settings",
            &serde_json::to_value(settings).unwrap_or_default(),
        ),
        SystemCmd::RevokeInvite {
            org,
            invite_key,
            actor,
            consent,
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__revoke_invite",
            &json!({ "inviteKey": invite_key }),
        ),
        SystemCmd::ScopeApps {
            org,
            apps,
            shared,
            actor,
            consent,
            ..
        } => org_consent(
            proofs,
            actor,
            consent,
            org,
            "__scope_apps",
            &scope_apps_args(apps.as_deref(), *shared),
        ),
        SystemCmd::RedeemInvite {
            invite_key,
            account,
            now,
            signature,
            ..
        } => authorize_invite_redemption(proofs, invite_key, account, *now, signature),
        _ => Ok(None),
    }
}

/// `actor`'s passkey consent, not used before, stating `method` on the
/// organization (`org:<org>`) with exactly the arguments `stated`.
fn org_consent(
    proofs: &Proofs<'_>,
    actor: &str,
    consent: &Value,
    org: &str,
    method: &str,
    stated: &Value,
) -> Result<Option<String>, Rejection> {
    org_consent_where(proofs, actor, consent, org, method, &|args| args == stated)
}

/// `actor`'s passkey consent, not used before, stating `method` on the
/// organization (`org:<org>`) with arguments `accepts` takes.
fn org_consent_where(
    proofs: &Proofs<'_>,
    actor: &str,
    consent: &Value,
    org: &str,
    method: &str,
    accepts: &dyn Fn(&Value) -> bool,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
    proofs.states(&statement, &format!("org:{org}"), method, accepts)?;
    Ok(Some(used))
}

/// A `DeleteTeam` judged by the rule that its apps need consent: with apps
/// below the team, the actor's passkey over exactly those apps, so one added
/// since the approval is not taken silently.
fn authorize_team_delete(
    proofs: &Proofs<'_>,
    org: &str,
    team: &str,
    actor: &str,
    consent: &Value,
    keep_apps: bool,
) -> Result<Option<String>, Rejection> {
    let apps = proofs.s.team_apps(org, team);
    if apps.is_empty() {
        return Ok(None);
    }
    if consent.is_null() {
        return Err(DELETE_TEAM_NEEDS_CONSENT.into());
    }
    let stated = super::delete_team_args(team, &apps, keep_apps);
    org_consent(proofs, actor, consent, org, "__delete_team", &stated)
}

/// A `RedeemInvite`: the token's key signs the redemption, so only a node the
/// redeemer gave the token to can make it, never one that only saw the log.
fn authorize_invite_redemption(
    proofs: &Proofs<'_>,
    invite_key: &str,
    account: &str,
    now: u64,
    signature: &str,
) -> Result<Option<String>, Rejection> {
    let state = proofs.s;
    if !state.invites.contains_key(invite_key) {
        return Err(INVITE_GONE.into());
    }
    let used = crate::proof::verify_invite_redeem(invite_key, account, now, signature)?;
    if state.used_proofs.contains(&used) {
        return Err(REDEMPTION_USED);
    }
    Ok(Some(used))
}

/// A refusal to whoever is not a member of the organization: as for one that
/// does not exist (`GET /orgs/{org}` answers them so).
const ORG_NOT_FOUND: Rejection = Rejection::new(RejectionKind::NotFound, "no such organization");
/// A refusal to a member who does not see the team: as for one that does not
/// exist (`GET /orgs/{org}/teams/{team}` answers them so).
const TEAM_NOT_FOUND: Rejection = Rejection::new(RejectionKind::NotFound, "no such team");

/// What [`apply`] refuses of `cmd` for its actor's authority, judged read-only
/// before it is proposed ([`super::prejudge`]). A refusal names the
/// organization (or team) missing to an actor who cannot see it.
pub(super) fn prejudge(s: &SystemState, cmd: &SystemCmd) -> Result<(), Rejection> {
    let (org, team, actor, judged) = match cmd {
        SystemCmd::SetOrgMetadata {
            org,
            metadata,
            actor,
        } => (
            org,
            None,
            actor,
            check_org_metadata(s, org, metadata, actor),
        ),
        SystemCmd::SetMemberMetadata {
            org,
            account,
            metadata,
            actor,
        } => (
            org,
            None,
            actor,
            check_member_metadata(s, org, account, metadata, actor),
        ),
        SystemCmd::CreateTeam {
            org,
            team_id,
            parent,
            join,
            actor,
            member_child_teams,
            ..
        } => (
            org,
            parent.as_deref(),
            actor,
            check_new_team(
                s,
                org,
                team_id,
                actor,
                parent.as_deref(),
                *join,
                *member_child_teams,
            ),
        ),
        SystemCmd::UpdateTeam {
            org,
            team,
            name,
            join,
            metadata,
            actor,
            ..
        } => {
            let judged = s.orgs.get(org).ok_or(NO_SUCH_ORG).and_then(|o| {
                if !o.teams.contains_key(team) {
                    return Err(NO_SUCH_TEAM);
                }
                check_team_changes(
                    o,
                    team,
                    actor,
                    name.is_some(),
                    join.is_some(),
                    metadata.as_ref(),
                )
            });
            (org, Some(team.as_str()), actor, judged)
        }
        _ => return Ok(()),
    };
    judged.map_err(|why| {
        let Some(o) = s.orgs.get(org).filter(|o| o.role(actor).is_some()) else {
            return ORG_NOT_FOUND;
        };
        match team {
            Some(t) if !o.teams.contains_key(t) || !o.sees_team(actor, t) => TEAM_NOT_FOUND,
            _ => why,
        }
    })
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
///
/// A command with more fields than a function takes arguments (`CreateOrg`,
/// `CreateTeam`, `SetTeamMember`, `UpdateTeam`) goes whole to its function,
/// which takes its fields by name.
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let state = &mut m.state;
    match cmd {
        cmd @ SystemCmd::CreateOrg { .. } => create_org(state, cmd),
        SystemCmd::DeleteOrg { org, actor, .. } => delete_org(state, &org, &actor),
        SystemCmd::RenameOrg {
            org, name, actor, ..
        } => rename_org(state, org, name, &actor),
        SystemCmd::SetOrgMetadata {
            org,
            metadata,
            actor,
        } => set_org_metadata(state, &org, metadata, &actor),
        SystemCmd::SetMemberMetadata {
            org,
            account,
            metadata,
            actor,
        } => set_member_metadata(state, &org, account, metadata, &actor),
        SystemCmd::SetOrgRole {
            org,
            account,
            role,
            actor,
            ..
        } => set_org_role(state, &org, account, role, &actor),
        SystemCmd::RemoveOrgMember {
            org,
            account,
            actor,
            ..
        } => remove_org_member(state, &org, &account, &actor),
        cmd @ SystemCmd::CreateTeam { .. } => create_team(state, cmd),
        cmd @ SystemCmd::SetTeamMember { .. } => set_team_member(state, cmd),
        SystemCmd::SetTeamMembers {
            org,
            account,
            teams,
            actor,
            ..
        } => set_team_members(state, &org, &account, teams, &actor),
        cmd @ SystemCmd::UpdateTeam { .. } => update_team(state, cmd),
        SystemCmd::DeleteTeam {
            org,
            team,
            actor,
            keep_apps,
            ..
        } => delete_team(state, &org, &team, &actor, keep_apps),
        SystemCmd::SetCapabilities {
            org,
            team,
            account,
            capabilities,
            actor,
            ..
        } => set_capabilities(state, &org, team, account, capabilities, &actor),
        SystemCmd::SetAllApps {
            org,
            account,
            all_apps,
            actor,
            ..
        } => set_all_apps(state, &org, account, all_apps, &actor),
        SystemCmd::SetDefaultCapabilities {
            org,
            team,
            capabilities,
            actor,
            ..
        } => set_default_capabilities(state, &org, team, capabilities, &actor),
        SystemCmd::RemoveTeamMember {
            org,
            team,
            account,
            actor,
            subtree,
            ..
        } => {
            let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
            remove_team_row(organization, &team, &account, &actor, subtree)?;
            Ok(Value::Null)
        }
        SystemCmd::RemoveTeamMembers {
            org,
            account,
            teams,
            actor,
            ..
        } => remove_team_members(state, &org, &account, &teams, &actor),
        SystemCmd::CreateInvite {
            invite_key,
            invite,
            actor,
            teams_seen,
            ..
        } => create_invite(state, invite_key, invite, actor, teams_seen),
        SystemCmd::RedeemInvite {
            invite_key,
            account,
            now,
            free_when_unchanged,
            clamp_grants,
            ..
        } => {
            check_known_account(state, &account)?;
            let org = state.redeem_org_invite(
                &invite_key,
                &account,
                now,
                free_when_unchanged,
                clamp_grants,
            )?;
            Ok(json!(InviteRedeemed {
                app: None,
                org,
                member: None,
            }))
        }
        SystemCmd::SetOrgSettings {
            org,
            settings,
            actor,
            ..
        } => set_org_settings(state, &org, &settings, &actor),
        SystemCmd::RevokeInvite {
            org,
            invite_key,
            actor,
            ..
        } => revoke_invite(state, &org, &invite_key, &actor),
        SystemCmd::ScopeApps {
            org,
            apps,
            shared,
            nonce,
            actor,
            member_child_teams,
            all_apps_rows,
            by_release,
            ..
        } => state.scope_apps(
            &org,
            apps,
            shared,
            &nonce,
            &actor,
            ScopeAsLogged {
                member_child_teams,
                all_apps_rows,
                by_release,
            },
        ),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached orgs::apply"),
    }
}

/// Create an organization: the rules it must meet, then its record and,
/// when it names a package, that package's app team.
fn create_org(state: &mut SystemState, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::CreateOrg {
        org_id,
        name,
        actor,
        release,
        recovery_key,
        at,
        name_fits,
        personal,
        app_team,
        capabilities,
    } = cmd
    else {
        // SAFETY: `apply` sends only a `CreateOrg` here.
        unreachable!("create_org applies a CreateOrg")
    };
    check_known_account(state, &actor)?;
    if app_team
        .as_ref()
        .is_some_and(|package| !state.packages.contains_key(package))
    {
        return Err(NO_SUCH_PACKAGE);
    }
    if name_fits {
        org_name_fits(&name)?;
    }
    check_defined_capabilities(capabilities)?;
    let recovery = recovery_key
        .map(|key| recovery_key_of(key, at))
        .transpose()?;
    if state.orgs.contains_key(&org_id) {
        return Err(ORG_EXISTS);
    }
    check_org_allowance(state, &actor, personal)?;
    let mut org = new_org(org_id.clone(), name, actor.clone(), release, personal);
    org.recovery = recovery;
    // The default capabilities are set once the app team is made, so its rows
    // are made under a new organization's default; a refused team removes the org.
    let _prev = state.orgs.insert(org_id.clone(), org);
    let Some(package) = app_team else {
        set_new_orgs_default(state, &org_id, capabilities);
        return Ok(json!(OrgCreated {
            org_id,
            team_id: None
        }));
    };
    make_app_team(state, &org_id, package, &actor, capabilities)
}

/// The account exists.
fn check_known_account(state: &SystemState, account: &str) -> Result<(), Rejection> {
    if !state.accounts.contains_key(account) {
        return Err(UNKNOWN_ACCOUNT);
    }
    Ok(())
}

/// Capability bits, when given, name only capabilities that exist.
fn check_defined_capabilities(capabilities: Option<u32>) -> Result<(), Rejection> {
    if capabilities.is_some_and(|bits| !crate::types::Org::defined_caps(bits)) {
        return Err(UNKNOWN_CAPABILITIES);
    }
    Ok(())
}

/// The actor may own one more organization: a personal one only while
/// organizations are not ordinary and they have none, any other within
/// `maxOrgsPerAccount` (once organizations are ordinary, a personal one made
/// before counts too).
fn check_org_allowance(state: &SystemState, actor: &str, personal: bool) -> Result<(), Rejection> {
    if personal && state.ordinary_orgs() {
        return Err(super::NO_PERSONAL_ORGS);
    }
    if personal {
        if state.personal_org_of(actor).is_some() {
            return Err(PERSONAL_ORG_EXISTS);
        }
        return Ok(());
    }
    let owned = state
        .orgs
        .values()
        .filter(|org| !state.acts_personal(org) && org.members.get(actor) == Some(&OrgRole::Owner))
        .count();
    if owned >= state.quotas.max_orgs_per_account as usize {
        return Err(Rejection::quota("organizations"));
    }
    Ok(())
}

/// A new organization's default capabilities, when its creator named them.
fn set_new_orgs_default(state: &mut SystemState, org_id: &str, capabilities: Option<u32>) {
    if let (Some(bits), Some(org)) = (capabilities, state.orgs.get_mut(org_id)) {
        org.default_capabilities = bits;
    }
}

/// The package's app team, as `ScopeApps` makes one for a package with no
/// instances here yet: open (standalone), the creator its lead. Its id comes
/// from the org's, so every replica agrees. A refusal takes the organization
/// back with it: one entry, all or nothing.
fn make_app_team(
    state: &mut SystemState,
    org_id: &str,
    package: String,
    actor: &str,
    capabilities: Option<u32>,
) -> Result<Value, Rejection> {
    let scoped = state.scope_apps(
        org_id,
        Some(vec![ScopeApp {
            package,
            name: None,
            instances: Some(Vec::new()),
            join: None,
            keep: None,
            child_teams: None,
        }]),
        Some(false),
        &format!("{org_id}/app-team"),
        actor,
        ScopeAsLogged::CURRENT,
    );
    match scoped {
        Ok(scope) => {
            set_new_orgs_default(state, org_id, capabilities);
            Ok(json!(OrgCreated {
                org_id: org_id.to_owned(),
                team_id: scope["teams"][0]["teamId"].as_str().map(str::to_owned),
            }))
        }
        Err(why) => {
            let _gone = state.orgs.remove(org_id);
            Err(why)
        }
    }
}

/// Delete an organization (its owner's, owning no deployment) with its
/// invites, publisher bindings and apps.
fn delete_org(state: &mut SystemState, org: &str, actor: &str) -> Result<Value, Rejection> {
    let organization = state.orgs.get(org).ok_or(NO_SUCH_ORG)?;
    if organization.role(actor) != Some(OrgRole::Owner) {
        return Err(ONLY_OWNER_DELETES_ORG);
    }
    let owner = Deployer::Org(org.to_owned());
    if state.deployments.values().any(|d| d.owner == owner) {
        return Err(OWNS_A_DEPLOYMENT);
    }
    let _gone = state.orgs.remove(org);
    state.invites.retain(|_, invite| invite.org != org);
    unbind_publishers(state, org);
    let apps: Vec<String> = state
        .apps
        .values()
        .filter(|app| app.org == org)
        .map(|app| app.app_id.clone())
        .collect();
    for app_id in &apps {
        state.remove_app(app_id);
    }
    Ok(json!(OrgDeleted { apps }))
}

/// A deleted organization's publisher keys go with it, each keeping its
/// generation: an organization created later under this id is not their
/// publisher, and the signatures that bound them stay spent.
fn unbind_publishers(state: &mut SystemState, org: &str) {
    let unbound: Vec<(String, u64)> = state
        .publishers
        .iter()
        .filter(|(_, binding)| binding.org == org)
        .map(|(signer, binding)| (signer.clone(), binding.generation))
        .collect();
    for (signer, generation) in unbound {
        let _gone = state.publishers.remove(&signer);
        let _prev = state.unbound_publishers.insert(signer, generation);
    }
}

/// Rename an organization: its owner's or an admin's.
fn rename_org(
    state: &mut SystemState,
    org: OrgId,
    name: String,
    actor: &str,
) -> Result<Value, Rejection> {
    let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
    if organization.role(actor) < Some(OrgRole::Admin) {
        return Err(ONLY_ADMINS_RENAME.into());
    }
    org_name_fits(&name)?;
    organization.name.clone_from(&name);
    Ok(json!(OrgRenamed { org_id: org, name }))
}

/// Set an organization's metadata: an admin's, or a holder of `MANAGE_METADATA`.
fn set_org_metadata(
    state: &mut SystemState,
    org: &str,
    metadata: Value,
    actor: &str,
) -> Result<Value, Rejection> {
    check_org_metadata(state, org, &metadata, actor)?;
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    organization.metadata = metadata;
    Ok(Value::Null)
}

/// [`set_org_metadata`]'s rules, read-only.
fn check_org_metadata(
    state: &SystemState,
    org: &str,
    metadata: &Value,
    actor: &str,
) -> Result<(), Rejection> {
    metadata_fits(metadata)?;
    let organization = state.orgs.get(org).ok_or(NO_SUCH_ORG)?;
    if organization.role(actor) < Some(OrgRole::Admin)
        && !organization.may(actor, None, crate::types::caps::MANAGE_METADATA)
    {
        return Err(MAY_NOT_SET_ORG_METADATA);
    }
    Ok(())
}

/// Set (or, with null, drop) a member's metadata: theirs or an admin's.
fn set_member_metadata(
    state: &mut SystemState,
    org: &str,
    account: AccountId,
    metadata: Value,
    actor: &str,
) -> Result<Value, Rejection> {
    check_member_metadata(state, org, &account, &metadata, actor)?;
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if metadata.is_null() {
        let _gone = organization.member_metadata.remove(&account);
    } else {
        let _prev = organization.member_metadata.insert(account, metadata);
    }
    Ok(Value::Null)
}

/// [`set_member_metadata`]'s rules, read-only.
fn check_member_metadata(
    state: &SystemState,
    org: &str,
    account: &str,
    metadata: &Value,
    actor: &str,
) -> Result<(), Rejection> {
    metadata_fits(metadata)?;
    let organization = state.orgs.get(org).ok_or(NO_SUCH_ORG)?;
    if organization.role(account).is_none() {
        return Err(NOT_A_MEMBER);
    }
    if actor != account && organization.role(actor) < Some(OrgRole::Admin) {
        return Err(MEMBER_METADATA_THEIRS);
    }
    Ok(())
}

/// Set a member's role: an admin's, ownership the owner's, and the one owner
/// there is becomes an admin when another takes it.
fn set_org_role(
    state: &mut SystemState,
    org: &str,
    account: AccountId,
    role: OrgRole,
    actor: &str,
) -> Result<Value, Rejection> {
    let known = state.accounts.contains_key(&account);
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    let actor_role = organization.role(actor).ok_or(NOT_A_MEMBER)?;
    let current = organization.role(&account);
    if actor_role < OrgRole::Admin {
        return Err(ONLY_ADMINS_MANAGE_MEMBERS);
    }
    if (role == OrgRole::Owner || current == Some(OrgRole::Owner)) && actor_role != OrgRole::Owner {
        return Err(ONLY_OWNER_CHANGES_OWNERSHIP);
    }
    if !known {
        return Err(UNKNOWN_ACCOUNT);
    }
    if role == OrgRole::Owner {
        for member_role in organization.members.values_mut() {
            if *member_role == OrgRole::Owner {
                *member_role = OrgRole::Admin;
            }
        }
    }
    let _prev = organization.members.insert(account, role);
    Ok(Value::Null)
}

/// Remove a member (an admin's, or the member leaving; never the owner) with
/// every row they hold in the organization and its teams.
fn remove_org_member(
    state: &mut SystemState,
    org: &str,
    account: &str,
    actor: &str,
) -> Result<Value, Rejection> {
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    let actor_role = organization.role(actor).ok_or(NOT_A_MEMBER)?;
    let target_role = organization.role(account).ok_or(NOT_A_MEMBER)?;
    let leaving = actor == account;
    if target_role == OrgRole::Owner {
        return Err(TRANSFER_OWNERSHIP_FIRST);
    }
    if !leaving && actor_role < OrgRole::Admin {
        return Err(ONLY_ADMINS_REMOVE_MEMBERS);
    }
    let _prev = organization.members.remove(account);
    let _caps = organization.capabilities.remove(account);
    let _meta = organization.member_metadata.remove(account);
    let _all = organization.all_apps.remove(account);
    for team in organization.teams.values_mut() {
        let _prev = team.members.remove(account);
        let _was = team.read_only.remove(account);
        let _caps = team.capabilities.remove(account);
    }
    Ok(Value::Null)
}

/// Create a team, led by its creator, where they may create one.
fn create_team(state: &mut SystemState, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::CreateTeam {
        org,
        team_id,
        name,
        parent,
        join,
        actor,
        member_child_teams,
    } = cmd
    else {
        // SAFETY: `apply` sends only a `CreateTeam` here.
        unreachable!("create_team applies a CreateTeam")
    };
    check_new_team(
        state,
        &org,
        &team_id,
        &actor,
        parent.as_deref(),
        join,
        member_child_teams,
    )?;
    let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
    let members = BTreeMap::from([(actor, true)]);
    let _prev = organization.teams.insert(
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
    Ok(json!(TeamCreated { team_id }))
}

/// [`create_team`]'s rules, read-only.
fn check_new_team(
    state: &SystemState,
    org: &str,
    team_id: &str,
    actor: &str,
    parent: Option<&str>,
    join: TeamJoin,
    member_child_teams: bool,
) -> Result<(), Rejection> {
    let organization = state.orgs.get(org).ok_or(NO_SUCH_ORG)?;
    if organization.teams.contains_key(team_id) {
        return Err(TEAM_EXISTS);
    }
    if !may_create_team(organization, actor, parent, join, member_child_teams) {
        return Err(MAY_NOT_CREATE_TEAM);
    }
    Ok(())
}

/// Who may create a team: an admin anywhere, a team's lead below it; a member
/// with `CREATE_TEAM` at the top; a member who writes a team and holds
/// `CREATE_CHILD_TEAM` there (an app team's default), a restricted team below
/// it (a private channel), which they lead.
fn may_create_team(
    organization: &Org,
    actor: &str,
    parent: Option<&str>,
    join: TeamJoin,
    member_child_teams: bool,
) -> bool {
    use crate::types::caps;

    let member_creates = |parent: &str| {
        member_child_teams
            && join == TeamJoin::Invite
            && organization.writes_team(actor, parent)
            && organization.may(actor, Some(parent), caps::CREATE_CHILD_TEAM)
    };
    match parent {
        Some(parent) => {
            organization.teams.contains_key(parent)
                && (organization.manages_team(actor, parent) || member_creates(parent))
        }
        None => {
            organization
                .role(actor)
                .is_some_and(|r| r >= OrgRole::Member)
                && organization.may(actor, None, caps::CREATE_TEAM)
        }
    }
}

/// Set one account's row in a team (`set_team_row`'s rules).
fn set_team_member(state: &mut SystemState, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::SetTeamMember {
        org,
        team,
        account,
        lead,
        role,
        capabilities,
        actor,
        ..
    } = cmd
    else {
        // SAFETY: `apply` sends only a `SetTeamMember` here.
        unreachable!("set_team_member applies a SetTeamMember")
    };
    let held = state.cluster_brings(cluster_version::Feature::HeldTeamGrants);
    let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
    let role = role.unwrap_or(if lead {
        TeamRole::Lead
    } else {
        TeamRole::Member
    });
    set_team_row(
        organization,
        &team,
        &account,
        role,
        capabilities,
        &actor,
        held,
    )?;
    Ok(Value::Null)
}

/// Set one account's rows in several teams, all or nothing: each row is
/// judged as its own `SetTeamMember` would be, in order, on a copy kept only
/// when every one holds.
fn set_team_members(
    state: &mut SystemState,
    org: &str,
    account: &str,
    teams: Vec<TeamRow>,
    actor: &str,
) -> Result<Value, Rejection> {
    check_team_batch(teams.iter().map(|row| row.team.as_str()))?;
    let held = state.cluster_brings(cluster_version::Feature::HeldTeamGrants);
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    let mut next = organization.clone();
    for row in teams {
        let role = row.role.unwrap_or(TeamRole::Member);
        set_team_row(
            &mut next,
            &row.team,
            account,
            role,
            row.capabilities,
            actor,
            held,
        )?;
    }
    *organization = next;
    Ok(Value::Null)
}

/// Update a team's name, visibility, parent or metadata, each by whoever may
/// change it; marking or unmarking an app team is installing an app.
fn update_team(state: &mut SystemState, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::UpdateTeam {
        org,
        team,
        name,
        join,
        parent,
        metadata,
        actor,
        all_apps_rows,
        close_drops_below,
    } = cmd
    else {
        // SAFETY: `apply` sends only an `UpdateTeam` here.
        unreachable!("update_team applies an UpdateTeam")
    };
    let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
    if !organization.teams.contains_key(&team) {
        return Err(NO_SUCH_TEAM);
    }
    check_team_changes(
        organization,
        &team,
        &actor,
        name.is_some(),
        join.is_some(),
        metadata.as_ref(),
    )?;
    let current = organization.teams.get(&team).ok_or(NO_SUCH_TEAM)?;
    let parent_after = parent.clone().unwrap_or_else(|| current.parent.clone());
    let package_before = current.app_package().map(str::to_owned);
    let package_after = app_package_after(current, parent_after.as_ref(), metadata.as_ref());
    check_app_team_change(
        organization,
        &team,
        &actor,
        package_before.as_deref(),
        package_after.as_deref(),
    )?;
    if let Some(to) = &parent {
        check_team_move(organization, &team, &actor, to.as_deref())?;
    }
    // Marked as an app team now: an app added to the organization. Someone
    // removed from the team before keeps out of it.
    let all_apps = if all_apps_rows && package_before.is_none() && package_after.is_some() {
        organization.all_apps_members()
    } else {
        Vec::new()
    };
    // Who is in it before a close: those who lose it go from the teams below too.
    let closing = close_drops_below && join == Some(TeamJoin::Invite);
    let inside: Vec<AccountId> = if closing {
        organization
            .members
            .keys()
            .filter(|member| organization.in_team(member, &team))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let record = organization.teams.get_mut(&team).ok_or(NO_SUCH_TEAM)?;
    for account in all_apps {
        if !record.removed.contains(&account) {
            let _row = record.members.entry(account).or_insert(false);
        }
    }
    if let Some(name) = name {
        record.name = name;
    }
    if let Some(join) = join {
        record.join = join;
    }
    if let Some(parent) = parent {
        record.parent = parent;
    }
    if let Some(metadata) = metadata {
        record.metadata = metadata;
    }
    let gone: Vec<AccountId> = inside
        .into_iter()
        .filter(|member| !organization.in_team(member, &team))
        .collect();
    organization.drop_rows_below(&team, &gone);
    Ok(Value::Null)
}

/// Each change a team update makes is the actor's to make: its name a
/// manager's, its visibility `MANAGE_VISIBILITY`'s, its metadata (which must
/// fit) `MANAGE_METADATA`'s.
fn check_team_changes(
    organization: &Org,
    team: &str,
    actor: &str,
    renames: bool,
    sets_visibility: bool,
    metadata: Option<&Value>,
) -> Result<(), Rejection> {
    use crate::types::caps;

    if renames && !organization.manages_team(actor, team) {
        return Err(MAY_NOT_RENAME_TEAM);
    }
    if sets_visibility && !organization.may(actor, Some(team), caps::MANAGE_VISIBILITY) {
        return Err(MAY_NOT_SET_TEAM_VISIBILITY);
    }
    if let Some(metadata) = metadata {
        metadata_fits(metadata)?;
    }
    if metadata.is_some() && !organization.may(actor, Some(team), caps::MANAGE_METADATA) {
        return Err(MAY_NOT_SET_TEAM_METADATA);
    }
    Ok(())
}

/// The package a team is the app team of after an update: only a team at the
/// top is one, by its (new or kept) `metadata.app.package`.
fn app_package_after(
    current: &Team,
    parent_after: Option<&TeamId>,
    metadata: Option<&Value>,
) -> Option<String> {
    if parent_after.is_some() {
        return None;
    }
    metadata
        .unwrap_or(&current.metadata)
        .get("app")
        .and_then(|app| app.get("package"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// An app team (`metadata.app.package` at the top) installs an app in the
/// organization: only an admin makes, changes or drops one, and a package has
/// one app team here.
fn check_app_team_change(
    organization: &Org,
    team: &str,
    actor: &str,
    package_before: Option<&str>,
    package_after: Option<&str>,
) -> Result<(), Rejection> {
    if package_before == package_after {
        return Ok(());
    }
    if !organization
        .role(actor)
        .is_some_and(|r| r >= OrgRole::Admin)
    {
        return Err(ONLY_ADMINS_INSTALL.into());
    }
    if let Some(package) = package_after {
        if organization
            .app_team(package)
            .is_some_and(|app_team| app_team.team_id != team)
        {
            return Err(APP_TEAM_EXISTS.into());
        }
    }
    Ok(())
}

/// A team moves (`to` a team, or to the top) only by whoever manages it and
/// where it goes (at the top: an admin or a holder of `CREATE_TEAM`), and
/// never under itself.
fn check_team_move(
    organization: &Org,
    team: &str,
    actor: &str,
    to: Option<&str>,
) -> Result<(), Rejection> {
    let may_move = organization.manages_team(actor, team)
        && match to {
            Some(parent) => {
                organization.teams.contains_key(parent) && organization.manages_team(actor, parent)
            }
            None => {
                organization
                    .role(actor)
                    .is_some_and(|r| r >= OrgRole::Admin)
                    || organization.may(actor, None, crate::types::caps::CREATE_TEAM)
            }
        };
    if !may_move {
        return Err(MAY_NOT_MOVE_TEAM);
    }
    if let Some(parent) = to {
        if organization
            .subtree(team)
            .iter()
            .any(|below| below == parent)
        {
            return Err(TEAM_UNDER_ITSELF);
        }
    }
    Ok(())
}

/// Delete a team and the teams below it: whoever manages it or holds
/// `DELETE_TEAM`. Its apps go with it unless kept; every app of the
/// organization loses those teams' grants.
fn delete_team(
    state: &mut SystemState,
    org: &str,
    team: &str,
    actor: &str,
    keep_apps: bool,
) -> Result<Value, Rejection> {
    let apps = state.team_apps(org, team);
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if !organization.teams.contains_key(team) {
        return Err(NO_SUCH_TEAM);
    }
    if !organization.manages_team(actor, team)
        && !organization.may(actor, None, crate::types::caps::DELETE_TEAM)
    {
        return Err(MAY_NOT_DELETE_TEAM);
    }
    let gone = organization.subtree(team);
    for team_id in &gone {
        let _removed = organization.teams.remove(team_id);
    }
    if !keep_apps {
        for app_id in &apps {
            state.remove_app(app_id);
        }
    }
    // An instance that keeps another way in, or is kept (`keep_apps`), loses only these
    // teams' grants: with none left, the owner alone reaches it.
    for app in state.apps.values_mut().filter(|app| app.org == org) {
        app.access = app.access.without_teams(&gone);
    }
    if keep_apps {
        return Ok(json!(TeamDeleted {
            teams: gone,
            apps: Vec::new(),
            kept_apps: Some(apps),
        }));
    }
    Ok(json!(TeamDeleted {
        teams: gone,
        apps,
        kept_apps: None,
    }))
}

/// Set a member's capabilities in a team (whoever manages it or holds
/// `MANAGE_MEMBERS` there, for a row of the team) or organization-wide (an admin).
fn set_capabilities(
    state: &mut SystemState,
    org: &str,
    team: Option<TeamId>,
    account: AccountId,
    capabilities: u32,
    actor: &str,
) -> Result<Value, Rejection> {
    let held = state.cluster_brings(cluster_version::Feature::HeldTeamGrants);
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if organization.role(&account).is_none() {
        return Err(NOT_A_MEMBER);
    }
    let Some(team) = team else {
        check_admin(organization, actor)?;
        let _prev = organization.capabilities.insert(account, capabilities);
        return Ok(Value::Null);
    };
    check_manages_members(organization, actor, &team)?;
    team_grant_refusal(organization, &team, actor, Some(capabilities), held)?;
    let record = organization.teams.get_mut(&team).ok_or(NO_SUCH_TEAM)?;
    if !record.members.contains_key(&account) {
        return Err(NOT_IN_THIS_TEAM);
    }
    let _prev = record.capabilities.insert(account, capabilities);
    Ok(Value::Null)
}

/// The actor is an admin (or the owner): only they manage members organization-wide.
fn check_admin(organization: &Org, actor: &str) -> Result<(), Rejection> {
    if !organization
        .role(actor)
        .is_some_and(|r| r >= OrgRole::Admin)
    {
        return Err(ONLY_ADMINS_MANAGE_MEMBERS);
    }
    Ok(())
}

/// The actor manages the team or holds `MANAGE_MEMBERS` there.
fn check_manages_members(organization: &Org, actor: &str, team: &str) -> Result<(), Rejection> {
    if !organization.manages_team(actor, team)
        && !organization.may(actor, Some(team), crate::types::caps::MANAGE_MEMBERS)
    {
        return Err(MAY_NOT_MANAGE_TEAM);
    }
    Ok(())
}

/// Give a member every app added later, or take it back: an admin's.
fn set_all_apps(
    state: &mut SystemState,
    org: &str,
    account: AccountId,
    all_apps: bool,
    actor: &str,
) -> Result<Value, Rejection> {
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if !organization
        .role(actor)
        .is_some_and(|r| r >= OrgRole::Admin)
    {
        return Err(ONLY_ADMINS_ALL_APPS.into());
    }
    if organization.role(&account).is_none() {
        return Err(NOT_A_MEMBER);
    }
    if all_apps {
        let _new = organization.all_apps.insert(account);
    } else {
        let _was = organization.all_apps.remove(&account);
    }
    Ok(Value::Null)
}

/// Set the capabilities new rows get: the organization's (an admin's) or a
/// team's (whoever may set its rows' capabilities).
fn set_default_capabilities(
    state: &mut SystemState,
    org: &str,
    team: Option<TeamId>,
    capabilities: u32,
    actor: &str,
) -> Result<Value, Rejection> {
    let held = state.cluster_brings(cluster_version::Feature::HeldTeamGrants);
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    let Some(team) = team else {
        check_admin(organization, actor)?;
        organization.default_capabilities = capabilities;
        return Ok(Value::Null);
    };
    check_manages_members(organization, actor, &team)?;
    team_grant_refusal(organization, &team, actor, Some(capabilities), held)?;
    let record = organization.teams.get_mut(&team).ok_or(NO_SUCH_TEAM)?;
    record.default_capabilities = Some(capabilities);
    Ok(Value::Null)
}

/// Remove one account's rows in several teams, all or nothing, each judged as
/// its own `RemoveTeamMember` (the rows below included) would be.
fn remove_team_members(
    state: &mut SystemState,
    org: &str,
    account: &str,
    teams: &[TeamId],
    actor: &str,
) -> Result<Value, Rejection> {
    check_team_batch(teams.iter().map(String::as_str))?;
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    let mut next = organization.clone();
    for team in teams {
        remove_team_row(&mut next, team, account, actor, true)?;
    }
    *organization = next;
    Ok(Value::Null)
}

/// Create an organization invite, made by the actor whatever the entry says:
/// the inviter may invite where it does, to that role, granting what it names.
fn create_invite(
    state: &mut SystemState,
    invite_key: String,
    invite: Invite,
    actor: AccountId,
    teams_seen: bool,
) -> Result<Value, Rejection> {
    let organization = state.orgs.get(&invite.org).ok_or(NO_SUCH_ORG)?;
    let role = organization.role(&actor).ok_or(NOT_A_MEMBER)?;
    check_inviter_reach(organization, &invite, &actor, role, teams_seen)?;
    if invite.role > role || (invite.role == OrgRole::Owner) {
        return Err(INVITE_ABOVE_ROLE);
    }
    if invite.all_apps && role < OrgRole::Admin {
        return Err(ONLY_ADMINS_ALL_APPS.into());
    }
    check_defined_capabilities(invite.capabilities)?;
    if invite
        .capabilities
        .is_some_and(|bits| !organization.may_grant(&actor, bits))
    {
        return Err(CANNOT_GRANT_UNHELD);
    }
    if state.sign_up_invites.contains_key(&invite_key) {
        return Err(INVITE_EXISTS);
    }
    let invite = Invite {
        created_by: actor,
        ..invite
    };
    let _prev = state.invites.insert(invite_key, invite);
    Ok(Value::Null)
}

/// Below admin, an inviter names only teams they see (an org-wide
/// `INVITE_MEMBERS`, which `may` falls back to, does not open a restricted
/// team they are not in) and holds `INVITE_MEMBERS` for the organization (an
/// invite naming no team) or for each team named (a team's leads hold it there).
fn check_inviter_reach(
    organization: &Org,
    invite: &Invite,
    actor: &str,
    role: OrgRole,
    teams_seen: bool,
) -> Result<(), Rejection> {
    use crate::types::caps;

    if teams_seen
        && role < OrgRole::Admin
        && !invite
            .teams
            .iter()
            .all(|team| organization.sees_team(actor, team))
    {
        return Err(INVITE_TEAMS_UNSEEN);
    }
    let may_invite = if invite.teams.is_empty() {
        organization.may(actor, None, caps::INVITE_MEMBERS)
    } else {
        invite
            .teams
            .iter()
            .all(|team| organization.may(actor, Some(team), caps::INVITE_MEMBERS))
    };
    if role < OrgRole::Admin && !may_invite {
        return Err(NEEDS_INVITE_MEMBERS);
    }
    Ok(())
}

/// Change an organization's settings: its owner's or an admin's, at least one.
fn set_org_settings(
    state: &mut SystemState,
    org: &str,
    settings: &OrgSettings,
    actor: &str,
) -> Result<Value, Rejection> {
    if !state.is_org_admin(org, actor) {
        return Err(ONLY_ADMINS_CHANGE_SETTINGS);
    }
    if settings.is_empty() {
        return Err(NO_SETTING_TO_CHANGE);
    }
    let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
    if let Some(allow) = settings.allow_guests {
        organization.allow_guests = allow;
    }
    if let Some(allow) = settings.allow_public_links {
        organization.allow_public_links = allow;
    }
    Ok(json!(OrgSettingsSet {
        allow_guests: organization.allow_guests,
        allow_public_links: organization.allow_public_links,
        org: org.to_owned(),
    }))
}

/// Revoke an organization's invite: whoever may see it (`SystemState::sees_invite`).
fn revoke_invite(
    state: &mut SystemState,
    org: &str,
    invite_key: &str,
    actor: &str,
) -> Result<Value, Rejection> {
    let invite = state.invites.get(invite_key).ok_or(INVITE_GONE)?;
    if invite.org != org {
        return Err(INVITE_GONE.into());
    }
    if !state.sees_invite(invite, actor) {
        return Err(ONLY_CREATOR_OR_MANAGERS_REVOKE_ORG_INVITE);
    }
    let _gone = state.invites.remove(invite_key);
    Ok(Value::Null)
}

/// A new organization owned by `owner` alone, approving `release` (the one it
/// was made through) to hold its apps.
pub(super) fn new_org(
    org_id: OrgId,
    name: String,
    owner: AccountId,
    release: Option<String>,
    personal: bool,
) -> Org {
    Org {
        org_id,
        name,
        members: BTreeMap::from([(owner, OrgRole::Owner)]),
        teams: BTreeMap::new(),
        default_capabilities: crate::types::caps::DEFAULT,
        capabilities: BTreeMap::new(),
        approved_releases: release.into_iter().collect(),
        outbound_approvals: BTreeMap::new(),
        metadata: Value::Null,
        member_metadata: BTreeMap::new(),
        upgrade_runs: BTreeMap::new(),
        recovery: None,
        all_apps: BTreeSet::new(),
        personal,
        allow_guests: true,
        allow_public_links: true,
        egress_off: false,
        upgrade_policy: None,
        app_upgrade_policies: BTreeMap::new(),
        pending_upgrades: BTreeMap::new(),
    }
}
