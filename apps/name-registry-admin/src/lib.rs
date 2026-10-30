//! Usernames no member can take from another, decided by the registry's admins.
//!
//! The v1 version was a `Registry<String, String, Admin>`; on the component SDK it is three
//! tables and these rules, each checked against `env::caller()`'s account:
//!
//! * **A claim decides nothing.** Any org member may `claim` a free or pending name: that
//!   writes the caller's own claim row (`(name, claimant)`, so nobody writes, edits or
//!   withdraws another's) and emits `NameClaimed` so an admin's client knows there is
//!   something to decide.
//! * **Only an admin decides.** `resolve` refuses (`NotAdmin`) unless the caller is in
//!   `admins`. It writes the name's one verdict: a release by the owner is answered with a
//!   vacancy at the next epoch, then the open epoch is granted to the first live claim, in
//!   the order the app's log applied them (v1 split claims seen together by a hash of the
//!   name, the epoch and the claimant; a log gives every replica the same first claim).
//! * **The admins change only through `setAdmins`**, which only an admin may call and which
//!   cannot empty the set. The app owner is the first and only admin at `init`.
//! * **Only `ownerOf` says who holds a name.** `status` shows a claim as `pending` until an
//!   admin decides, and as `lost` to a claimant of that epoch who did not get it.
//!
//! A name's epoch counts its releases: a claim bids for the open epoch (0 until a verdict,
//! the vacancy's epoch after a release), and claims for an earlier epoch are never
//! considered again, except that `rebid` renews a lost claim once the name is open.

use calimero_app_sdk::prelude::*;

/// The most claims one name is read with.
pub const MAX_CLAIMS: u32 = 1000;

/// One account's claim to one name.
#[derive(Record)]
#[table(key = ["name", "claimant"], index(by_name = ["name", "seq"]))]
pub struct Claim {
    pub name: String,
    pub claimant: String,
    /// The display name shown once the name is the claimant's.
    pub display: String,
    /// The epoch this claim bids for.
    pub epoch: u32,
    /// Set by the owner to ask the admins to free the name.
    pub release: bool,
    /// When the log applied this claim: the first live claim wins.
    pub seq: u64,
}

/// The admins' decision about a name: its owner at `epoch`, or a vacancy (`owner` null)
/// that opens `epoch` after a release.
#[derive(Record)]
#[table(key = "name")]
pub struct Verdict {
    pub name: String,
    pub epoch: u32,
    pub owner: Option<String>,
}

/// An account that may resolve names.
#[derive(Record)]
#[table(key = "account")]
pub struct Admin {
    pub account: String,
}

/// Where a name stands, as the caller sees it.
#[derive(Record)]
#[table(key = "state")]
pub struct NameView {
    /// `free`, `pending`, `owned` or `lost`.
    pub state: String,
    /// The owner, once an admin has granted the name.
    pub owner: Option<String>,
    /// The epoch the owner holds it at: 0 for the first grant, one more after each release.
    pub epoch: Option<u32>,
    /// How many accounts bid for it while it is pending.
    pub claimants: u32,
    /// Whether the caller bids for it or owns it.
    pub mine: bool,
}

/// The claims on a name, the same whoever asks.
#[derive(Record)]
#[table(key = "claims")]
pub struct ClaimTally {
    /// How many accounts hold a claim, stale ones included.
    pub claims: u32,
    /// How many of those ask for the name to be freed.
    pub releases: u32,
    /// The newest epoch any claim bids for.
    pub epoch: u32,
}

#[app::state(version = 1)]
pub struct AdminNameRegistry {
    claims: Table<Claim>,
    verdicts: Table<Verdict>,
    admins: Table<Admin>,
    /// The last claim `seq` handed out.
    claim_seq: Counter,
}

#[app::error]
pub enum Error {
    /// Not a username: 3 to 32 of a-z, 0-9 and `_`.
    BadName { name: String },
    /// Somebody, the caller included, owns the name.
    Owned { name: String },
    /// Only an admin may do this.
    NotAdmin,
    /// The admin set cannot be empty.
    NoAdmins,
}

#[app::event]
pub enum Event {
    /// A member claimed a name; an admin decides it.
    NameClaimed { name: String },
    /// An owner asked to free a name; an admin answers.
    ReleaseRequested { name: String },
    /// An admin settled a name: its owner, if it has one now.
    NameResolved { name: String, owner: Option<String> },
    /// An admin replaced the admins.
    AdminsChanged { admins: Vec<String> },
}

/// A username is 3 to 32 of `a-z`, `0-9` and `_`.
fn is_username(name: &str) -> bool {
    (3..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The epoch claims bid for: 0 before any verdict, else the verdict's.
fn open_epoch(verdict: Option<&Verdict>) -> u32 {
    verdict.map_or(0, |v| v.epoch)
}

/// Where a name stands for `me`, from its verdict and claims.
enum Status {
    Free,
    Pending { claimants: u32, mine: bool },
    Owned { owner: String, epoch: u32 },
    Lost { owner: String, epoch: u32 },
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[app::logic]
impl AdminNameRegistry {
    /// The app owner becomes the first and only admin.
    #[app::init]
    pub fn init() -> app::Result<Self> {
        let mut s = Self::load();
        s.admins.insert(Admin {
            account: env::app_owner(),
        })?;
        Ok(s)
    }

    /// The caller's account, as `owner` fields and `admins` show it.
    #[app::view]
    pub fn me(&self) -> app::Result<String> {
        guards::require_member()
    }

    /// The accounts that may resolve names, ascending.
    #[app::view]
    pub fn admins(&self) -> app::Result<Vec<String>> {
        guards::require_member()?;
        Ok(self.admin_list()?)
    }

    /// Hands the admin role to `admins`, replacing the current set; the caller keeps it only
    /// if it names itself. Admins only (`NotAdmin`); `NoAdmins` for an empty list.
    pub fn set_admins(&mut self, admins: Vec<String>) -> app::Result<()> {
        self.require_admin()?;
        let mut admins = admins;
        admins.sort();
        admins.dedup();
        if admins.is_empty() {
            app::bail!(Error::NoAdmins);
        }
        for old in self.admin_list()? {
            self.admins.remove(&old)?;
        }
        for account in &admins {
            self.admins.insert(Admin {
                account: account.clone(),
            })?;
        }
        env::emit(Event::AdminsChanged { admins }, Audience::Org)?;
        Ok(())
    }

    /// Asks the admins for `name`, showing `display` once it is the caller's. Claiming again
    /// replaces the caller's claim.
    pub fn claim(&mut self, name: String, display: String) -> app::Result<()> {
        let me = guards::require_member()?;
        if !is_username(&name) {
            app::bail!(Error::BadName { name });
        }
        let verdict = self.verdicts.get(&name)?;
        let claims = self.claims_of(&name)?;
        if Self::owned(verdict.as_ref(), &claims).is_some() {
            app::bail!(Error::Owned { name });
        }
        self.write_claim(&name, &me, display, open_epoch(verdict.as_ref()))?;
        env::emit(Event::NameClaimed { name }, Audience::Org)?;
        Ok(())
    }

    /// Renews the caller's earlier claim to `name`, with the display it carried, once the name
    /// is open again: for a claimant who lost it to an owner who has since released it.
    /// Returns `rebid`, or `nothing` when the caller has no claim to renew, gave the name up
    /// itself, already bids for it, or the name is not open.
    pub fn rebid(&mut self, name: String) -> app::Result<String> {
        let me = guards::require_member()?;
        let open = matches!(
            self.status_for(&name, &me)?,
            Status::Free | Status::Pending { mine: false, .. }
        );
        let earlier = self
            .claims
            .get(&(name.as_str(), me.as_str()))?
            .filter(|c| !c.release);
        let Some(claim) = earlier.filter(|_| open) else {
            return Ok("nothing".to_owned());
        };
        let epoch = open_epoch(self.verdicts.get(&name)?.as_ref());
        self.write_claim(&name, &me, claim.display, epoch)?;
        env::emit(Event::NameClaimed { name }, Audience::Org)?;
        Ok("rebid".to_owned())
    }

    /// Frees `name` if the caller owns it (the release takes effect when an admin resolves
    /// it), or withdraws the caller's pending claim. Returns `released`, `withdrawn` or
    /// `nothing`. A claim that lost is kept, so its claimant can `rebid`.
    pub fn let_go(&mut self, name: String) -> app::Result<String> {
        let me = guards::require_member()?;
        Ok(match self.status_for(&name, &me)? {
            Status::Owned { owner, .. } if owner == me => {
                self.claims
                    .update(&(name.as_str(), me.as_str()), |c| c.release = true)?;
                env::emit(Event::ReleaseRequested { name }, Audience::Org)?;
                "released"
            }
            Status::Pending { mine: true, .. } => {
                self.claims.remove(&(name.as_str(), me.as_str()))?;
                "withdrawn"
            }
            _ => "nothing",
        }
        .to_owned())
    }

    /// Decides `name`, as an admin (`NotAdmin` otherwise): answers its owner's release with a
    /// vacancy at the next epoch, then grants the open epoch to the first live claim. Returns
    /// the owner it then has. A name already decided is left as it is.
    pub fn resolve(&mut self, name: String) -> app::Result<Option<String>> {
        self.require_admin()?;
        let owner = self.settle(&name)?;
        env::emit(
            Event::NameResolved {
                name,
                owner: owner.clone(),
            },
            Audience::Org,
        )?;
        Ok(owner)
    }

    /// Where `name` stands, as the caller sees it.
    #[app::view]
    pub fn status(&self, name: String) -> app::Result<NameView> {
        let me = guards::require_member()?;
        let view = |state: &str, owner: Option<String>, epoch, claimants, mine| NameView {
            state: state.to_owned(),
            owner,
            epoch,
            claimants,
            mine,
        };
        Ok(match self.status_for(&name, &me)? {
            Status::Free => view("free", None, None, 0, false),
            Status::Pending { claimants, mine } => view("pending", None, None, claimants, mine),
            Status::Owned { owner, epoch } => {
                let mine = owner == me;
                view("owned", Some(owner), Some(epoch), 0, mine)
            }
            Status::Lost { owner, epoch } => view("lost", Some(owner), Some(epoch), 0, false),
        })
    }

    /// The claims on `name`, without the caller-relative fields of `status`.
    #[app::view]
    pub fn claim_tally(&self, name: String) -> app::Result<ClaimTally> {
        guards::require_member()?;
        let claims = self.claims_of(&name)?;
        Ok(ClaimTally {
            claims: count(claims.len()),
            releases: count(claims.iter().filter(|c| c.release).count()),
            epoch: claims.iter().map(|c| c.epoch).max().unwrap_or(0),
        })
    }

    /// Who owns `name`, once an admin has granted it. Gate every use of a name on this.
    #[app::view]
    pub fn owner_of(&self, name: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        let verdict = self.verdicts.get(&name)?;
        let claims = self.claims_of(&name)?;
        Ok(Self::owned(verdict.as_ref(), &claims).map(|c| c.claimant.clone()))
    }

    /// The display name the owner of `name` attached.
    #[app::view]
    pub fn display_of(&self, name: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        let verdict = self.verdicts.get(&name)?;
        let claims = self.claims_of(&name)?;
        Ok(Self::owned(verdict.as_ref(), &claims).map(|c| c.display.clone()))
    }

    fn require_admin(&self) -> app::Result<String> {
        let me = guards::require_member()?;
        if !self.admins.contains(&me)? {
            app::bail!(Error::NotAdmin);
        }
        Ok(me)
    }

    fn admin_list(&self) -> Result<Vec<String>, calimero_app_sdk::StorageError> {
        Ok(self
            .admins
            .scan(FindOpts::default().limit(MAX_CLAIMS))?
            .items
            .into_iter()
            .map(|a| a.account)
            .collect())
    }

    /// The claims on `name`, in the order the log applied them.
    fn claims_of(&self, name: &str) -> Result<Vec<Claim>, calimero_app_sdk::StorageError> {
        Ok(self
            .claims
            .find(
                "byName",
                &[KeyPart::Str(name.to_owned())],
                FindOpts::default().limit(MAX_CLAIMS),
            )?
            .items)
    }

    fn write_claim(
        &mut self,
        name: &str,
        me: &str,
        display: String,
        epoch: u32,
    ) -> app::Result<()> {
        let seq = self.claim_seq.increment()?;
        self.claims.upsert(Claim {
            name: name.to_owned(),
            claimant: me.to_owned(),
            display,
            epoch,
            release: false,
            seq,
        })?;
        Ok(())
    }

    /// The granted owner's claim, when the verdict is a grant and its claim is held.
    fn owned<'c>(verdict: Option<&Verdict>, claims: &'c [Claim]) -> Option<&'c Claim> {
        let verdict = verdict?;
        let owner = verdict.owner.as_ref()?;
        claims
            .iter()
            .find(|c| &c.claimant == owner && c.epoch == verdict.epoch)
    }

    fn status_for(&self, name: &str, me: &str) -> app::Result<Status> {
        let verdict = self.verdicts.get(name)?;
        let claims = self.claims_of(name)?;
        if let (Some(v), Some(owner)) = (verdict.as_ref(), Self::owned(verdict.as_ref(), &claims)) {
            let owner = owner.claimant.clone();
            let lost = owner != me
                && claims
                    .iter()
                    .any(|c| c.claimant == me && c.epoch == v.epoch);
            return Ok(if lost {
                Status::Lost {
                    owner,
                    epoch: v.epoch,
                }
            } else {
                Status::Owned {
                    owner,
                    epoch: v.epoch,
                }
            });
        }
        let epoch = open_epoch(verdict.as_ref());
        let bidders: Vec<&str> = claims
            .iter()
            .filter(|c| c.epoch == epoch && !c.release)
            .map(|c| c.claimant.as_str())
            .collect();
        Ok(if bidders.is_empty() {
            Status::Free
        } else {
            Status::Pending {
                claimants: count(bidders.len()),
                mine: bidders.contains(&me),
            }
        })
    }

    /// Writes whatever verdict `name` needs; returns its owner afterwards.
    fn settle(&mut self, name: &str) -> app::Result<Option<String>> {
        let claims = self.claims_of(name)?;
        let mut verdict = self.verdicts.get(name)?;
        if let Some(granted) = Self::owned(verdict.as_ref(), &claims) {
            if !granted.release {
                return Ok(Some(granted.claimant.clone()));
            }
            // The owner asked to let go: a vacancy opens the next epoch.
            let vacancy = Verdict {
                name: name.to_owned(),
                epoch: granted.epoch.saturating_add(1),
                owner: None,
            };
            self.verdicts.upsert(vacancy)?;
            verdict = self.verdicts.get(name)?;
        }
        let epoch = open_epoch(verdict.as_ref());
        let Some(winner) = claims.iter().find(|c| c.epoch == epoch && !c.release) else {
            return Ok(None);
        };
        self.verdicts.upsert(Verdict {
            name: name.to_owned(),
            epoch,
            owner: Some(winner.claimant.clone()),
        })?;
        Ok(Some(winner.claimant.clone()))
    }
}
