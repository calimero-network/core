//! Notes an organization keeps, gated by the caller's standing in it (`env::org_role`,
//! `env::in_team`): members read the pinned note, admins write it, and a team's members
//! write that team's note. Method names and arguments are the ABI's camelCase ones.
//!
//! The v1 SDK version this was ported from is kept as a wasmer-runtime fixture,
//! `apps/wasmer-fixtures/org-gated`; `apps/v2-fixtures/org-gated` is the node's component
//! fixture of the same shape. `scenarios/` holds the app-level scenarios (run by
//! `tests/scenarios.rs`) and `scenarios/cluster/` the one that changes a team mid-run.

use calimero_app_sdk::env::{Caller, OrgRole};
use calimero_app_sdk::prelude::*;

#[app::state(version = 1)]
pub struct OrgNotes {
    /// The organization-wide note.
    pinned: Cell<String>,
    /// Each team's note, by team.
    team_notes: UnorderedMap<String, String>,
}

#[app::error]
pub enum Error {
    MembersOnly,
    AdminsOnly,
    TeamOnly,
}

#[app::event]
pub enum Event {
    Announced { text: String },
}

/// What the node told the app about the caller.
#[derive(serde::Serialize)]
pub struct Standing {
    pub org: String,
    pub role: String,
    pub teams: Vec<String>,
}

fn role_name(r: OrgRole) -> &'static str {
    match r {
        OrgRole::Guest => "guest",
        OrgRole::Member => "member",
        OrgRole::Admin => "admin",
        OrgRole::Owner => "owner",
    }
}

/// The calling account, if the caller is one.
fn account() -> Option<String> {
    match env::caller() {
        Caller::Account(a) | Caller::ApiKey { account: a, .. } => Some(a),
        _ => None,
    }
}

/// The caller's role in the app's org; `None` outside one.
fn role() -> Option<OrgRole> {
    env::org()?;
    env::org_role(&account()?)
}

#[app::logic]
impl OrgNotes {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// The caller's standing, or `null` outside the app's organization. The node does not
    /// list a caller's teams, so the caller names `candidates` and gets those it is in.
    #[app::view]
    pub fn whoami(&self, candidates: Option<Vec<String>>) -> app::Result<Option<Standing>> {
        let (Some(org), Some(me), Some(role)) = (env::org(), account(), role()) else {
            return Ok(None);
        };
        let teams = candidates
            .unwrap_or_default()
            .into_iter()
            .filter(|t| env::in_team(&me, t))
            .collect();
        Ok(Some(Standing {
            org,
            role: role_name(role).to_owned(),
            teams,
        }))
    }

    /// The caller's role as the app sees it (`guest`, `member`, `admin`, `owner`), or `null`
    /// outside the app's organization: `whoami` without the org id, which a scenario cannot
    /// know in advance.
    #[app::view]
    pub fn role(&self) -> app::Result<Option<String>> {
        Ok(role().map(|r| role_name(r).to_owned()))
    }

    /// The organization-wide note. Members and above.
    #[app::view]
    pub fn pinned(&self) -> app::Result<String> {
        if !role().is_some_and(|r| r >= OrgRole::Member) {
            return Err(Error::MembersOnly.into());
        }
        Ok(self.pinned.get()?.unwrap_or_default())
    }

    /// Replaces the organization-wide note. Admins and owners only.
    pub fn set_pinned(&mut self, text: String) -> app::Result<()> {
        if !role().is_some_and(|r| r >= OrgRole::Admin) {
            return Err(Error::AdminsOnly.into());
        }
        self.pinned.set(&text)?;
        Ok(())
    }

    /// Tells the organization's members `text` (an `Org` audience event). Members and above.
    pub fn announce(&mut self, text: String) -> app::Result<()> {
        if !role().is_some_and(|r| r >= OrgRole::Member) {
            return Err(Error::MembersOnly.into());
        }
        env::emit(Event::Announced { text }, Audience::Org)?;
        Ok(())
    }

    /// Replaces `team`'s note. Members of that team only.
    pub fn set_team_note(&mut self, team: String, text: String) -> app::Result<()> {
        let allowed = env::org().is_some() && account().is_some_and(|a| env::in_team(&a, &team));
        if !allowed {
            return Err(Error::TeamOnly.into());
        }
        let _old = self.team_notes.insert(&team, &text)?;
        Ok(())
    }

    /// `team`'s note, or `null`.
    #[app::view]
    pub fn team_note(&self, team: String) -> app::Result<Option<String>> {
        Ok(self.team_notes.get(&team)?)
    }
}
