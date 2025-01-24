use core::fmt;
use core::ops::{Deref, DerefMut};

use soroban_sdk::{contracttype, Address, BytesN, Env, Vec};

use crate::types::Application;

/// Represents the different types of values that can be guarded
#[contracttype]
#[derive(Clone, Debug)]
pub enum GuardedValue {
    Application(Application),
    Members(Vec<BytesN<32>>),
    Proxy(Address),
}

/// A guard that protects access to a value with privileged users
#[contracttype]
#[derive(Clone, Debug)]
pub struct Guard {
    inner: GuardedValue,
    revision: u32,
    privileged: Vec<BytesN<32>>,
}

/// Handle for read-only access to a guard
pub struct GuardHandle<'a> {
    guard: &'a mut Guard,
}

/// Handle for mutable access to a guard
pub struct GuardMut<'a> {
    guard: &'a mut Guard,
}

/// Handle for managing privileges
pub struct Privileges<'a> {
    privileged: &'a mut Vec<BytesN<32>>,
}

/// Error type for unauthorized access attempts
#[derive(Debug)]
pub struct UnauthorizedAccess {
    _priv: (),
}

impl fmt::Display for UnauthorizedAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("unauthorized access")
    }
}

impl Deref for Guard {
    type Target = GuardedValue;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Guard {
    /// Creates a new guard with initial privileged user
    /// # Arguments
    /// * `env` - The environment reference
    /// * `creator` - The initial privileged user
    /// * `inner` - The value to be guarded
    pub fn new(env: &Env, creator: &BytesN<32>, inner: GuardedValue) -> Self {
        let mut privileged = Vec::new(env);
        privileged.push_back(creator.clone());

        Self {
            inner,
            revision: 0,
            privileged,
        }
    }

    /// Attempts to get a handle to the guard
    /// # Errors
    /// Returns UnauthorizedAccess if signer is not privileged
    pub fn get(&mut self, signer_id: &BytesN<32>) -> Result<GuardHandle, UnauthorizedAccess> {
        if !self.privileged.contains(signer_id) {
            return Err(UnauthorizedAccess { _priv: () });
        }
        Ok(GuardHandle { guard: self })
    }

    /// Consumes the guard and returns the inner value
    pub fn into_inner(self) -> GuardedValue {
        self.inner
    }

    /// Returns a reference to the privileged users list
    pub fn privileged(&self) -> &Vec<BytesN<32>> {
        &self.privileged
    }

    /// Returns a handle to manage privileges
    pub fn privileges(&mut self) -> Privileges {
        Privileges {
            privileged: &mut self.privileged,
        }
    }

    /// Returns the current revision number
    pub const fn revision(&self) -> u32 {
        self.revision
    }
}

impl<'a> GuardHandle<'a> {
    /// Upgrades to mutable access
    pub fn get_mut(self) -> GuardMut<'a> {
        GuardMut { guard: self.guard }
    }

    /// Returns a handle to manage privileges
    pub fn privileges(&mut self) -> Privileges {
        self.guard.privileges()
    }
}

impl<'a> Deref for GuardHandle<'a> {
    type Target = GuardedValue;

    fn deref(&self) -> &Self::Target {
        &self.guard.inner
    }
}

impl<'a> GuardMut<'a> {
    /// Returns a handle to manage privileges
    pub fn privileges(&mut self) -> Privileges {
        self.guard.privileges()
    }
}

impl<'a> Deref for GuardMut<'a> {
    type Target = GuardedValue;

    fn deref(&self) -> &Self::Target {
        &self.guard.inner
    }
}

impl<'a> DerefMut for GuardMut<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard.inner
    }
}

impl<'a> Drop for GuardMut<'a> {
    fn drop(&mut self) {
        self.guard.revision = self.guard.revision.wrapping_add(1);
    }
}

impl<'a> Privileges<'a> {
    /// Grants privileges to a new user
    pub fn grant(&mut self, signer_id: &BytesN<32>) {
        if !self.privileged.contains(signer_id) {
            self.privileged.push_back(signer_id.clone());
        }
    }

    /// Revokes privileges from a user
    pub fn revoke(&mut self, signer_id: &BytesN<32>) {
        if let Some(index) = self.privileged.iter().position(|x| x == *signer_id) {
            self.privileged.remove(index as u32);
        }
    }
}
