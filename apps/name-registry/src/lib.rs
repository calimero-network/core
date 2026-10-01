//! Usernames no member can take from another: a [`Registry`] the TEE decides.
//!
//! A member asks for a name with [`NameRegistry::claim`]. That writes the
//! member's own claim, which decides nothing, and emits `NameClaimed` with the
//! TEE handler `tee:resolve_name`. The namespace's elected TEE authority runs
//! [`NameRegistry::resolve_name`] inside the enclave, which writes the verdict
//! into `TeeOnly` state: the first claim to reach the TEE wins, and claims it
//! sees together are split by an order no member can backdate. Every peer
//! accepts that verdict because its signer resolves to the TEE authority, and
//! drops any member's attempt to write one.
//!
//! A timer, [`NameRegistry::sweep`], resolves whatever a missed trigger left
//! pending. Firing is at least once, and every resolution is idempotent, so a
//! second firing, or a second TEE, decides nothing new.
//!
//! Only [`NameRegistry::owner_of`] says who holds a name. Holding a claim says
//! nothing: [`NameRegistry::status`] shows it as `pending` until the TEE
//! decides, and as `lost` to the claimant who did not get it.

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, AccountId};
use calimero_storage::collections::{Registry, Status};
use thiserror::Error;

/// The most names one sweep resolves, so one timer tick has a bounded cost.
const SWEEP_BUDGET: usize = 32;

#[app::state(emits = for<'a> Event<'a>)]
pub struct NameRegistry {
    /// Username → the display name its claimant attached. The TEE decides who
    /// owns each.
    names: Registry<String, String>,
}

#[app::event]
pub enum Event<'a> {
    /// A member claimed a name; the TEE handler decides it.
    NameClaimed { name: &'a str },
    /// An owner asked to free a name; the TEE handler answers.
    ReleaseRequested { name: &'a str },
    /// The TEE settled a name: its owner, if it has one now.
    NameResolved {
        name: &'a str,
        owner: Option<&'a str>,
    },
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
    /// The owner, once the TEE has granted the name.
    pub owner: Option<String>,
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
impl NameRegistry {
    #[app::init]
    pub fn init() -> NameRegistry {
        NameRegistry {
            names: Registry::new(),
        }
    }

    /// The caller's account, as `owner` fields show it.
    pub fn me(&self) -> app::Result<String> {
        Ok(caller().to_string())
    }

    /// Ask the TEE for `name`, showing `display` once it is yours.
    ///
    /// # Errors
    /// If `name` is not a username, or someone, the caller included, owns it.
    pub fn claim(&mut self, name: String, display: String) -> app::Result<()> {
        if !is_username(&name) {
            app::bail!(Error::BadName(&name));
        }
        self.names.claim(name.clone(), display)?;
        app::emit!((Event::NameClaimed { name: &name }, "tee:resolve_name"));
        Ok(())
    }

    /// Drop the caller's claim to `name`, or free it if the caller owns it.
    /// Returns what it did: `released`, `withdrawn` or `nothing`.
    ///
    /// A release takes effect when the TEE answers it.
    ///
    /// # Errors
    /// Any storage error.
    pub fn let_go(&mut self, name: String) -> app::Result<String> {
        if self.names.owner_of(&name)? == Some(caller()) {
            self.names.release(&name)?;
            app::emit!((Event::ReleaseRequested { name: &name }, "tee:resolve_name"));
            return Ok("released".to_owned());
        }
        Ok(if self.names.withdraw(&name)? {
            "withdrawn"
        } else {
            "nothing"
        }
        .to_owned())
    }

    /// Decide `name`. Runs only on the elected TEE authority.
    ///
    /// Idempotent: a name already decided is left as it is, so a replayed or
    /// doubled trigger cannot re-grant it.
    #[app::tee]
    #[app::handler]
    pub fn resolve_name(&mut self, name: String) -> app::Result<()> {
        let owner = self.names.resolve(&name)?.map(|owner| owner.to_string());
        app::emit!(Event::NameResolved {
            name: &name,
            owner: owner.as_deref(),
        });
        Ok(())
    }

    /// Decide every name a missed trigger left pending.
    #[app::tee(every = "1m")]
    pub fn sweep(&mut self) -> app::Result<()> {
        let _settled = self.names.resolve_all_pending(SWEEP_BUDGET)?;
        Ok(())
    }

    /// Where `name` stands, as the caller sees it.
    pub fn status(&self, name: String) -> app::Result<NameView> {
        let view = |state: &str, owner: Option<AccountId>, claimants: usize, mine: bool| NameView {
            state: state.to_owned(),
            owner: owner.map(|owner| owner.to_string()),
            claimants: u32::try_from(claimants).unwrap_or(u32::MAX),
            mine,
        };
        Ok(match self.names.status(&name)? {
            Status::Free => view("free", None, 0, false),
            Status::Pending { claimants, mine } => view("pending", None, claimants, mine),
            Status::Owned { owner, .. } => view("owned", Some(owner), 0, owner == caller()),
            Status::Lost { owner, .. } => view("lost", Some(owner), 0, false),
            // A registry the TEE decides is never contested.
            Status::Contested { claimants } => view("pending", None, claimants.len(), false),
        })
    }

    /// Who owns `name`, once the TEE has granted it.
    pub fn owner_of(&self, name: String) -> app::Result<Option<String>> {
        Ok(self.names.owner_of(&name)?.map(|owner| owner.to_string()))
    }

    /// The display name the owner of `name` attached.
    pub fn display_of(&self, name: String) -> app::Result<Option<String>> {
        Ok(self.names.value_of(&name)?)
    }

    /// The names the caller claims that the TEE has not decided yet.
    pub fn my_pending(&self) -> app::Result<Vec<String>> {
        Ok(self.names.my_pending()?)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use calimero_sdk::testing::TestHost;

    use super::*;

    const ANN: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    fn account(bytes: [u8; 32]) -> String {
        AccountId::from(bytes).to_string()
    }

    /// Run every TEE handler the members' calls asked for, as the scheduler
    /// would.
    fn fire_triggers(app: &mut TestHost<NameRegistry>) -> usize {
        let mut fired = 0;
        for event in app.take_events() {
            if event.handler.as_deref() == Some("tee:resolve_name") {
                let name: String = calimero_sdk::serde_json::from_slice::<
                    calimero_sdk::serde_json::Value,
                >(&event.data)
                .unwrap()["name"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                app.call_as_tee(|s| s.resolve_name(name.clone())).unwrap();
                fired += 1;
            }
        }
        fired
    }

    fn status_of(app: &mut TestHost<NameRegistry>, who: [u8; 32], name: &str) -> NameView {
        app.call_as_account(who, who, |s| s.status(name.to_owned()))
            .unwrap()
    }

    #[test]
    fn a_claim_is_pending_until_the_tee_grants_it() {
        let mut app = TestHost::new(NameRegistry::init);
        app.call_as_account(ANN, ANN, |s| s.claim("ann".into(), "Ann".into()))
            .unwrap();
        assert_eq!(status_of(&mut app, ANN, "ann").state, "pending");
        assert_eq!(app.view(|s| s.owner_of("ann".into())).unwrap(), None);
        assert_eq!(
            app.call_as_account(ANN, ANN, |s| s.my_pending()).unwrap(),
            ["ann"]
        );

        assert_eq!(fire_triggers(&mut app), 1);
        assert_eq!(
            app.view(|s| s.owner_of("ann".into())).unwrap(),
            Some(account(ANN))
        );
        assert_eq!(
            app.view(|s| s.display_of("ann".into())).unwrap(),
            Some("Ann".to_owned())
        );
        let view = status_of(&mut app, ANN, "ann");
        assert_eq!((view.state.as_str(), view.mine), ("owned", true));
        assert!(app
            .call_as_account(ANN, ANN, |s| s.my_pending())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn two_members_claiming_one_name_leave_one_owner() {
        let mut app = TestHost::new(NameRegistry::init);
        app.call_as_account(ANN, ANN, |s| s.claim("rose".into(), "Ann".into()))
            .unwrap();
        app.call_as_account(BOB, BOB, |s| s.claim("rose".into(), "Bob".into()))
            .unwrap();
        // Both claims reached the TEE before it ran: two triggers, one grant.
        assert_eq!(fire_triggers(&mut app), 2);

        let owner = app.view(|s| s.owner_of("rose".into())).unwrap().unwrap();
        let (winner, loser) = if owner == account(ANN) {
            (ANN, BOB)
        } else {
            (BOB, ANN)
        };
        assert_eq!(owner, account(winner));
        assert_eq!(status_of(&mut app, winner, "rose").state, "owned");
        let lost = status_of(&mut app, loser, "rose");
        assert_eq!(lost.state, "lost");
        assert_eq!(lost.owner, Some(owner));
        assert!(app
            .call_as_account(loser, loser, |s| s.claim("rose".into(), "again".into()))
            .is_err());
    }

    #[test]
    fn a_released_name_goes_to_the_next_claimant() {
        let mut app = TestHost::new(NameRegistry::init);
        app.call_as_account(ANN, ANN, |s| s.claim("rose".into(), "Ann".into()))
            .unwrap();
        let _ = fire_triggers(&mut app);

        assert_eq!(
            app.call_as_account(BOB, BOB, |s| s.let_go("rose".into()))
                .unwrap(),
            "nothing"
        );
        assert_eq!(
            app.call_as_account(ANN, ANN, |s| s.let_go("rose".into()))
                .unwrap(),
            "released"
        );
        // Still Ann's until the TEE answers.
        assert_eq!(
            app.view(|s| s.owner_of("rose".into())).unwrap(),
            Some(account(ANN))
        );
        assert_eq!(fire_triggers(&mut app), 1);
        assert_eq!(status_of(&mut app, BOB, "rose").state, "free");

        app.call_as_account(BOB, BOB, |s| s.claim("rose".into(), "Bob".into()))
            .unwrap();
        let _ = fire_triggers(&mut app);
        assert_eq!(
            app.view(|s| s.owner_of("rose".into())).unwrap(),
            Some(account(BOB))
        );
        assert_eq!(status_of(&mut app, ANN, "rose").state, "owned");
    }

    #[test]
    fn a_pending_claim_can_be_withdrawn() {
        let mut app = TestHost::new(NameRegistry::init);
        app.call_as_account(ANN, ANN, |s| s.claim("rose".into(), "Ann".into()))
            .unwrap();
        assert_eq!(
            app.call_as_account(ANN, ANN, |s| s.let_go("rose".into()))
                .unwrap(),
            "withdrawn"
        );
        let _ = fire_triggers(&mut app);
        assert_eq!(app.view(|s| s.owner_of("rose".into())).unwrap(), None);
        assert_eq!(status_of(&mut app, ANN, "rose").state, "free");
    }

    #[test]
    fn the_sweep_resolves_what_a_missed_trigger_left() {
        let mut app = TestHost::new(NameRegistry::init);
        app.call_as_account(ANN, ANN, |s| s.claim("ann".into(), "Ann".into()))
            .unwrap();
        app.call_as_account(BOB, BOB, |s| s.claim("bob".into(), "Bob".into()))
            .unwrap();
        let _missed = app.take_events();
        app.call_as_tee(|s| s.sweep()).unwrap();
        assert_eq!(
            app.view(|s| s.owner_of("ann".into())).unwrap(),
            Some(account(ANN))
        );
        assert_eq!(
            app.view(|s| s.owner_of("bob".into())).unwrap(),
            Some(account(BOB))
        );
        // Firing twice decides nothing new.
        let root = app.root_hash();
        app.call_as_tee(|s| s.sweep()).unwrap();
        app.call_as_tee(|s| s.resolve_name("ann".into())).unwrap();
        assert_eq!(app.root_hash(), root);
    }

    #[test]
    fn a_member_cannot_run_the_tee_resolver() {
        let mut app = TestHost::new(NameRegistry::init);
        app.call_as_account(ANN, ANN, |s| s.claim("ann".into(), "Ann".into()))
            .unwrap();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            app.call_as_account(ANN, ANN, |s| s.resolve_name("ann".into()))
        }));
        assert!(outcome.is_err(), "#[app::tee] must refuse a non-TEE caller");
        assert_eq!(app.view(|s| s.owner_of("ann".into())).unwrap(), None);
    }

    #[test]
    fn a_member_cannot_write_a_verdict_directly() {
        let mut app = TestHost::new(NameRegistry::init);
        app.call_as_account(ANN, ANN, |s| s.claim("ann".into(), "Ann".into()))
            .unwrap();
        let write = app.call_as_account(ANN, ANN, |s| s.names.resolve(&"ann".to_owned()));
        assert!(write.is_err(), "only the TEE authority writes verdicts");
        assert_eq!(app.view(|s| s.owner_of("ann".into())).unwrap(), None);
    }

    #[test]
    fn a_username_is_checked() {
        let mut app = TestHost::new(NameRegistry::init);
        for bad in ["ab", "Ann", "a-b-c", &"x".repeat(33)] {
            assert!(
                app.call_as_account(ANN, ANN, |s| s.claim(bad.to_owned(), "x".into()))
                    .is_err(),
                "{bad}"
            );
        }
        assert!(app.take_events().is_empty());
    }
}
