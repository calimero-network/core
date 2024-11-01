use core::ops::{Deref, DerefMut};
use std::fmt;

use calimero_context_config::types::{Revision, SignerId};
use near_sdk::near;
use near_sdk::store::IterableSet;

use super::Prefix;

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct Guard<T> {
    inner: T,
    revision: Revision,
    priviledged: IterableSet<SignerId>,
}

#[derive(Copy, Clone)]
pub struct UnauthorizedAccess {
    _priv: (),
}

impl fmt::Display for UnauthorizedAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("unauthorized access")
    }
}

impl fmt::Debug for UnauthorizedAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<T> Guard<T> {
    pub fn new(prefix: Prefix, signer_id: SignerId, inner: T) -> Self {
        let mut priviledged = IterableSet::new(prefix);
        let _ = priviledged.insert(signer_id);

        Self {
            inner,
            revision: 0,
            priviledged,
        }
    }

    pub fn get(&mut self, signer_id: &SignerId) -> Result<GuardHandle<'_, T>, UnauthorizedAccess> {
        if !self.priviledged.contains(signer_id) {
            return Err(UnauthorizedAccess { _priv: () });
        }

        Ok(GuardHandle { inner: self })
    }

    pub fn into_inner(self) -> T {
        let mut this = self;
        this.priviledged.clear();
        this.inner
    }

    pub const fn priviledged(&self) -> &IterableSet<SignerId> {
        &self.priviledged
    }

    pub fn priviledges(&mut self) -> Priviledges<'_> {
        Priviledges {
            inner: &mut self.priviledged,
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

    pub fn priviledges(&mut self) -> Priviledges<'_> {
        self.inner.priviledges()
    }
}

#[derive(Debug)]
pub struct GuardMut<'a, T> {
    inner: &'a mut Guard<T>,
}

impl<T> GuardMut<'_, T> {
    pub fn priviledges(&mut self) -> Priviledges<'_> {
        self.inner.priviledges()
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
pub struct Priviledges<'a> {
    inner: &'a mut IterableSet<SignerId>,
}

impl Priviledges<'_> {
    pub fn grant(&mut self, signer_id: SignerId) {
        let _ = self.inner.insert(signer_id);
    }

    pub fn revoke(&mut self, signer_id: &SignerId) {
        let _ = self.inner.remove(signer_id);
    }
}
