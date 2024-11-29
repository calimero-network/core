use std::fmt;
use std::ops::{Deref, DerefMut};
use std::collections::BTreeSet;

use calimero_context_config::types::Revision;
use candid::CandidType;
use serde::{Deserialize, Serialize};

use crate::types::ICSignerId;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct Guard<T> {
    inner: T,
    revision: Revision,
    privileged: BTreeSet<ICSignerId>,
}

#[derive(Debug)]
pub struct UnauthorizedAccess {
    _priv: (),
}

impl fmt::Display for UnauthorizedAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("unauthorized access")
    }
}

impl<T> Guard<T> {
    pub fn new(creator: ICSignerId, inner: T) -> Self {
        Self {
            inner,
            revision: 0,
            privileged: BTreeSet::from([creator]),
        }
    }

    pub fn get(
        &mut self,
        signer_id: &ICSignerId,
    ) -> Result<GuardHandle<'_, T>, UnauthorizedAccess> {
        if !self.privileged.contains(signer_id) {
            return Err(UnauthorizedAccess { _priv: () });
        }
        Ok(GuardHandle { inner: self })
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    pub fn privileged(&self) -> &BTreeSet<ICSignerId> {
        &self.privileged
    }

    pub fn privileges(&mut self) -> Privileges<'_> {
        Privileges {
            inner: &mut self.privileged,
        }
    }

    pub const fn revision(&self) -> Revision {
        self.revision
    }
}

impl<T> Deref for Guard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug)]
pub struct GuardHandle<'a, T> {
    inner: &'a mut Guard<T>,
}

impl<'a, T> GuardHandle<'a, T> {
    pub fn get_mut(self) -> GuardMut<'a, T> {
        GuardMut { inner: self.inner }
    }

    pub fn privileges(&mut self) -> Privileges<'_> {
        self.inner.privileges()
    }
}

#[derive(Debug)]
pub struct GuardMut<'a, T> {
    inner: &'a mut Guard<T>,
}

impl<T> GuardMut<'_, T> {
    pub fn privileges(&mut self) -> Privileges<'_> {
        self.inner.privileges()
    }
}

impl<T> Deref for GuardMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner
    }
}

impl<T> DerefMut for GuardMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.inner
    }
}

impl<T> Drop for GuardMut<'_, T> {
    fn drop(&mut self) {
        self.inner.revision = self.inner.revision.wrapping_add(1);
    }
}

#[derive(Debug)]
pub struct Privileges<'a> {
    inner: &'a mut BTreeSet<ICSignerId>,
}

impl Privileges<'_> {
    pub fn grant(&mut self, signer_id: ICSignerId) {
        self.inner.insert(signer_id);
    }

    pub fn revoke(&mut self, signer_id: &ICSignerId) {
        self.inner.remove(signer_id);
    }
}
