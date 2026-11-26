//! A team's shared space: notes, a status line, settings, a config and roles, each guarded
//! by an explicit check against the caller. Three layers of access control, in the order a
//! call meets them:
//!
//! 1. **The node decides who reaches the app** (its access: an organization, a team, listed
//!    accounts). The app never sees a caller the node turned away.
//! 2. **Organization standing** (`env::org_role`, `env::in_team`), which the node keeps and
//!    the app only reads:
//!    * the pinned note: members (and above) read it with `pinned`, admins and the owner
//!      replace it with `setPinned` (`MembersOnly`, `AdminsOnly`);
//!    * `announce` tells the organization's members something (an `Org` audience event),
//!      members and above (`MembersOnly`);
//!    * a team's note: only that team's members write it with `setTeamNote`, not even the
//!      organization's owner otherwise (`TeamOnly`); anyone reads it with `teamNote`.
//!
//!    `whoami` and `role` say what the node told the app about the caller.
//! 3. **The app's own rules**, kept in its state, judged by the caller's account
//!    (`myAccount`), never a device:
//!    * **an owned config**: one account owns it; only the owner may `setConfig` or
//!      `transfer` it to another account (`NotOwner`); anyone reads `getConfig` and
//!      `owner`;
//!    * **writer sets**: rows of the `writers` table, one per (set, account). The set
//!      `value` guards the status line (`setShared`), `map` the settings map (`mapSet`) and
//!      `roles` the role registry (`grantRole`, `revokeRole`); anyone else gets
//!      `NotWriter { field }` and nothing is written. Anyone reads the status line
//!      (`getShared`), the settings (`mapGet`) and the roles (`hasRole`);
//!    * **rotation**: a writer of a set replaces that set with `rotateWriters` (the status
//!      line's unless `field` names another), one to `MAX_WRITERS` accounts (`NoWriters`,
//!      `TooManyWriters`); the other sets stay as they were, so handing the status line to
//!      someone gives them neither the settings nor the roles.
//!
//! The app owner, who runs `init`, owns the config and is the one writer of every set.
//! Layer 3 does not ask about the organization: anyone the node admits is judged by the
//! app's own rows alone (`crates/node/tests/it/shared_storage.rs` reaches the app as a listed
//! account outside its organization).
//!
//! v1 checked writer sets on every node at merge (`SharedStorage`, `AccessControl`). Here a
//! write runs once, on the app's leader, and is replicated as a log entry, so the check in
//! the method is the whole boundary: a refused call writes nothing anywhere.
//!
//! `tryGrantRole` and `tryRotateWriters` answer `false` where the plain method fails; v1's
//! workflows could not assert a refusal, and the scenarios keep them. The node's cluster
//! tests run this app too (`crates/node/tests/it/component_cluster.rs`, `shared_storage.rs`).
//! Method names and arguments are the ABI's camelCase ones.

use std::collections::BTreeSet;

use calimero_sdk::env::{Caller, OrgRole};
use calimero_sdk::prelude::*;

/// The writer set of the status line.
const VALUE: &str = "value";
/// The writer set of the settings map.
const MAP: &str = "map";
/// The writer set of the role registry: who grants and revokes roles.
const ROLES: &str = "roles";

/// The most accounts one writer set holds.
const MAX_WRITERS: u32 = 1000;

/// One account allowed to write what one set guards.
#[derive(Record)]
#[table(key = ["field", "account"], index(by_field = "field"), count(per_field = "field"))]
pub struct Writer {
    /// `"value"`, `"map"` or `"roles"`.
    pub field: String,
    pub account: String,
}

/// One entry of the settings map.
#[derive(Record)]
#[table(key = "key")]
pub struct Entry {
    pub key: String,
    pub value: String,
}

/// `account` holds `role`.
#[derive(Record)]
#[table(key = ["role", "account"])]
pub struct Grant {
    pub role: String,
    pub account: String,
}

#[app::state(version = 1)]
pub struct TeamSpace {
    /// The organization-wide note.
    pinned: Cell<String>,
    /// Each team's note, by team.
    team_notes: UnorderedMap<String, String>,
    /// The account that owns the config.
    config_owner: Cell<String>,
    config: Cell<String>,
    /// Every writer set.
    writers: Table<Writer>,
    /// The status line.
    value: Cell<String>,
    /// The settings map.
    map: Table<Entry>,
    roles: Table<Grant>,
}

#[app::error]
pub enum Error {
    MembersOnly,
    AdminsOnly,
    TeamOnly,
    /// The caller does not own the config.
    NotOwner,
    /// The caller is not in the writer set `field`.
    NotWriter {
        field: String,
    },
    /// A writer set must name at least one account.
    NoWriters,
    /// A writer set names at most `max` accounts.
    TooManyWriters {
        max: u32,
    },
}

#[app::event]
pub enum Event {
    Announced { text: String },
    SharedSet { value: String },
    WritersRotated { field: String, count: u32 },
}

/// What the node told the app about the caller.
#[derive(serde::Serialize, AbiType)]
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
fn org_role() -> Option<OrgRole> {
    env::org()?;
    env::org_role(&account()?)
}

#[app::logic]
impl TeamSpace {
    /// The app owner owns the config and is the one writer of every set.
    #[app::init]
    pub fn init() -> app::Result<Self> {
        let mut s = Self::load();
        let owner = env::app_owner();
        s.config_owner.set(&owner)?;
        for field in [VALUE, MAP, ROLES] {
            s.writers.insert(Writer {
                field: field.to_owned(),
                account: owner.clone(),
            })?;
        }
        Ok(s)
    }

    /// The caller's standing, or `null` outside the app's organization. The node does not
    /// list a caller's teams, so the caller names `candidates` and gets those it is in.
    #[app::view]
    pub fn whoami(&self, candidates: Option<Vec<String>>) -> app::Result<Option<Standing>> {
        let (Some(org), Some(me), Some(role)) = (env::org(), account(), org_role()) else {
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
        Ok(org_role().map(|r| role_name(r).to_owned()))
    }

    /// The caller's account: what the config's owner, a writer set or a role grant names.
    #[app::view]
    pub fn my_account(&self) -> app::Result<String> {
        account().ok_or_else(AppError::login_required)
    }

    /// The organization-wide note. Members and above.
    #[app::view]
    pub fn pinned(&self) -> app::Result<String> {
        if !org_role().is_some_and(|r| r >= OrgRole::Member) {
            return Err(Error::MembersOnly.into());
        }
        Ok(self.pinned.get()?.unwrap_or_default())
    }

    /// Replaces the organization-wide note. Admins and owners only.
    pub fn set_pinned(&mut self, text: String) -> app::Result<()> {
        if !org_role().is_some_and(|r| r >= OrgRole::Admin) {
            return Err(Error::AdminsOnly.into());
        }
        self.pinned.set(&text)?;
        Ok(())
    }

    /// Tells the organization's members `text` (an `Org` audience event). Members and above.
    pub fn announce(&mut self, text: String) -> app::Result<()> {
        if !org_role().is_some_and(|r| r >= OrgRole::Member) {
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

    /// Replaces the config. Its owner only (`NotOwner`).
    pub fn set_config(&mut self, value: String) -> app::Result<()> {
        self.require_config_owner()?;
        self.config.set(&value)?;
        Ok(())
    }

    /// The config; `""` until it is first set.
    #[app::view]
    pub fn get_config(&self) -> app::Result<String> {
        Ok(self.config.get()?.unwrap_or_default())
    }

    /// The config's owner.
    #[app::view]
    pub fn owner(&self) -> app::Result<Option<String>> {
        Ok(self.config_owner.get()?)
    }

    /// Hands the config to `newOwner`. Its owner only (`NotOwner`).
    pub fn transfer(&mut self, new_owner: String) -> app::Result<()> {
        self.require_config_owner()?;
        self.config_owner.set(&new_owner)?;
        Ok(())
    }

    /// Replaces the status line. Writers of `value` only.
    pub fn set_shared(&mut self, value: String) -> app::Result<()> {
        self.require_writer(VALUE)?;
        self.value.set(&value)?;
        env::emit(Event::SharedSet { value }, Audience::All)?;
        Ok(())
    }

    /// The status line; `""` before the first `setShared`.
    #[app::view]
    pub fn get_shared(&self) -> app::Result<String> {
        Ok(self.value.get()?.unwrap_or_default())
    }

    /// Sets a setting. Writers of `map` only.
    pub fn map_set(&mut self, key: String, value: String) -> app::Result<()> {
        self.require_writer(MAP)?;
        self.map.upsert(Entry { key, value })?;
        Ok(())
    }

    /// A setting, or `null`.
    #[app::view]
    pub fn map_get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.map.get(&key)?.map(|e| e.value))
    }

    /// Grants `role` to `who`. Writers of `roles` only.
    pub fn grant_role(&mut self, role: String, who: String) -> app::Result<()> {
        self.require_writer(ROLES)?;
        self.roles.upsert(Grant { role, account: who })?;
        Ok(())
    }

    /// `grantRole` that answers `false` instead of `NotWriter`.
    pub fn try_grant_role(&mut self, role: String, who: String) -> app::Result<bool> {
        if self.require_writer(ROLES).is_err() {
            return Ok(false);
        }
        self.grant_role(role, who)?;
        Ok(true)
    }

    /// Revokes `role` from `who`. Writers of `roles` only.
    pub fn revoke_role(&mut self, role: String, who: String) -> app::Result<()> {
        self.require_writer(ROLES)?;
        self.roles.remove(&(role.as_str(), who.as_str()))?;
        Ok(())
    }

    /// Whether `who` holds `role`.
    #[app::view]
    pub fn has_role(&self, role: String, who: String) -> app::Result<bool> {
        Ok(self.roles.contains(&(role.as_str(), who.as_str()))?)
    }

    /// Replaces the writer set `field` (`value` when absent) with `newWriters`. Writers of
    /// that set only; every other set stays as it was.
    pub fn rotate_writers(
        &mut self,
        new_writers: Vec<String>,
        field: Option<String>,
    ) -> app::Result<()> {
        let field = field.unwrap_or_else(|| VALUE.to_owned());
        self.require_writer(&field)?;
        let count = self.replace_writers(&field, new_writers)?;
        env::emit(Event::WritersRotated { field, count }, Audience::All)?;
        Ok(())
    }

    /// `rotateWriters` that answers `false` instead of `NotWriter`.
    pub fn try_rotate_writers(
        &mut self,
        new_writers: Vec<String>,
        field: Option<String>,
    ) -> app::Result<bool> {
        if self
            .require_writer(field.as_deref().unwrap_or(VALUE))
            .is_err()
        {
            return Ok(false);
        }
        self.rotate_writers(new_writers, field)?;
        Ok(true)
    }

    /// How many accounts are in the writer set `field` (`value` when absent).
    #[app::view]
    pub fn writer_count(&self, field: Option<String>) -> app::Result<u64> {
        let field = field.as_deref().unwrap_or(VALUE);
        Ok(self.writers.count("perField", &[field.to_key_part()])?)
    }

    fn require_config_owner(&self) -> app::Result<()> {
        let me = account();
        if me.is_none() || self.config_owner.get()? != me {
            app::bail!(Error::NotOwner);
        }
        Ok(())
    }

    /// An unknown `field` has no rows, so it refuses everyone.
    fn require_writer(&self, field: &str) -> app::Result<()> {
        let allowed = match account() {
            Some(me) => self.writers.contains(&(field, me.as_str()))?,
            None => false,
        };
        if !allowed {
            app::bail!(Error::NotWriter {
                field: field.to_owned()
            });
        }
        Ok(())
    }

    fn replace_writers(&mut self, field: &str, accounts: Vec<String>) -> app::Result<u32> {
        let accounts: BTreeSet<String> = accounts.into_iter().collect();
        if accounts.is_empty() {
            app::bail!(Error::NoWriters);
        }
        let count = u32::try_from(accounts.len()).unwrap_or(u32::MAX);
        if count > MAX_WRITERS {
            app::bail!(Error::TooManyWriters { max: MAX_WRITERS });
        }
        let old = self.writers.find(
            "byField",
            &[field.to_key_part()],
            FindOpts::default().limit(MAX_WRITERS),
        )?;
        for w in old.items {
            self.writers.remove(&(field, w.account.as_str()))?;
        }
        for account in accounts {
            self.writers.insert(Writer {
                field: field.to_owned(),
                account,
            })?;
        }
        Ok(count)
    }
}
