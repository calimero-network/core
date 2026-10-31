//! Three access-control patterns, each a table plus an explicit check against the caller.
//!
//! v1 showed these as `calimero_storage` components (`Ownable`, `PermissionedStorage`,
//! `AccessControl`) whose writer sets every node re-checked at merge. Here every write runs
//! on the app's leader and is replicated as a log entry, so the check in the method is the
//! whole boundary: a refused call writes nothing anywhere.
//!
//! * **Owned config cell** (`Ownable`). Rule: only the cell's owner (`env::caller()`'s
//!   account equal to the stored `owner`) may `setConfig` or `transfer`; anyone reads it.
//!   The app owner owns it at `init`.
//! * **Writer-set settings** (`PermissionedStorage`). Rule: only an account in
//!   `settingsWriters` may `settingsSet`; anyone reads. The app owner is the one writer at
//!   `init`.
//! * **Role registry** (`AccessControl`). Rule: only an account in `admins` may
//!   `grantRole` / `revokeRole`; anyone asks `hasRole`. The app owner is the one admin at
//!   `init`.
//!
//! The owner, writers and admins are accounts, never devices: a person on a second device is
//! the same caller. Every method needs an org member (or above); guests get `Forbidden`.

use calimero_app_sdk::prelude::*;

/// The id of the one config cell.
const CONFIG: &str = "config";

/// The owned config cell.
#[derive(Record)]
#[table(key = "id")]
pub struct ConfigCell {
    pub id: String,
    pub owner: String,
    pub value: String,
}

/// An account that may write the settings.
#[derive(Record)]
#[table(key = "account")]
pub struct Writer {
    pub account: String,
}

#[derive(Record)]
#[table(key = "key")]
pub struct Setting {
    pub key: String,
    pub value: String,
}

/// An account that may grant and revoke roles.
#[derive(Record)]
#[table(key = "account")]
pub struct Admin {
    pub account: String,
}

/// `account` holds `role`.
#[derive(Record)]
#[table(key = ["role", "account"])]
pub struct Grant {
    pub role: String,
    pub account: String,
}

#[app::state(version = 1)]
pub struct ComponentsDemo {
    config: Table<ConfigCell>,
    settings_writers: Table<Writer>,
    settings: Table<Setting>,
    admins: Table<Admin>,
    roles: Table<Grant>,
}

#[app::error]
pub enum Error {
    /// The caller does not own the config cell.
    NotOwner,
    /// The caller is not in the settings' writer set.
    NotWriter,
    /// The caller is not a role admin.
    NotAdmin,
}

#[app::logic]
impl ComponentsDemo {
    /// The app owner becomes the config owner, the one settings writer and the one admin.
    #[app::init]
    pub fn init() -> app::Result<Self> {
        let mut s = Self::load();
        let me = env::app_owner();
        s.config.insert(ConfigCell {
            id: CONFIG.to_owned(),
            owner: me.clone(),
            value: String::new(),
        })?;
        s.settings_writers.insert(Writer {
            account: me.clone(),
        })?;
        s.admins.insert(Admin { account: me })?;
        Ok(s)
    }

    /// The caller's account: what an owner, a writer set or a role grant names.
    #[app::view]
    pub fn my_account(&self) -> app::Result<String> {
        guards::require_member()
    }

    /// Grants `role` to `who`. Admins only (`NotAdmin`).
    pub fn grant_role(&mut self, role: String, who: String) -> app::Result<()> {
        self.require_role_admin()?;
        self.roles.upsert(Grant { role, account: who })?;
        Ok(())
    }

    /// Revokes `role` from `who`. Admins only (`NotAdmin`).
    pub fn revoke_role(&mut self, role: String, who: String) -> app::Result<()> {
        self.require_role_admin()?;
        self.roles.remove(&(role.as_str(), who.as_str()))?;
        Ok(())
    }

    /// Whether `who` holds `role`.
    #[app::view]
    pub fn has_role(&self, role: String, who: String) -> app::Result<bool> {
        guards::require_member()?;
        Ok(self.roles.contains(&(role.as_str(), who.as_str()))?)
    }

    /// `grantRole` that answers `false` instead of `NotAdmin` (kept from v1, whose workflow
    /// could not assert a refusal; a real app calls `grantRole`).
    pub fn try_grant_role(&mut self, role: String, who: String) -> app::Result<bool> {
        let me = guards::require_member()?;
        if !self.admins.contains(&me)? {
            return Ok(false);
        }
        self.roles.upsert(Grant { role, account: who })?;
        Ok(true)
    }

    /// Replaces the config. Its owner only (`NotOwner`).
    pub fn set_config(&mut self, value: String) -> app::Result<()> {
        self.require_config_owner()?;
        self.config.update(CONFIG, |c| c.value = value)?;
        Ok(())
    }

    /// The config; `""` until it is first set.
    #[app::view]
    pub fn get_config(&self) -> app::Result<String> {
        guards::require_member()?;
        Ok(self
            .config
            .get(CONFIG)?
            .map(|c| c.value)
            .unwrap_or_default())
    }

    /// The config cell's owner.
    #[app::view]
    pub fn owner(&self) -> app::Result<Option<String>> {
        guards::require_member()?;
        Ok(self.config.get(CONFIG)?.map(|c| c.owner))
    }

    /// Hands the config cell to `newOwner`. Its owner only (`NotOwner`).
    pub fn transfer(&mut self, new_owner: String) -> app::Result<()> {
        self.require_config_owner()?;
        self.config.update(CONFIG, |c| c.owner = new_owner)?;
        Ok(())
    }

    /// Sets a settings entry. Writers only (`NotWriter`).
    pub fn settings_set(&mut self, key: String, value: String) -> app::Result<()> {
        let me = guards::require_member()?;
        if !self.settings_writers.contains(&me)? {
            app::bail!(Error::NotWriter);
        }
        self.settings.upsert(Setting { key, value })?;
        Ok(())
    }

    /// A settings entry, or `null`.
    #[app::view]
    pub fn settings_get(&self, key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        Ok(self.settings.get(&key)?.map(|s| s.value))
    }

    fn require_config_owner(&self) -> app::Result<()> {
        let me = guards::require_member()?;
        match self.config.get(CONFIG)? {
            Some(c) if c.owner == me => Ok(()),
            _ => Err(Error::NotOwner.into()),
        }
    }

    fn require_role_admin(&self) -> app::Result<()> {
        let me = guards::require_member()?;
        if !self.admins.contains(&me)? {
            app::bail!(Error::NotAdmin);
        }
        Ok(())
    }
}
