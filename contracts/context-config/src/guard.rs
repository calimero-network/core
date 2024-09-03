use std::fmt;
use std::ops::{Deref, DerefMut};

use near_sdk::near;
use near_sdk::store::IterableSet;

use super::Prefix;
use calimero_context_config::types::SignerId;

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct Guard<T> {
    inner: T,
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

        priviledged.insert(signer_id);

        Self { inner, priviledged }
    }

    pub fn get_mut(&mut self, signer_id: &SignerId) -> Result<GuardMut<'_, T>, UnauthorizedAccess> {
        if !self.priviledged.contains(signer_id) {
            return Err(UnauthorizedAccess { _priv: () });
        }

        Ok(GuardMut { inner: self })
    }

    pub fn priviledged(&self) -> &IterableSet<SignerId> {
        &self.priviledged
    }

    pub fn priviledges(&mut self) -> Priviledges<'_> {
        Priviledges {
            inner: &mut self.priviledged,
        }
    }
}

impl<T> Deref for Guard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug)]
pub struct GuardMut<'a, T> {
    inner: &'a mut Guard<T>,
}

impl<T> GuardMut<'_, T> {
    pub fn priviledges(&mut self) -> Priviledges<'_> {
        Priviledges {
            inner: &mut self.inner.priviledged,
        }
    }
}

impl<T> Deref for GuardMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for GuardMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.inner
    }
}

#[derive(Debug)]
pub struct Priviledges<'a> {
    inner: &'a mut IterableSet<SignerId>,
}

impl Priviledges<'_> {
    pub fn grant(&mut self, signer_id: SignerId) {
        self.inner.insert(signer_id);
    }

    pub fn revoke(&mut self, signer_id: &SignerId) {
        self.inner.remove(signer_id);
    }
}
