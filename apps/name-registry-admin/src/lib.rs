//! Usernames no member can take from another: a [`Registry`] its admins decide.
//!
//! A member asks for a name with [`AdminNameRegistry::claim`]. That writes the
//! member's own claim, which decides nothing, and emits `NameClaimed` so an
//! admin's client knows there is something to decide. An admin then calls
//! [`AdminNameRegistry::resolve`], which writes the verdict into a
//! `SharedStorage` cell whose writers are the admins: several claims it sees
//! together are split by an order derived from the name, the epoch and the
//! claimant, never from a clock. Every peer accepts that verdict because its
//! signer is in the cell's writer set as of the verdict, and drops any other
//! account's attempt to write one.
//!
//! The admins start as the account that ran `init` (the context's creator) and
//! change only through [`AdminNameRegistry::set_admins`], which only an admin
//! may call. Every node checks a rotation like any other writer-set change, so
//! an admin who handed the role on can no longer resolve.
//!
//! Only [`AdminNameRegistry::owner_of`] says who holds a name. Holding a claim
//! says nothing: [`AdminNameRegistry::status`] shows it as `pending` until an
//! admin decides, and as `lost` to the claimant who did not get it.

use std::collections::BTreeSet;

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, AccountId};
use calimero_storage::collections::{Admin, Registry, Status};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
pub struct AdminNameRegistry {
    /// Username → the display name its claimant attached. The admins decide
    /// who owns each.
    names: Registry<String, String, Admin>,
}

#[app::event]
pub enum Event<'a> {
    /// A member claimed a name; an admin decides it.
    NameClaimed { name: &'a str },
    /// An owner asked to free a name; an admin answers.
    ReleaseRequested { name: &'a str },
    /// An admin settled a name: its owner, if it has one now.
    NameResolved {
        name: &'a str,
        owner: Option<&'a str>,
    },
    /// An admin replaced the admins.
    AdminsChanged { admins: Vec<String> },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("`{0}` is not a username: 3 to 32 of a-z, 0-9 and `_`")]
    BadName(&'a str),
}

/// Where a name stands, as the caller sees it.
#[derive(Debug, PartialEq, Eq, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct NameView {
    /// `free`, `pending`, `owned` or `lost`.
    pub state: String,
    /// The owner, once an admin has granted the name.
    pub owner: Option<String>,
    /// The epoch the owner holds it at: 0 for the first grant, one more after
    /// each release.
    pub epoch: Option<u32>,
    /// How many accounts claim it while it is pending.
    pub claimants: u32,
    /// Whether the caller claims or owns it.
    pub mine: bool,
}

/// A username is 3 to 32 of `a-z`, `0-9` and `_`.
fn is_username(name: &str) -> bool {
    (3..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn caller() -> AccountId {
    AccountId::from(env::account_id())
}

#[app::logic]
impl AdminNameRegistry {
    /// The account running `init` becomes the first and only admin.
    #[app::init]
    pub fn init() -> AdminNameRegistry {
        AdminNameRegistry {
            names: Registry::new(),
        }
    }

    /// The caller's account, as `owner` fields and `admins` show it.
    pub fn me(&self) -> app::Result<String> {
        Ok(caller().to_string())
    }

    /// The accounts that may resolve names, ascending.
    pub fn admins(&self) -> app::Result<Vec<String>> {
        Ok(self
            .names
            .admins()
            .into_iter()
            .map(|admin| admin.to_string())
            .collect())
    }

    /// Hand the admin role to `admins`, replacing the current set. The caller
    /// keeps it only if it names itself.
    ///
    /// # Errors
    /// If the caller is not an admin, or `admins` is empty.
    pub fn set_admins(&mut self, admins: Vec<AccountId>) -> app::Result<()> {
        let admins: BTreeSet<AccountId> = admins.into_iter().collect();
        self.names.set_admins(admins.clone())?;
        app::emit!(Event::AdminsChanged {
            admins: admins.iter().map(ToString::to_string).collect(),
        });
        Ok(())
    }

    /// Ask the admins for `name`, showing `display` once it is yours.
    ///
    /// # Errors
    /// If `name` is not a username, or someone, the caller included, owns it.
    pub fn claim(&mut self, name: String, display: String) -> app::Result<()> {
        if !is_username(&name) {
            app::bail!(Error::BadName(&name));
        }
        self.names.claim(name.clone(), display)?;
        app::emit!(Event::NameClaimed { name: &name });
        Ok(())
    }

    /// Renew the caller's earlier claim to `name`, with the display it carried,
    /// once the name is open again: for a claimant who lost it to an owner who
    /// has since released it. Returns `rebid`, or `nothing` when the caller
    /// has no claim to renew, gave the name up itself, already bids for it, or
    /// the name is not open.
    ///
    /// # Errors
    /// Any storage error.
    pub fn rebid(&mut self, name: String) -> app::Result<String> {
        let me = caller();
        let open = matches!(
            self.names.status(&name)?,
            Status::Free | Status::Pending { mine: false, .. }
        );
        let earlier = self
            .names
            .claimants(&name)?
            .into_iter()
            .find(|(claimant, claim)| *claimant == me && !claim.release);
        let Some((_, claim)) = earlier.filter(|_| open) else {
            return Ok("nothing".to_owned());
        };
        self.names.claim(name.clone(), claim.value)?;
        app::emit!(Event::NameClaimed { name: &name });
        Ok("rebid".to_owned())
    }

    /// Free `name` if the caller owns it, or withdraw the caller's pending
    /// claim to it. Returns what it did: `released`, `withdrawn` or `nothing`.
    /// A claim that lost is kept, so its claimant can [`rebid`](Self::rebid)
    /// once the name is released.
    ///
    /// A release takes effect when an admin resolves the name.
    ///
    /// # Errors
    /// Any storage error.
    pub fn let_go(&mut self, name: String) -> app::Result<String> {
        Ok(match self.names.status(&name)? {
            Status::Owned { owner, .. } if owner == caller() => {
                self.names.release(&name)?;
                app::emit!(Event::ReleaseRequested { name: &name });
                "released"
            }
            Status::Pending { mine: true, .. } => {
                let _held = self.names.withdraw(&name)?;
                "withdrawn"
            }
            _ => "nothing",
        }
        .to_owned())
    }

    /// Decide `name`, as an admin: answer its owner's release with a vacancy,
    /// then grant the open epoch to one claimant. Returns the owner it then has.
    ///
    /// Idempotent: a name already decided is left as it is.
    ///
    /// # Errors
    /// If the caller is not an admin; any storage error.
    pub fn resolve(&mut self, name: String) -> app::Result<Option<String>> {
        let owner = self.names.resolve(&name)?.map(|owner| owner.to_string());
        app::emit!(Event::NameResolved {
            name: &name,
            owner: owner.as_deref(),
        });
        Ok(owner)
    }

    /// Where `name` stands, as the caller sees it.
    pub fn status(&self, name: String) -> app::Result<NameView> {
        let me = caller();
        let view =
            |state: &str, owner: Option<AccountId>, epoch, claimants: usize, mine| NameView {
                state: state.to_owned(),
                owner: owner.map(|owner| owner.to_string()),
                epoch,
                claimants: u32::try_from(claimants).unwrap_or(u32::MAX),
                mine,
            };
        Ok(match self.names.status(&name)? {
            Status::Free => view("free", None, None, 0, false),
            Status::Pending { claimants, mine } => view("pending", None, None, claimants, mine),
            Status::Owned { owner, epoch, .. } => {
                view("owned", Some(owner), Some(epoch), 0, owner == me)
            }
            Status::Lost { owner, epoch } => view("lost", Some(owner), Some(epoch), 0, false),
            // A registry with an authority is never contested.
            Status::Contested { claimants } => view(
                "pending",
                None,
                None,
                claimants.len(),
                claimants.contains(&me),
            ),
        })
    }

    /// Who owns `name`, once an admin has granted it.
    pub fn owner_of(&self, name: String) -> app::Result<Option<String>> {
        Ok(self.names.owner_of(&name)?.map(|owner| owner.to_string()))
    }

    /// The display name the owner of `name` attached.
    pub fn display_of(&self, name: String) -> app::Result<Option<String>> {
        Ok(self.names.value_of(&name)?)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    const ANN: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    fn account(bytes: [u8; 32]) -> String {
        AccountId::from(bytes).to_string()
    }

    /// A registry whose first admin is the harness's own account.
    fn registry() -> (TestHost<AdminNameRegistry>, [u8; 32]) {
        let app = TestHost::new(AdminNameRegistry::init);
        let admin = app.account_id();
        assert_ne!(admin, ANN);
        assert_ne!(admin, BOB);
        (app, admin)
    }

    fn status_of(app: &mut TestHost<AdminNameRegistry>, who: [u8; 32], name: &str) -> NameView {
        app.call_as_account(who, who, |s| s.status(name.to_owned()))
            .unwrap()
    }

    fn owner(app: &TestHost<AdminNameRegistry>, name: &str) -> Option<String> {
        app.view(|s| s.owner_of(name.to_owned())).unwrap()
    }

    fn claim(app: &mut TestHost<AdminNameRegistry>, who: [u8; 32], name: &str) {
        app.call_as_account(who, who, |s| s.claim(name.to_owned(), name.to_owned()))
            .unwrap();
    }

    fn resolve(
        app: &mut TestHost<AdminNameRegistry>,
        who: [u8; 32],
        name: &str,
    ) -> app::Result<Option<String>> {
        app.call_as_account(who, who, |s| s.resolve(name.to_owned()))
    }

    #[test]
    fn the_creator_is_the_first_admin() {
        let (app, admin) = registry();
        assert_eq!(app.view(|s| s.admins()).unwrap(), [account(admin)]);
    }

    #[test]
    fn a_claim_is_pending_until_an_admin_resolves_it() {
        let (mut app, admin) = registry();
        claim(&mut app, ANN, "ann");
        let view = status_of(&mut app, ANN, "ann");
        assert_eq!((view.state.as_str(), view.mine), ("pending", true));
        assert_eq!(owner(&app, "ann"), None);

        assert_eq!(resolve(&mut app, admin, "ann").unwrap(), Some(account(ANN)));
        assert_eq!(owner(&app, "ann"), Some(account(ANN)));
        let view = status_of(&mut app, ANN, "ann");
        assert_eq!(
            (view.state.as_str(), view.epoch, view.mine),
            ("owned", Some(0), true)
        );
        // Resolving again decides nothing new.
        let root = app.root_hash();
        let _ = resolve(&mut app, admin, "ann").unwrap();
        assert_eq!(app.root_hash(), root);
    }

    #[test]
    fn two_members_claiming_one_name_leave_one_owner() {
        let (mut app, admin) = registry();
        claim(&mut app, ANN, "rose");
        claim(&mut app, BOB, "rose");
        assert_eq!(status_of(&mut app, admin, "rose").claimants, 2);

        let winner_account = resolve(&mut app, admin, "rose").unwrap().unwrap();
        let (winner, loser) = if winner_account == account(ANN) {
            (ANN, BOB)
        } else {
            (BOB, ANN)
        };
        assert_eq!(winner_account, account(winner));
        assert_eq!(status_of(&mut app, winner, "rose").state, "owned");
        let lost = status_of(&mut app, loser, "rose");
        assert_eq!(lost.state, "lost");
        assert_eq!(lost.owner, Some(winner_account));
        assert!(app
            .call_as_account(loser, loser, |s| s.claim("rose".into(), "again".into()))
            .is_err());
        // A lost claim is not the loser's to renew while the name is owned.
        assert_eq!(
            app.call_as_account(loser, loser, |s| s.rebid("rose".into()))
                .unwrap(),
            "nothing"
        );
    }

    #[test]
    fn a_released_name_goes_to_the_other_claimant_at_the_next_epoch() {
        let (mut app, admin) = registry();
        claim(&mut app, ANN, "rose");
        claim(&mut app, BOB, "rose");
        let first = resolve(&mut app, admin, "rose").unwrap().unwrap();
        let (winner, loser) = if first == account(ANN) {
            (ANN, BOB)
        } else {
            (BOB, ANN)
        };

        // Both run the same calls; what each does depends on who won.
        for who in [ANN, BOB] {
            let did = app
                .call_as_account(who, who, |s| s.let_go("rose".into()))
                .unwrap();
            assert_eq!(did, if who == winner { "released" } else { "nothing" });
        }
        // Still the winner's until an admin answers.
        assert_eq!(owner(&app, "rose"), Some(first.clone()));
        assert_eq!(resolve(&mut app, admin, "rose").unwrap(), None);
        assert_eq!(status_of(&mut app, loser, "rose").state, "free");

        for who in [ANN, BOB] {
            let did = app
                .call_as_account(who, who, |s| s.rebid("rose".into()))
                .unwrap();
            assert_eq!(did, if who == loser { "rebid" } else { "nothing" });
        }
        assert_eq!(
            resolve(&mut app, admin, "rose").unwrap(),
            Some(account(loser))
        );
        let view = status_of(&mut app, admin, "rose");
        assert_eq!(
            (view.state.as_str(), view.owner, view.epoch),
            ("owned", Some(account(loser)), Some(1))
        );
        assert_eq!(
            app.view(|s| s.display_of("rose".into())).unwrap(),
            Some("rose".to_owned())
        );
        let former = status_of(&mut app, winner, "rose");
        assert_eq!((former.state.as_str(), former.mine), ("owned", false));
    }

    #[test]
    fn a_pending_claim_can_be_withdrawn() {
        let (mut app, admin) = registry();
        claim(&mut app, ANN, "rose");
        assert_eq!(
            app.call_as_account(ANN, ANN, |s| s.let_go("rose".into()))
                .unwrap(),
            "withdrawn"
        );
        assert_eq!(resolve(&mut app, admin, "rose").unwrap(), None);
        assert_eq!(status_of(&mut app, ANN, "rose").state, "free");
    }

    #[test]
    fn a_member_cannot_resolve() {
        let (mut app, _admin) = registry();
        claim(&mut app, ANN, "iris");
        let root = app.root_hash();
        assert!(resolve(&mut app, ANN, "iris").is_err());
        assert!(resolve(&mut app, BOB, "iris").is_err());
        assert_eq!(app.root_hash(), root);
        assert_eq!(owner(&app, "iris"), None);
        assert_eq!(status_of(&mut app, ANN, "iris").state, "pending");
    }

    #[test]
    fn a_member_cannot_make_itself_admin() {
        let (mut app, admin) = registry();
        assert!(app
            .call_as_account(ANN, ANN, |s| s.set_admins(vec![AccountId::from(ANN)]))
            .is_err());
        assert_eq!(app.view(|s| s.admins()).unwrap(), [account(admin)]);
    }

    #[test]
    fn the_admin_set_cannot_be_emptied() {
        let (mut app, admin) = registry();
        assert!(app
            .call_as_account(admin, admin, |s| s.set_admins(Vec::new()))
            .is_err());
        assert_eq!(app.view(|s| s.admins()).unwrap(), [account(admin)]);
    }

    #[test]
    fn a_rotated_out_admin_cannot_resolve_and_the_new_one_can() {
        let (mut app, admin) = registry();
        app.call_as_account(admin, admin, |s| s.set_admins(vec![AccountId::from(ANN)]))
            .unwrap();
        assert_eq!(app.view(|s| s.admins()).unwrap(), [account(ANN)]);

        claim(&mut app, BOB, "lily");
        claim(&mut app, BOB, "iris");
        assert!(resolve(&mut app, admin, "iris").is_err());
        assert_eq!(owner(&app, "iris"), None);
        assert_eq!(resolve(&mut app, ANN, "lily").unwrap(), Some(account(BOB)));
        assert_eq!(owner(&app, "lily"), Some(account(BOB)));

        // The new admin rotates on in turn; the old one cannot take it back.
        assert!(app
            .call_as_account(admin, admin, |s| s.set_admins(vec![AccountId::from(admin)]))
            .is_err());
        assert_eq!(app.view(|s| s.admins()).unwrap(), [account(ANN)]);
    }

    #[test]
    fn a_username_is_checked() {
        let (mut app, _admin) = registry();
        for bad in ["ab", "Ann", "a-b-c", &"x".repeat(33)] {
            assert!(
                app.call_as_account(ANN, ANN, |s| s.claim(bad.to_owned(), "x".into()))
                    .is_err(),
                "{bad}"
            );
        }
    }
}
