//! The harness org and the `Directory` the host core asks about it.

use std::collections::BTreeSet;

use calimero_app_host::{Directory, OrgRole};

/// The account that owns the app and runs `init`.
pub const OWNER: &str = "owner";
/// The org id the harness installs the app in.
pub const ORG_ID: &str = "test-org";

/// The accounts of the org the app is installed in: `Org::new().admins([..]).members([..])`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Org {
    pub(crate) admins: BTreeSet<String>,
    pub(crate) members: BTreeSet<String>,
    pub(crate) guests: BTreeSet<String>,
}

impl Org {
    /// An org with no accounts besides the app owner.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds admins.
    #[must_use]
    pub fn admins<I, S>(mut self, accounts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.admins.extend(accounts.into_iter().map(Into::into));
        self
    }

    /// Adds members.
    #[must_use]
    pub fn members<I, S>(mut self, accounts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.members.extend(accounts.into_iter().map(Into::into));
        self
    }

    /// Adds guests.
    #[must_use]
    pub fn guests<I, S>(mut self, accounts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.guests.extend(accounts.into_iter().map(Into::into));
        self
    }

    pub(crate) fn remove(&mut self, account: &str) {
        self.admins.remove(account);
        self.members.remove(account);
        self.guests.remove(account);
    }

    pub(crate) fn accounts(&self) -> impl Iterator<Item = String> + '_ {
        let all = self.admins.iter().chain(&self.members).chain(&self.guests);
        std::iter::once(OWNER.to_owned()).chain(all.cloned())
    }

    pub(crate) fn role(&self, account: &str) -> Option<OrgRole> {
        if account == OWNER {
            Some(OrgRole::Owner)
        } else if self.admins.contains(account) {
            Some(OrgRole::Admin)
        } else if self.members.contains(account) {
            Some(OrgRole::Member)
        } else if self.guests.contains(account) {
            Some(OrgRole::Guest)
        } else {
            None
        }
    }
}

/// A snapshot of the org, taken per invocation.
pub(crate) struct HarnessDir(pub(crate) Org);

impl Directory for HarnessDir {
    fn org(&self) -> Option<String> {
        Some(ORG_ID.to_owned())
    }

    fn app_owner(&self) -> String {
        OWNER.to_owned()
    }

    fn org_role(&self, account: &str) -> Option<OrgRole> {
        self.0.role(account)
    }

    fn in_team(&self, _account: &str, _team: &str) -> bool {
        false
    }

    fn team_exists(&self, _team: &str) -> bool {
        false
    }

    fn orgs_of(&self, account: &str) -> Vec<(String, OrgRole)> {
        self.0
            .role(account)
            .map(|role| (ORG_ID.to_owned(), role))
            .into_iter()
            .collect()
    }
}
