//! System commands for organizations, members, teams and invites.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(
    proofs: &Proofs<'_>,
    cmd: &SystemCmd,
) -> Result<Option<String>, &'static str> {
    let s = proofs.s;
    match cmd {
        SystemCmd::DeleteOrg {
            org,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&st, &format!("org:{org}"), "__delete_org", &|a| {
                a["org"] == json!(org)
            })?;
            Ok(Some(used))
        }
        SystemCmd::RenameOrg {
            org,
            name,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&st, &format!("org:{org}"), "__rename_org", &|a| {
                *a == json!({ "name": name })
            })?;
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "account": account, "role": role });
            proofs.states(&st, &format!("org:{org}"), "__set_org_role", &|a| {
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "account": account });
            proofs.states(&st, &format!("org:{org}"), "__remove_member", &|a| {
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({
                "team": team, "account": account, "lead": lead,
                "role": role, "capabilities": capabilities,
            });
            proofs.states(&st, &format!("org:{org}"), "__set_team_member", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetTeamMembers {
            org,
            account,
            teams,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = team_members_args(account, teams);
            proofs.states(&st, &format!("org:{org}"), "__set_team_members", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemoveTeamMembers {
            org,
            account,
            teams,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = remove_team_members_args(account, teams);
            proofs.states(&st, &format!("org:{org}"), "__remove_team_members", &|a| {
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "team": team, "apps": apps });
            proofs.states(&st, &format!("org:{org}"), "__delete_team", &|a| *a == args)?;
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "team": team, "account": account });
            proofs.states(&st, &format!("org:{org}"), "__remove_team_member", &|a| {
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "team": team, "account": account, "capabilities": capabilities });
            proofs.states(&st, &format!("org:{org}"), "__set_capabilities", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetAllApps {
            org,
            account,
            all_apps,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = all_apps_args(account, *all_apps);
            proofs.states(&st, &format!("org:{org}"), "__set_all_apps", &|a| {
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = default_capabilities_args(team.as_deref(), *capabilities);
            proofs.states(
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = invite_args(invite_key, invite);
            proofs.states(
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "inviteKey": invite_key });
            proofs.states(&st, &format!("org:{org}"), "__revoke_invite", &|a| {
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
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = scope_apps_args(apps.as_deref(), *shared);
            proofs.states(&st, &format!("org:{org}"), "__scope_apps", &|a| *a == args)?;
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
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let s = &mut m.state;
    match cmd {
        SystemCmd::CreateOrg {
            org_id,
            name,
            actor,
            release,
            recovery_key,
            at,
            name_fits,
            personal,
            app_team,
        } => {
            if !s.accounts.contains_key(&actor) {
                return Err("unknown account");
            }
            if app_team
                .as_ref()
                .is_some_and(|p| !s.packages.contains_key(p))
            {
                return Err("no such package");
            }
            if name_fits {
                org_name_fits(&name)?;
            }
            let recovery = recovery_key.map(|k| recovery_key_of(k, at)).transpose()?;
            if s.orgs.contains_key(&org_id) {
                return Err("organization exists");
            }
            if personal {
                if s.personal_org_of(&actor).is_some() {
                    return Err(PERSONAL_ORG_EXISTS);
                }
            } else {
                let owned = s
                    .orgs
                    .values()
                    .filter(|o| !o.personal && o.members.get(&actor) == Some(&OrgRole::Owner))
                    .count();
                if owned >= s.quotas.max_orgs_per_account as usize {
                    return Err("quota exceeded: organizations");
                }
            }
            let mut org = new_org(org_id.clone(), name, actor.clone(), release, personal);
            org.recovery = recovery;
            let _prev = s.orgs.insert(org_id.clone(), org);
            let Some(package) = app_team else {
                return Ok(json!({ "orgId": org_id }));
            };
            // The package's app team, as `ScopeApps` makes one for a package
            // with no instances here yet: open (standalone), the creator its
            // lead. Its id comes from the org's, so every replica agrees. A
            // refusal takes the organization back with it: one entry, all or
            // nothing.
            let scoped = s.scope_apps(
                &org_id,
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
                &actor,
                ScopeAsLogged::CURRENT,
            );
            match scoped {
                Ok(v) => Ok(json!({ "orgId": org_id, "teamId": v["teams"][0]["teamId"] })),
                Err(why) => {
                    let _gone = s.orgs.remove(&org_id);
                    Err(why)
                }
            }
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
            // Its publisher keys go with it, each keeping its generation:
            // an organization created later under this id is not their
            // publisher, and the signatures that bound them stay spent.
            let unbound: Vec<(String, u64)> = s
                .publishers
                .iter()
                .filter(|(_, b)| b.org == org)
                .map(|(k, b)| (k.clone(), b.generation))
                .collect();
            for (signer, generation) in unbound {
                let _gone = s.publishers.remove(&signer);
                let _prev = s.unbound_publishers.insert(signer, generation);
            }
            let apps: Vec<String> = s
                .apps
                .values()
                .filter(|a| a.org == org)
                .map(|a| a.app_id.clone())
                .collect();
            for a in &apps {
                s.remove_app(a);
            }
            Ok(json!({ "apps": apps }))
        }
        SystemCmd::RenameOrg {
            org, name, actor, ..
        } => {
            let o = m.org_mut(&org)?;
            if o.role(&actor) < Some(OrgRole::Admin) {
                return Err(ONLY_ADMINS_RENAME);
            }
            org_name_fits(&name)?;
            o.name.clone_from(&name);
            Ok(json!({ "orgId": org, "name": name }))
        }
        SystemCmd::SetOrgMetadata {
            org,
            metadata,
            actor,
        } => {
            metadata_fits(&metadata)?;
            let o = m.org_mut(&org)?;
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
            let o = m.org_mut(&org)?;
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
        SystemCmd::SetOrgRole {
            org,
            account,
            role,
            actor,
            ..
        } => {
            let known = s.accounts.contains_key(&account);
            let o = m.org_mut(&org)?;
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
            let o = m.org_mut(&org)?;
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
            let _all = o.all_apps.remove(&account);
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
            let o = m.org_mut(&org)?;
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
            let o = m.org_mut(&org)?;
            let role = role.unwrap_or(if lead {
                TeamRole::Lead
            } else {
                TeamRole::Member
            });
            set_team_row(o, &team, &account, role, capabilities, &actor)?;
            Ok(Value::Null)
        }
        SystemCmd::SetTeamMembers {
            org,
            account,
            teams,
            actor,
            ..
        } => {
            check_team_batch(teams.iter().map(|r| r.team.as_str()))?;
            // All or nothing: each row is judged as its own
            // `SetTeamMember` would be, in order, on a copy kept only
            // when every one holds.
            let o = m.org_mut(&org)?;
            let mut next = o.clone();
            for row in teams {
                let role = row.role.unwrap_or(TeamRole::Member);
                set_team_row(
                    &mut next,
                    &row.team,
                    &account,
                    role,
                    row.capabilities,
                    &actor,
                )?;
            }
            *o = next;
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
            all_apps_rows,
            close_drops_below,
        } => {
            let o = m.org_mut(&org)?;
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
            // Marked as an app team now: an app added to the organization.
            // Someone removed from the team before keeps out of it.
            let all_apps = if all_apps_rows && package_before.is_none() && package_after.is_some() {
                o.all_apps_members()
            } else {
                Vec::new()
            };
            // Who is in it before a close: those who lose it go from the
            // teams below too.
            let closing = close_drops_below && join == Some(TeamJoin::Invite);
            let inside: Vec<AccountId> = if closing {
                o.members
                    .keys()
                    .filter(|m| o.in_team(m, &team))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            let t = o.teams.get_mut(&team).ok_or("no such team")?;
            for a in all_apps {
                if !t.removed.contains(&a) {
                    let _row = t.members.entry(a).or_insert(false);
                }
            }
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
            let gone: Vec<AccountId> = inside
                .into_iter()
                .filter(|m| !o.in_team(m, &team))
                .collect();
            o.drop_rows_below(&team, &gone);
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
            let o = m.org_mut(&org)?;
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
        SystemCmd::SetAllApps {
            org,
            account,
            all_apps,
            actor,
            ..
        } => {
            let o = m.org_mut(&org)?;
            if !o.role(&actor).is_some_and(|r| r >= OrgRole::Admin) {
                return Err(ONLY_ADMINS_ALL_APPS);
            }
            if o.role(&account).is_none() {
                return Err("not a member");
            }
            if all_apps {
                let _new = o.all_apps.insert(account);
            } else {
                let _was = o.all_apps.remove(&account);
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
            let o = m.org_mut(&org)?;
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
            let o = m.org_mut(&org)?;
            remove_team_row(o, &team, &account, &actor, subtree)?;
            Ok(Value::Null)
        }
        SystemCmd::RemoveTeamMembers {
            org,
            account,
            teams,
            actor,
            ..
        } => {
            check_team_batch(teams.iter().map(String::as_str))?;
            // All or nothing, each judged as its own `RemoveTeamMember`
            // (the rows below included) would be.
            let o = m.org_mut(&org)?;
            let mut next = o.clone();
            for team in &teams {
                remove_team_row(&mut next, team, &account, &actor, true)?;
            }
            *o = next;
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
            if invite.all_apps && role < OrgRole::Admin {
                return Err(ONLY_ADMINS_ALL_APPS);
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
            accounts_in_app_team,
            member_child_teams,
            all_apps_rows,
            by_release,
            ..
        } => s.scope_apps(
            &org,
            apps,
            shared,
            &nonce,
            &actor,
            ScopeAsLogged {
                owner_in_team,
                accounts_in_app_team,
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
    }
}
