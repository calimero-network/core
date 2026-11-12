//! The API's routes for organizations, members, teams and invites.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// The most passkeys `GET /orgs/{org}/members?detail=security` counts before
/// it answers `"3+"`: an admin learns whether a member has a backup, not how
/// many devices they own.
const PASSKEYS_COUNTED: usize = 3;

#[derive(Deserialize)]
pub(super) struct MembersQuery {
    #[serde(default)]
    detail: Option<String>,
}

/// `GET /orgs/{org}/members?detail=security`: for the organization's owner
/// and admins, the lockout risk of each member they see (`Org::sees_member`,
/// everyone for them): their self-set name, role and teams, how many
/// passkeys they hold (`1`, `2`, or `"3+"`), whether a recovery code is set,
/// the UTC day they were last active and how many live sessions they hold.
/// Never a credential id, a passkey's label or times, a session, an origin,
/// a recovery under way or an activity entry: those are the account
/// holder's alone (`/auth/me`, `/auth/sessions`, `/auth/activity`). A member
/// below admin is refused (`403`); someone outside the organization is
/// answered as if it did not exist (`404`). The admin's own session on the
/// cluster's origin only: never an API key or a package-bound session.
async fn org_members(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Query<MembersQuery>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    if q.detail.as_deref() != Some("security") {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "the member listing is `?detail=security`",
        ));
    }
    node.sys_fresh_from(p.sys_index).await;
    let now = now_secs();
    node.sys(|s| {
        let o = s
            .orgs
            .get(&org)
            .filter(|o| o.role(&account).is_some())
            .ok_or_else(|| ApiError::not_found("no such organization"))?;
        if !s.is_org_admin(&org, &account) {
            return Err(ApiError::forbidden(
                "only the organization's owner and admins see its members' security",
            ));
        }
        let used = session_use(s, now);
        let members: Vec<Value> = o
            .members
            .iter()
            .filter(|(m, _)| o.sees_member(&account, m))
            .map(|(m, role)| {
                let acc = s.accounts.get(m);
                let passkeys = acc.map_or(0, |a| a.passkeys.len());
                json!({
                    "account": m,
                    "name": o
                        .member_metadata
                        .get(m)
                        .and_then(|v| v.get("name"))
                        .and_then(Value::as_str),
                    "role": role,
                    "teams": o
                        .teams
                        .keys()
                        .filter(|t| o.in_team(m, t))
                        .collect::<Vec<_>>(),
                    "passkeys": if passkeys >= PASSKEYS_COUNTED {
                        json!(format!("{PASSKEYS_COUNTED}+"))
                    } else {
                        json!(passkeys)
                    },
                    "recoveryCode": s.recovery_keys.values().any(|a| a == m),
                    "lastActiveDay": last_active(s, m, &used).and_then(utc_day),
                    "sessions": used.get(m.as_str()).map_or(0, |u| u.0),
                })
            })
            .collect();
        Ok(Json(json!({ "orgId": org, "members": members })))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OrgBody {
    name: String,
    /// The organization's recovery key (base64url Ed25519 public key): the
    /// creator keeps the seed (`docs/specs/org-recovery.md`).
    #[serde(default)]
    recovery_key: Option<String>,
}

async fn org_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OrgBody>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    crate::system::org_name_fits(&body.name).map_err(malformed)?;
    let org_id = typed_id::<16>();
    let v = node
        .sys_write(&SystemCmd::CreateOrg {
            org_id,
            name: body.name,
            actor: account,
            release: Some(node.own_release()),
            recovery_key: body.recovery_key,
            at: recovery_now(&node),
            name_fits: true,
        })
        .await?;
    Ok(Json(v))
}

async fn org_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let o = node
        .sys(|s| s.orgs.get(&org).cloned())
        .ok_or_else(|| ApiError::not_found("no such organization"))?;
    if o.role(&account).is_none() {
        return Err(ApiError::not_found("no such organization"));
    }
    // Teams a member does not see are left out, as if they did not exist,
    // and so are the people they do not see (`Org::sees_member`): in a shared
    // organization a member has the people of their apps, a guest always.
    let mut v = serde_json::to_value(&o).unwrap_or_default();
    if let Some(teams) = v.get_mut("teams").and_then(Value::as_object_mut) {
        teams.retain(|id, _| o.sees_team(&account, id));
        for team in teams.values_mut() {
            for field in ["members", "capabilities"] {
                if let Some(m) = team.get_mut(field).and_then(Value::as_object_mut) {
                    m.retain(|a, _| o.sees_member(&account, a));
                }
            }
            for field in ["readOnly", "removed"] {
                if let Some(m) = team.get_mut(field).and_then(Value::as_array_mut) {
                    m.retain(|a| a.as_str().is_some_and(|a| o.sees_member(&account, a)));
                }
            }
        }
    }
    if let Some(m) = v.get_mut("allApps").and_then(Value::as_array_mut) {
        m.retain(|a| a.as_str().is_some_and(|a| o.sees_member(&account, a)));
    }
    for field in ["members", "capabilities", "memberMetadata"] {
        if let Some(m) = v.get_mut(field).and_then(Value::as_object_mut) {
            m.retain(|a, _| o.sees_member(&account, a));
        }
    }
    // Who may approve its releases, the keys it publishes, and whether a
    // passed deadline has stopped new work on its own set (`null`: it has
    // not).
    let (approvers, publishes, stall) = node.sys(|s| {
        (
            s.release_approvers(&org),
            s.publishers
                .iter()
                .filter(|(_, b)| b.org == org)
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>(),
            s.stalled_for(Some(&org), "", now_secs()),
        )
    });
    if let Some(obj) = v.as_object_mut() {
        let _prev = obj.insert("mode".into(), json!(o.mode()));
        let _prev = obj.insert("releaseApprovers".into(), json!(approvers));
        let _prev = obj.insert("publishes".into(), json!(publishes));
        let _prev = obj.insert(
            "newWorkStopped".into(),
            stall.map_or(
                Value::Null,
                |st| json!({ "release": st.release, "after": crate::system::rfc3339(st.after) }),
            ),
        );
    }
    Ok(Json(v))
}

async fn org_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let is_owner =
        node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&actor)) == Some(OrgRole::Owner));
    if !is_owner {
        return Err(ApiError::forbidden(
            "only the organization's owner deletes it",
        ));
    }
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__delete_org",
        &json!({ "org": org }),
        confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::DeleteOrg {
            org,
            actor,
            consent,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RenameOrgBody {
    name: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Rename an organization: its owner's or an admin's, with a passkey
/// assertion over the new name (a 412 asks for it), to a name that fits as a
/// new organization's must. Judged on fresh state; `404` outside it.
async fn org_rename(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<RenameOrgBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let role = node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&actor)));
    let Some(role) = role else {
        return Err(ApiError::not_found("no such organization"));
    };
    if role < OrgRole::Admin {
        return Err(ApiError::forbidden(crate::system::ONLY_ADMINS_RENAME));
    }
    crate::system::org_name_fits(&body.name).map_err(malformed)?;
    // Before the passkey step-up: a node that could not apply it refuses it anyway.
    node.require_features(&[Feature::RenameOrg]).await?;
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__rename_org",
        &json!({ "name": body.name }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::RenameOrg {
            org,
            name: body.name,
            actor,
            consent,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AllAppsBody {
    account: String,
    all_apps: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Give a member every app added to the organization from now on, or stop
/// it (`SystemCmd::SetAllApps`): the owner's or an admin's, with a passkey
/// assertion over exactly that (a 412 asks for it). Judged on fresh state;
/// `404` outside the organization.
async fn org_all_apps(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<AllAppsBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let role = node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&actor)));
    let Some(role) = role else {
        return Err(ApiError::not_found("no such organization"));
    };
    if role < OrgRole::Admin {
        return Err(ApiError::forbidden(crate::system::ONLY_ADMINS_ALL_APPS));
    }
    // Before the passkey step-up: a node that could not apply it refuses it anyway.
    node.require_features(&[Feature::AllApps]).await?;
    let args = crate::system::all_apps_args(&body.account, body.all_apps);
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__set_all_apps",
        &args,
        body.confirm,
    )
    .await?;
    let _ok = node
        .sys_write(&SystemCmd::SetAllApps {
            org,
            account: body.account,
            all_apps: body.all_apps,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn org_metadata(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<MetadataBody>,
) -> ApiResult {
    let actor = signed_in(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetOrgMetadata {
            org,
            metadata: body.metadata,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

async fn member_metadata(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, account)): Path<(String, String)>,
    Json(body): Json<MetadataBody>,
) -> ApiResult {
    let actor = signed_in(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::SetMemberMetadata {
            org,
            account,
            metadata: body.metadata,
            actor,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub(super) struct MemberBody {
    account: String,
    role: OrgRole,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

async fn org_set_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<MemberBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let args = json!({ "account": body.account, "role": body.role });
    let consent = org_consent(&node, &p, &org, "__set_org_role", &args, body.confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::SetOrgRole {
            org,
            account: body.account,
            role: body.role,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Remove a member, or leave (`account` = oneself): either way with the
/// actor's passkey (412 first; the retry's JSON body carries `confirm`).
async fn org_remove_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, account)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let args = json!({ "account": account });
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = org_consent(&node, &p, &org, "__remove_member", &args, confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::RemoveOrgMember {
            org,
            account,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub(super) struct TeamBody {
    name: String,
    #[serde(default)]
    parent: Option<String>,
    join: TeamJoin,
}

async fn team_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<TeamBody>,
) -> ApiResult {
    let actor = holder(&node, &headers).await?.account()?;
    let v = node
        .sys_write(&SystemCmd::CreateTeam {
            org,
            team_id: typed_id::<16>(),
            name: body.name,
            parent: body.parent,
            join: body.join,
            actor,
            member_child_teams: true,
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub(super) struct TeamMemberBody {
    account: String,
    #[serde(default)]
    lead: bool,
    #[serde(default)]
    role: Option<crate::types::TeamRole>,
    #[serde(default)]
    capabilities: Option<u32>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

async fn team_set_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
    Json(body): Json<TeamMemberBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let args = json!({
        "team": team, "account": body.account, "lead": body.lead,
        "role": body.role, "capabilities": body.capabilities,
    });
    let consent = org_consent(&node, &p, &org, "__set_team_member", &args, body.confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::SetTeamMember {
            org,
            team,
            account: body.account,
            lead: body.lead,
            role: body.role,
            capabilities: body.capabilities,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TeamRowsBody {
    teams: Vec<crate::system::TeamRow>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TeamIdsBody {
    teams: Vec<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// A batch of teams the API refuses before asking for a passkey (as
/// `SetTeamMembers` and `RemoveTeamMembers` judge it again where they apply).
fn team_batch<'a>(teams: impl ExactSizeIterator<Item = &'a str>) -> Result<(), ApiError> {
    crate::system::check_team_batch(teams).map_err(|e| ApiError::bad_request("BAD_TEAMS", e))
}

/// `account`'s rows in several teams of `org` at once (`SystemCmd::SetTeamMembers`),
/// with one passkey (412 first): each team judged as `POST
/// /orgs/{org}/teams/{team}/members` would judge it, and none set unless
/// every one may be.
async fn member_set_teams(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, account)): Path<(String, String)>,
    Json(body): Json<TeamRowsBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    team_batch(body.teams.iter().map(|r| r.team.as_str()))?;
    let args = crate::system::team_members_args(&account, &body.teams);
    // Before the passkey step-up: a node that could not apply it refuses it anyway.
    node.require_features(&[Feature::TeamMembersBatch]).await?;
    let consent = org_consent(&node, &p, &org, "__set_team_members", &args, body.confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::SetTeamMembers {
            org,
            account,
            teams: body.teams,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Take `account` out of several teams of `org` at once, or leave them
/// (`account` = oneself; `SystemCmd::RemoveTeamMembers`), with one passkey
/// (412 first): each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}`
/// would judge it, the rows below included, and none taken unless every one
/// may be.
async fn member_remove_teams(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, account)): Path<(String, String)>,
    Json(body): Json<TeamIdsBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    team_batch(body.teams.iter().map(String::as_str))?;
    let args = crate::system::remove_team_members_args(&account, &body.teams);
    let method = "__remove_team_members";
    // Before the passkey step-up: a node that could not apply it refuses it anyway.
    node.require_features(&[Feature::TeamMembersBatch]).await?;
    let consent = org_consent(&node, &p, &org, method, &args, body.confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::RemoveTeamMembers {
            org,
            account,
            teams: body.teams,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

/// A team as `account` may see it: its record, its members with their roles,
/// the teams below it they see, and its apps. `None` when they may not see it
/// (a restricted team they are not in looks like no team at all).
fn team_view(
    s: &crate::system::SystemState,
    o: &crate::types::Org,
    team: &str,
    account: &str,
) -> Option<Value> {
    if !o.sees_team(account, team) {
        return None;
    }
    let t = o.teams.get(team)?;
    let members: Vec<Value> = t
        .members
        .keys()
        .map(|a| {
            json!({
                "account": a,
                "role": o.team_role(a, team),
                "capabilities": o.team_caps(a, team),
            })
        })
        .collect();
    let children: Vec<&String> = o
        .teams
        .values()
        .filter(|c| c.parent.as_deref() == Some(team) && o.sees_team(account, &c.team_id))
        .map(|c| &c.team_id)
        .collect();
    let apps: Vec<Value> = s
        .apps
        .values()
        .filter(|a| {
            a.org.as_deref() == Some(o.org_id.as_str())
                && matches!(&a.access, Access::Team { team: t } if t == team)
        })
        .map(|a| json!({ "appId": a.app_id, "name": a.name, "contextId": hex::encode(sha256(a.app_id.as_bytes())) }))
        .collect();
    Some(json!({
        "teamId": t.team_id,
        "name": t.name,
        "parent": t.parent,
        "join": t.join,
        "metadata": t.metadata,
        "package": t.app_package(),
        "defaultCapabilities": t.default_capabilities,
        "members": members,
        "children": children,
        "apps": apps,
        "myRole": o.team_role(account, team),
        "member": o.in_team(account, team),
    }))
}

async fn team_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let v = node
        .sys(|s| {
            let o = s.orgs.get(&org)?;
            o.role(&account)?;
            team_view(s, o, &team, &account)
        })
        .ok_or_else(|| ApiError::not_found("no such team"))?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TeamUpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    join: Option<TeamJoin>,
    /// Present (even `null`) to move the team; `null` moves it to the top.
    #[serde(default, deserialize_with = "crate::types::present")]
    parent: Option<Option<String>>,
    #[serde(default)]
    metadata: Option<Value>,
}

async fn team_update(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
    Json(body): Json<TeamUpdateBody>,
) -> ApiResult {
    let actor = holder(&node, &headers).await?.account()?;
    let _ok = node
        .sys_write(&SystemCmd::UpdateTeam {
            org,
            team,
            name: body.name,
            join: body.join,
            parent: body.parent,
            metadata: body.metadata,
            actor,
            all_apps_rows: true,
            close_drops_below: true,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Delete a team with the teams below it and their apps. When there are
/// apps, with the actor's passkey over them (412 first, its details listing
/// the `apps` that go; the retry's JSON body carries `confirm`).
async fn team_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    // Only someone who may delete it learns which apps go (as `DeleteTeam`
    // judges it again where it applies).
    let (sees, may_delete) = node.sys(|s| {
        s.orgs
            .get(&org)
            .filter(|o| o.teams.contains_key(&team))
            .map_or((false, false), |o| {
                let may = o.manages_team(&actor, &team)
                    || o.may(&actor, None, crate::types::caps::DELETE_TEAM);
                (may || o.sees_team(&actor, &team), may)
            })
    });
    if !sees {
        return Err(ApiError::not_found("no such team"));
    }
    if !may_delete {
        return Err(ApiError::forbidden("not allowed to delete this team"));
    }
    let apps = node.sys(|s| s.team_apps(&org, &team));
    let consent = if apps.is_empty() {
        Value::Null
    } else {
        let args = json!({ "team": team, "apps": apps });
        let confirm = body.and_then(|Json(b)| b.confirm);
        org_consent(&node, &p, &org, "__delete_team", &args, confirm)
            .await
            .map_err(|e| match e.code.as_str() {
                "CONFIRM_REQUIRED" => with_detail(e, "apps", json!(apps)),
                _ => e,
            })?
    };
    let v = node
        .sys_write(&SystemCmd::DeleteTeam {
            org,
            team,
            actor,
            consent,
            apps_need_consent: true,
        })
        .await?;
    Ok(Json(v))
}

/// `e` with `key` added to its details (an object, or made one).
pub(super) fn with_detail(mut e: ApiError, key: &str, value: Value) -> ApiError {
    if !e.details.is_object() {
        *e.details = json!({});
    }
    if let Some(d) = e.details.as_object_mut() {
        let _prev = d.insert(key.to_owned(), value);
    }
    e
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CapabilitiesBody {
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    team: Option<String>,
    capabilities: u32,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// A member's capabilities (`account`, organization-wide or in `team`), or,
/// without an account, the default: the organization's, or with `team` what
/// everyone in that team holds there on top of their own (G6); approved with
/// the actor's passkey (412 first).
async fn org_capabilities(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<CapabilitiesBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let cmd = match body.account {
        Some(account) => {
            let args = json!({
                "team": body.team, "account": account, "capabilities": body.capabilities,
            });
            let consent =
                org_consent(&node, &p, &org, "__set_capabilities", &args, body.confirm).await?;
            SystemCmd::SetCapabilities {
                org,
                team: body.team,
                account,
                capabilities: body.capabilities,
                actor,
                consent,
            }
        }
        None => {
            let args =
                crate::system::default_capabilities_args(body.team.as_deref(), body.capabilities);
            let method = "__set_default_capabilities";
            let consent = org_consent(&node, &p, &org, method, &args, body.confirm).await?;
            SystemCmd::SetDefaultCapabilities {
                org,
                team: body.team,
                capabilities: body.capabilities,
                actor,
                consent,
            }
        }
    };
    let _ok = node.sys_write(&cmd).await?;
    Ok(Json(json!({})))
}

/// Remove a team member, or leave a team; with the actor's passkey (412
/// first; the retry's JSON body carries `confirm`).
async fn team_remove_member(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, team, account)): Path<(String, String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let args = json!({ "team": team, "account": account });
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = org_consent(&node, &p, &org, "__remove_team_member", &args, confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::RemoveTeamMember {
            org,
            team,
            account,
            actor,
            consent,
            subtree: true,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InviteBody {
    role: OrgRole,
    #[serde(default)]
    teams: Vec<String>,
    #[serde(default)]
    ttl_secs: Option<u64>,
    #[serde(default)]
    uses: Option<u32>,
    /// Whoever redeems it also gets every app added later (an admin's).
    #[serde(default)]
    all_apps: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Mint an invite: the actor's, approved with their passkey over the
/// invite's key and terms (412 first). The first request makes the token and
/// keeps it with the challenge; the log records only the token's public key
/// (`proof::invite_key`), so redeeming needs the token itself.
async fn invite_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<InviteBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    if body.all_apps && !node.sys(|s| s.is_org_admin(&org, &actor)) {
        return Err(ApiError::forbidden(crate::system::ONLY_ADMINS_ALL_APPS));
    }
    if body.all_apps {
        // Before the passkey step-up: an earlier release would drop the flag.
        node.require_features(&[Feature::AllApps]).await?;
    }
    let token = format!("inv_{}", b64url(&random::<24>()));
    let invite = Invite {
        org: org.clone(),
        role: body.role,
        teams: body.teams,
        expires_at: now_secs() + body.ttl_secs.unwrap_or(7 * 24 * 3600),
        uses_left: body.uses.unwrap_or(1).max(1),
        created_by: actor.clone(),
        created_at: now_secs(),
        all_apps: body.all_apps,
    };
    let args = crate::system::invite_args(&crate::proof::invite_key(&token), &invite);
    let c = consent_minted(
        &node,
        &p,
        &format!("org:{org}"),
        "__create_invite",
        &args,
        Some(json!(token)),
        body.confirm,
    )
    .await?;
    // The retry records the invite the approval names, minted by the first
    // request; its terms must be the ones this request asks for.
    let token = c
        .carry
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("WRONG_CHALLENGE", "not an invite approval"))?;
    let invite_key = crate::proof::invite_key(&token);
    let invite = Invite {
        expires_at: c
            .args
            .get("expiresAt")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        ..invite
    };
    if crate::system::invite_args(&invite_key, &invite) != c.args {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "the approval was for a different call",
        ));
    }
    let _ok = node
        .sys_write(&SystemCmd::CreateInvite {
            invite_key,
            invite,
            actor,
            consent: c.consent,
            teams_seen: true,
        })
        .await?;
    Ok(Json(json!({ "token": token })))
}

/// Redeem an invite for the signed-in account. This node, holding the token,
/// signs the redemption with the token's key; no passkey prompt. One that
/// changes nothing for the account (already a member, at the invite's role or
/// above, in every team it names) spends no use.
async fn invite_redeem(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    let now = now_secs();
    let v = node
        .sys_write(&SystemCmd::RedeemInvite {
            invite_key: crate::proof::invite_key(&token),
            signature: crate::proof::sign_invite_redeem(&token, &account, now),
            account,
            now,
            free_when_unchanged: true,
        })
        .await?;
    Ok(Json(v))
}

/// An organization invite as its list shows it: its key and terms, never a
/// token (the log never had one).
fn invite_json(key: &str, inv: &Invite, now: u64) -> Value {
    json!({
        "inviteKey": key,
        "role": inv.role,
        "teams": inv.teams,
        "usesLeft": inv.uses_left,
        "expiresAt": inv.expires_at,
        "expired": inv.expires_at <= now,
        "createdBy": (!inv.created_by.is_empty()).then_some(&inv.created_by),
        "createdAt": (inv.created_at != 0).then_some(inv.created_at),
        "allApps": inv.all_apps,
    })
}

/// The organization's pending invites the caller may see and revoke
/// (`SystemState::sees_invite`): every one for an admin, those naming only
/// teams they manage for a team's manager, and their own; `404` to anyone
/// outside the organization.
async fn invite_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let now = now_secs();
    let v = node
        .sys(|s| {
            s.orgs.get(&org)?.role(&account)?;
            let invites: Vec<Value> = s
                .invites
                .iter()
                .filter(|(_, i)| i.org == org && s.sees_invite(i, &account))
                .map(|(k, i)| invite_json(k, i, now))
                .collect();
            Some(json!({ "invites": invites }))
        })
        .ok_or_else(|| ApiError::not_found("no such organization"))?;
    Ok(Json(v))
}

/// Withdraw an invite (its key, from [`invite_list`]): whoever may see it,
/// with their passkey (412 first; the retry's JSON body carries `confirm`).
async fn invite_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, key)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let visible = node.sys(|s| {
        s.invites
            .get(&key)
            .is_some_and(|i| i.org == org && s.sees_invite(i, &actor))
    });
    if !visible {
        return Err(ApiError::not_found(crate::system::INVITE_GONE));
    }
    let confirm = body.and_then(|Json(b)| b.confirm);
    let args = json!({ "inviteKey": key });
    let consent = org_consent(&node, &p, &org, "__revoke_invite", &args, confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::RevokeInvite {
            org,
            invite_key: key,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

/// The organization's app teams (one per package) and its mode, for any
/// member: each team's package, id, name, whether it is open, how many people
/// are in it, and whether the caller is in it or manages it. A guest sees
/// only the app teams they are in. Names only: a team's people are read
/// where the caller sees them (`GET /orgs/{org}/teams/{team}`).
async fn app_team_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let v = node
        .sys(|s| {
            let o = s.orgs.get(&org)?;
            let role = o.role(&account)?;
            let teams: Vec<Value> = o
                .teams
                .values()
                .filter_map(|t| Some((t.app_package()?, t)))
                .filter(|(_, t)| role >= OrgRole::Member || o.in_team(&account, &t.team_id))
                .map(|(package, t)| {
                    let count = o
                        .members
                        .keys()
                        .filter(|m| o.in_team(m, &t.team_id))
                        .count();
                    json!({
                        "package": package,
                        "teamId": t.team_id,
                        "name": t.name,
                        "join": t.join,
                        "memberCount": count,
                        "defaultCapabilities": t.default_capabilities,
                        "member": o.in_team(&account, &t.team_id),
                        "manages": o.manages_team(&account, &t.team_id),
                    })
                })
                .collect();
            Some(json!({ "mode": o.mode(), "appTeams": teams }))
        })
        .ok_or_else(|| ApiError::not_found("no such organization"))?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ScopeAppsBody {
    /// The packages to give app teams (absent: every package with an
    /// organization-access instance here).
    #[serde(default)]
    apps: Option<Vec<crate::system::ScopeApp>>,
    /// Close every app team (absent: when the organization then holds two
    /// or more).
    #[serde(default)]
    shared: Option<bool>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Give apps their own access lists in one step (`SystemCmd::ScopeApps`):
/// an admin's, with their passkey (412 first). Answers each package's team,
/// whether it was made, the instances moved and the rows added, and the
/// organization's mode after.
async fn app_teams_scope(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<ScopeAppsBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    if body
        .apps
        .as_ref()
        .is_some_and(|a| a.len() > crate::system::MAX_SCOPE_PACKAGES)
    {
        return Err(ApiError::bad_request(
            "TOO_MANY_PACKAGES",
            "too many packages in one step",
        ));
    }
    // The API always sets `by_release` (#4931).
    let mut features = scope_app_features(body.apps.as_deref());
    features.push(Feature::ScopeByRelease);
    node.require_features(&features).await?;
    let args = crate::system::scope_apps_args(body.apps.as_deref(), body.shared);
    let consent = org_consent(&node, &p, &org, "__scope_apps", &args, body.confirm).await?;
    Ok(Json(
        node.sys_write(&SystemCmd::ScopeApps {
            org,
            apps: body.apps,
            shared: body.shared,
            nonce: b64url(&random::<16>()),
            actor,
            consent,
            owner_in_team: true,
            accounts_in_app_team: true,
            member_child_teams: true,
            all_apps_rows: true,
            by_release: true,
        })
        .await?,
    ))
}

/// What today's runs of the apps an organization is the developer of used (Q20, D76,
/// D78): fuel and writes, summed over the component apps billed to it as their
/// developer (`usage::developer`: instances of its deployments, and its own apps made
/// from a release), on the UTC day of the node's app time, with the quotas; for its
/// admins and owner only, judged on fresh system state like `GET /orgs/{org}`. One
/// who is not a member learns nothing of it (404); a member below admin gets 403.
async fn org_usage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    node.sys_fresh().await;
    let (role, apps, quotas) = node.sys(|s| {
        (
            s.orgs.get(&org).and_then(|o| o.role(&account)),
            s.apps
                .values()
                .filter(|a| crate::usage::Billed::of(s, a).developer == org)
                .count(),
            s.quotas.clone(),
        )
    });
    match role {
        None => return Err(ApiError::not_found("no such organization")),
        Some(r) if r < OrgRole::Admin => {
            return Err(ApiError::forbidden(
                "only the organization's admins read its usage",
            ))
        }
        Some(_) => {}
    }
    let today = node.developer_usage_today(&org).await;
    Ok(Json(json!({
        "apps": apps,
        "developerFuelToday": today.fuel.to_string(),
        "developerWritesToday": today.writes.to_string(),
        "quotas": quotas,
    })))
}

/// An organization's apps, what they run and who upgraded them, the
/// releases it approved and those the cluster's nodes run: for any of its
/// members; `404` to anyone else, as `GET /orgs/{org}`.
async fn org_apps(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let limit = q.limit()?;
    let member = node.sys(|s| s.orgs.get(&org).is_some_and(|o| o.role(&account).is_some()));
    if !member {
        return Err(ApiError::not_found("no such organization"));
    }
    Ok(Json(
        scope_report(&node, &p, &account, Some(&org), &q, limit).await,
    ))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/orgs", post(org_create))
        .route("/orgs/{org}", get(org_get).delete(org_delete))
        .route("/orgs/{org}/name", post(org_rename))
        .route("/orgs/{org}/metadata", post(org_metadata))
        .route("/orgs/{org}/usage", get(org_usage))
        .route(
            "/orgs/{org}/members/{account}/metadata",
            post(member_metadata),
        )
        .route("/orgs/{org}/members", get(org_members).post(org_set_member))
        .route("/orgs/{org}/members/{account}", delete(org_remove_member))
        .route(
            "/orgs/{org}/members/{account}/teams",
            post(member_set_teams).delete(member_remove_teams),
        )
        .route("/orgs/{org}/teams", post(team_create))
        .route(
            "/orgs/{org}/teams/{team}",
            get(team_get).post(team_update).delete(team_delete),
        )
        .route("/orgs/{org}/capabilities", post(org_capabilities))
        .route("/orgs/{org}/all-apps", post(org_all_apps))
        .route("/orgs/{org}/apps", get(org_apps))
        .route("/orgs/{org}/teams/{team}/members", post(team_set_member))
        .route(
            "/orgs/{org}/teams/{team}/members/{account}",
            delete(team_remove_member),
        )
        .route("/orgs/{org}/invites", post(invite_create).get(invite_list))
        .route("/orgs/{org}/invites/{key}", delete(invite_revoke))
        .route(
            "/orgs/{org}/app-teams",
            get(app_team_list).post(app_teams_scope),
        )
        .route("/invites/{token}/redeem", post(invite_redeem))
}
