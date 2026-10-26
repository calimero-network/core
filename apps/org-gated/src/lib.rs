//! Notes an organization keeps, gated by the caller's standing in it
//! (`env::caller_org`, release-v2 cloud apps): members read, admins write,
//! and a team's members write that team's note.

use calimero_sdk::app;
use calimero_sdk::env::{self, OrgRole};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state]
pub struct OrgNotes {
    /// The organization-wide note.
    pinned: LwwRegister<String>,
    /// One note per team id.
    team_notes: UnorderedMap<String, LwwRegister<String>>,
}

/// What the node told the app about the caller.
#[derive(Serialize)]
#[serde(crate = "calimero_sdk::serde")]
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

#[app::logic]
impl OrgNotes {
    #[app::init]
    pub fn init() -> OrgNotes {
        OrgNotes {
            pinned: LwwRegister::new(String::new()),
            team_notes: UnorderedMap::new(),
        }
    }

    /// The caller's standing, or `null` outside the app's organization.
    pub fn whoami(&self) -> app::Result<Option<Standing>> {
        Ok(env::caller_org().map(|o| Standing {
            org: o.org,
            role: role_name(o.role).to_owned(),
            teams: o.teams,
        }))
    }

    /// The organization-wide note. Members and above.
    pub fn pinned(&self) -> app::Result<String> {
        if !env::caller_org().is_some_and(|o| o.role >= OrgRole::Member) {
            app::bail!("members only");
        }
        Ok(self.pinned.get().clone())
    }

    /// Replace the organization-wide note. Admins and owners only.
    pub fn set_pinned(&mut self, text: String) -> app::Result<()> {
        if !env::caller_org().is_some_and(|o| o.role >= OrgRole::Admin) {
            app::bail!("admins only");
        }
        self.pinned.set(text);
        Ok(())
    }

    /// Replace `team`'s note. Members of that team only.
    pub fn set_team_note(&mut self, team: String, text: String) -> app::Result<()> {
        if !env::caller_org().is_some_and(|o| o.in_team(&team)) {
            app::bail!("members of that team only");
        }
        drop(self.team_notes.insert(team, LwwRegister::new(text))?);
        Ok(())
    }

    pub fn team_note(&self, team: String) -> app::Result<Option<String>> {
        Ok(self.team_notes.get(&team)?.map(|r| r.get().clone()))
    }
}
